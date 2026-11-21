// Cloudflare Email Worker: forwarded email -> signed POST /api/v1/ingest/email.
//
// Bindings (see wrangler.example.toml):
//   ALLOWED_SENDERS       secret or var: comma-separated addresses allowed to create tasks
//   INGEST_HMAC_SECRET    secret: HMAC-SHA256 key, at least 32 characters
//   INGEST_URL            var: full URL of the server's ingest endpoint
//   INGEST_LOCAL_PART     var, optional: local part of the ingest address (default "todo")
//   RECIPIENT_SECRET      secret, optional: when set, only todo-<secret>@ (and +<list>) is accepted
//   MAX_ACCEPTED_PER_DAY  var, optional: accepted emails per isolate per 24 h (default 50)
//   DEBUG_AUTH_SHAPE      var, optional: "true" logs the Authentication-Results shape as booleans
//   INGEST_QUEUE          queue producer: retry queue, consumed by this same Worker
//   DEAD_LETTER_QUEUE     var, optional: name of the dead-letter queue (default below)
//   NOTIFY                send_email binding, optional: restricted to NOTIFY_ADDRESS
//   NOTIFY_ADDRESS        var, optional: verified destination address for failure notices
//   NOTIFY_FROM           var, optional: sender address on the Email Routing domain

import { domainOf, parseAllowlist, parseRecipient, parseSingleMailbox, recipientSecret } from './address.js';
import { authShape, checkAuth, CLOUDFLARE_AUTHSERV_ID } from './auth.js';
import { checkEnvelope, parseEnvelope, postPayload, retryDelaySeconds } from './deliver.js';
import { DEFAULT_MAX_ACCEPTED_PER_DAY, MAX_RAW_BYTES, RETRY_WINDOW_MS } from './limits.js';
import { log as defaultLog, midHash } from './log.js';
import { buildPayload, fitPayload, hasHeaderEnd, headersOnly, messageIdFor, parseMime, readCapped } from './payload.js';
import { randomNonce } from './sign.js';

/** Default dead-letter queue name; matches wrangler.example.toml. */
export const DEFAULT_DEAD_LETTER_QUEUE = 'todo-email-ingest-dlq';

/** Delay before a failed failure notice is retried, in seconds. */
const NOTIFY_RETRY_DELAY_S = 300;

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * @typedef {{ admit(nowMs: number, max: number): boolean }} Admission
 * @typedef {object} Deps
 * @property {typeof fetch} fetch
 * @property {() => number} now Milliseconds since the epoch.
 * @property {() => string} nonce
 * @property {import('./log.js').Logger} log
 * @property {Admission} admission
 */

/** SMTP rejection texts. They name no address, no secret and no content. */
const REJECT_TEXT = {
  not_allowed: 'Sender not allowed',
  auth: 'Sender authentication failed',
  recipient: 'Unknown recipient',
  bad_list: 'Invalid list name in address',
  unparseable: 'Message could not be parsed',
};

/** Failure notice reasons, as shown to the owner. */
const NOTICE_TEXT = {
  refused: 'the server refused it',
  expired: 'the server was unreachable for 23 hours',
  dead_letter: 'delivery stopped after repeated errors',
};

/**
 * A per-isolate admission counter: at most `max` accepted emails in any sliding 24 h window. It is a
 * cheap guard, not an account-wide limit: Cloudflare may run several isolates, and an isolate's
 * memory is lost when it's recycled.
 * @returns {Admission}
 */
export function createAdmission() {
  /** @type {number[]} */
  const times = [];
  return {
    admit(nowMs, max) {
      while (times.length > 0 && times[0] <= nowMs - DAY_MS) times.shift();
      if (times.length >= max) return false;
      times.push(nowMs);
      return true;
    },
  };
}

/**
 * @param {unknown} value
 * @returns {number}
 */
function maxAcceptedPerDay(value) {
  const n = typeof value === 'number' ? value : Number.parseInt(String(value ?? ''), 10);
  return Number.isInteger(n) && n >= 1 && n <= 100_000 ? n : DEFAULT_MAX_ACCEPTED_PER_DAY;
}

/**
 * Sends a failure notice to the owner. It carries the task title, the received time, the reason and
 * the Message-ID hash: enough to find and forward the email again.
 * @param {Record<string, any>} env
 * @param {{ mid: string | undefined, title: unknown, receivedAt: unknown, reason: keyof typeof NOTICE_TEXT,
 *   status: number }} info
 * @param {Deps} deps
 * @returns {Promise<'sent' | 'unconfigured' | 'failed'>}
 */
async function notifyFailure(env, info, deps) {
  if (!env.NOTIFY || typeof env.NOTIFY.send !== 'function' || !env.NOTIFY_ADDRESS || !env.NOTIFY_FROM) {
    deps.log('notify_unconfigured', { mid: info.mid, reason: info.reason });
    return 'unconfigured';
  }
  const lines = [
    'An email you forwarded could not be turned into a task. It was not stored.',
    '',
    `Title: ${typeof info.title === 'string' ? info.title : '(unknown)'}`,
    `Received: ${typeof info.receivedAt === 'string' ? info.receivedAt : '(unknown)'}`,
    `Reason: ${NOTICE_TEXT[info.reason]}${info.status ? ` (HTTP ${info.status})` : ''}`,
    `Reference: ${info.mid ?? 'unknown'}`,
    '',
    'Forward it again once the server is reachable.',
  ];
  try {
    await env.NOTIFY.send({
      to: env.NOTIFY_ADDRESS,
      from: env.NOTIFY_FROM,
      subject: 'Email to task failed',
      text: lines.join('\n'),
    });
    deps.log('notify_sent', { mid: info.mid, reason: info.reason });
    return 'sent';
  } catch {
    deps.log('notify_failed', { mid: info.mid, reason: info.reason });
    return 'failed';
  }
}

/**
 * Handles one incoming email.
 * @param {any} message ForwardableEmailMessage
 * @param {Record<string, any>} env
 * @param {Deps} deps
 * @returns {Promise<string>} The outcome token (also logged), for tests.
 */
export async function handleEmail(message, env, deps) {
  const reject = (/** @type {keyof typeof REJECT_TEXT} */ text, /** @type {string} */ reason) => {
    message.setReject(REJECT_TEXT[text]);
    deps.log('rejected', { reason });
    return `rejected:${reason}`;
  };

  // 1. Recipient: todo@ or todo+<list>@, or todo-<secret>@ and todo-<secret>+<list>@ when a recipient
  //    secret is configured. A malformed secret rejects everything rather than fall back.
  const rs = recipientSecret(env.RECIPIENT_SECRET);
  if ('error' in rs) return reject('recipient', 'config_recipient_secret');
  const rcpt = parseRecipient(message.to, env.INGEST_LOCAL_PART || 'todo', rs.secret);
  if (!rcpt.ok) return reject(rcpt.reason === 'bad_list' ? 'bad_list' : 'recipient', rcpt.reason);

  // 2. Sender: exactly one From mailbox. Checked from headers before reading the body.
  const from = parseSingleMailbox(message.headers.get('from'));
  if (!from) return reject('not_allowed', 'bad_from');

  // Email Routing prepends its own Authentication-Results, so it is the first one. Headers.get()
  // joins repeated headers with ", "; the parser stops at the first top-level comma.
  const authResults = message.headers.get('authentication-results');
  const trustedId = env.TRUSTED_AUTHSERV_ID || CLOUDFLARE_AUTHSERV_ID;
  if (env.DEBUG_AUTH_SHAPE === 'true') deps.log('auth_shape', authShape(authResults, domainOf(from.address), trustedId));

  // 3. Allowlist.
  const allow = parseAllowlist(env.ALLOWED_SENDERS);
  if (allow.size === 0) return reject('not_allowed', 'allowlist_empty');
  if (!allow.has(from.address)) return reject('not_allowed', 'not_allowed');

  // 4. Authentication: a DKIM or SPF pass for exactly the From domain.
  const auth = checkAuth(authResults, domainOf(from.address), trustedId);
  if (!auth.ok) return reject('auth', auth.reason);

  // 5. Admission budget. Over it, throw: the message isn't accepted, and the sending server retries.
  if (!deps.admission.admit(deps.now(), maxAcceptedPerDay(env.MAX_ACCEPTED_PER_DAY))) {
    deps.log('admission_limited', {});
    throw new Error('email-ingest: admission budget exhausted, try again later');
  }

  // 6. Read at most MAX_RAW_BYTES and parse.
  const receivedAtMs = deps.now();
  const { bytes, truncated } = await readCapped(message.raw, MAX_RAW_BYTES);
  if (!hasHeaderEnd(bytes)) return reject('unparseable', 'headers_too_large');
  // A message postal-mime refuses (too many parts, too deeply nested, malformed) still becomes a
  // task, from its headers only, rather than bouncing.
  let email;
  let parseFailed = false;
  try {
    email = await parseMime(bytes);
  } catch {
    email = headersOnly(message.headers);
    parseFailed = true;
  }
  const messageId = await messageIdFor(message.headers.get('message-id'), bytes);
  const mid = await midHash(messageId);

  // 7. Payload: metadata only, plus a capped text body for #keep.
  let fitted;
  try {
    fitted = fitPayload(buildPayload({ messageId, list: rcpt.list, from, email, receivedAtMs, rawTruncated: truncated, parseFailed }));
  } catch {
    return reject('unparseable', 'payload_too_large');
  }
  const { payload, json } = fitted;
  const accepted = {
    mid,
    via: auth.via,
    has_list: rcpt.list !== null,
    keep: payload.keep,
    body_truncated: payload.body?.truncated ?? false,
    raw_truncated: truncated,
    parse_failed: parseFailed,
  };

  // 8. Deliver now; queue on a transient failure.
  const res = await postPayload(env, json, deps);
  if (res.outcome === 'ok') {
    deps.log('delivered', { ...accepted, status: res.status, attempt: 0 });
    return 'delivered';
  }
  if (res.outcome === 'permanent') {
    // The forwarder gets a bounce at SMTP time, so nothing is lost silently.
    message.setReject(`Task server refused the message (HTTP ${res.status})`);
    deps.log('permanent_failure', { mid, status: res.status, attempt: 0 });
    return 'rejected:permanent';
  }
  const delay = retryDelaySeconds(1, receivedAtMs, deps.now()) ?? 1;
  try {
    const envelope = checkEnvelope({ v: 1, accepted_at: receivedAtMs, attempt: 1, mid, payload });
    await env.INGEST_QUEUE.send(envelope, { delaySeconds: delay });
  } catch {
    // Neither the server nor the queue took it. Throw, so Email Routing doesn't treat the message as
    // handled, rather than accept it and lose it.
    deps.log('queue_failed', { mid, reason: res.reason, status: res.status });
    throw new Error('email-ingest: delivery and queueing both failed');
  }
  deps.log('queued', { ...accepted, reason: res.reason, status: res.status, attempt: 1, delay_s: delay });
  return 'queued';
}

/**
 * Ends a queued delivery: sends the failure notice first, and only then acks. If the notice fails, the
 * message is retried (and, after max_retries, moved to the dead-letter queue, whose consumer tries the
 * notice again). Without a configured notice there is nothing more to do, so it acks.
 * @param {any} msg
 * @param {Record<string, any>} env
 * @param {Deps} deps
 * @param {{ mid: string | undefined, title: unknown, receivedAt: unknown, status: number }} info
 * @param {keyof typeof NOTICE_TEXT} reason
 */
async function finish(msg, env, deps, info, reason) {
  const notice = await notifyFailure(env, { ...info, reason }, deps);
  if (notice === 'failed') msg.retry({ delaySeconds: NOTIFY_RETRY_DELAY_S });
  else msg.ack();
}

/**
 * Handles one batch from the retry queue or the dead-letter queue.
 * @param {{ queue?: string, messages: readonly any[] }} batch
 * @param {Record<string, any>} env
 * @param {Deps} deps
 */
export async function handleQueue(batch, env, deps) {
  const dlq = env.DEAD_LETTER_QUEUE || DEFAULT_DEAD_LETTER_QUEUE;
  if (batch.queue === dlq) {
    await handleDeadLetters(batch, env, deps);
    return;
  }
  for (const msg of batch.messages) {
    const envelope = parseEnvelope(msg.body);
    if (!envelope) {
      deps.log('queue_invalid', {});
      msg.ack();
      continue;
    }
    const { mid, payload, accepted_at: acceptedAt } = envelope;
    // A re-enqueued message starts at msg.attempts = 1 and carries its count in the envelope. After a
    // msg.retry() fallback the envelope is unchanged and the platform counts instead, so add both.
    const redeliveries = Number.isInteger(msg.attempts) && msg.attempts > 1 ? msg.attempts - 1 : 0;
    const attempt = envelope.attempt + redeliveries;
    const info = { mid, title: payload.title, receivedAt: payload.received_at, status: 0 };

    // The deadline is checked before the POST: an expired message only gets its notice.
    if (deps.now() >= acceptedAt + RETRY_WINDOW_MS) {
      deps.log('final_failure', { mid, reason: 'expired', attempt });
      await finish(msg, env, deps, info, 'expired');
      continue;
    }

    let res;
    try {
      res = await postPayload(env, JSON.stringify(payload), deps);
    } catch {
      res = { outcome: /** @type {const} */ ('transient'), status: 0, reason: 'internal' };
    }
    if (res.outcome === 'ok') {
      deps.log('delivered', { mid, status: res.status, attempt });
      msg.ack();
      continue;
    }
    if (res.outcome === 'permanent') {
      deps.log('permanent_failure', { mid, status: res.status, attempt });
      await finish(msg, env, deps, { ...info, status: res.status }, 'refused');
      continue;
    }
    const next = attempt + 1;
    const delay = retryDelaySeconds(next, acceptedAt, deps.now());
    if (delay === null) {
      deps.log('final_failure', { mid, reason: res.reason, status: res.status, attempt });
      await finish(msg, env, deps, { ...info, status: res.status }, 'expired');
      continue;
    }
    try {
      // Re-enqueue as a new message rather than msg.retry(): that resets the queue's 24 h retention
      // clock and keeps our own attempt count, so the deadline is ours, not the queue's.
      await env.INGEST_QUEUE.send(checkEnvelope({ ...envelope, attempt: next }), { delaySeconds: delay });
      msg.ack();
      deps.log('requeued', { mid, reason: res.reason, status: res.status, attempt: next, delay_s: delay });
    } catch {
      msg.retry({ delaySeconds: delay });
      deps.log('retry_fallback', { mid, reason: res.reason, status: res.status, attempt: next, delay_s: delay });
    }
  }
}

/**
 * Handles the dead-letter queue: messages the platform gave up on after max_retries (the consumer threw,
 * or retries kept failing). Each gets a failure notice; a failed notice is retried.
 * @param {{ messages: readonly any[] }} batch
 * @param {Record<string, any>} env
 * @param {Deps} deps
 */
async function handleDeadLetters(batch, env, deps) {
  for (const msg of batch.messages) {
    const envelope = parseEnvelope(msg.body);
    const info = envelope
      ? { mid: envelope.mid, title: envelope.payload.title, receivedAt: envelope.payload.received_at, status: 0 }
      : { mid: undefined, title: undefined, receivedAt: undefined, status: 0 };
    deps.log('dead_letter', { mid: info.mid, attempt: envelope?.attempt });
    await finish(msg, env, deps, info, 'dead_letter');
  }
}

/**
 * @param {Partial<Deps>} [overrides]
 */
export function createWorker(overrides = {}) {
  /** @type {Deps} */
  const deps = {
    fetch: (...args) => fetch(...args),
    now: () => Date.now(),
    nonce: randomNonce,
    log: defaultLog,
    admission: createAdmission(),
    ...overrides,
  };
  return {
    /**
     * @param {any} message
     * @param {Record<string, any>} env
     */
    async email(message, env) {
      await handleEmail(message, env, deps);
    },
    /**
     * @param {{ queue?: string, messages: readonly any[] }} batch
     * @param {Record<string, any>} env
     */
    async queue(batch, env) {
      await handleQueue(batch, env, deps);
    },
  };
}

export default createWorker();
