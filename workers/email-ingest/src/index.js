// Cloudflare Email Worker: forwarded email -> signed POST /api/v1/ingest/email.
//
// Bindings (see wrangler.example.toml):
//   ALLOWED_SENDERS     secret or var: comma-separated addresses allowed to create tasks
//   INGEST_HMAC_SECRET  secret: HMAC-SHA256 key, at least 32 characters
//   INGEST_URL          var: full URL of the server's ingest endpoint
//   INGEST_LOCAL_PART   var, optional: local part of the ingest address (default "todo")
//   INGEST_QUEUE        queue producer: retry queue, consumed by this same Worker
//   NOTIFY              send_email binding, optional: restricted to NOTIFY_ADDRESS
//   NOTIFY_ADDRESS      var, optional: verified destination address for failure notices
//   NOTIFY_FROM         var, optional: sender address on the Email Routing domain

import { domainOf, parseAllowlist, parseRecipient, parseSingleMailbox } from './address.js';
import { checkAuth, CLOUDFLARE_AUTHSERV_ID } from './auth.js';
import { checkEnvelope, parseEnvelope, postPayload, retryDelaySeconds } from './deliver.js';
import { MAX_RAW_BYTES } from './limits.js';
import { log as defaultLog, midHash } from './log.js';
import { buildPayload, fitPayload, hasHeaderEnd, messageIdFor, parseMime, readCapped } from './payload.js';
import { randomNonce } from './sign.js';

/**
 * @typedef {object} Deps
 * @property {typeof fetch} fetch
 * @property {() => number} now Milliseconds since the epoch.
 * @property {() => string} nonce
 * @property {import('./log.js').Logger} log
 */

/** SMTP rejection texts. They name no address and no content. */
const REJECT_TEXT = {
  not_allowed: 'Sender not allowed',
  auth: 'Sender authentication failed',
  recipient: 'Unknown recipient',
  bad_list: 'Invalid list name in address',
  unparseable: 'Message could not be parsed',
};

/**
 * The first Authentication-Results header: Email Routing prepends its own, so it is the first one.
 * `Headers.get()` joins repeated headers with ", "; `checkAuth` only reads up to the first top-level
 * comma, which is the end of the first header.
 * @param {Headers} headers
 * @returns {string | null}
 */
function firstAuthResults(headers) {
  return headers.get('authentication-results');
}

/**
 * Sends a failure notice to the owner, if configured. It carries the task title, the received time,
 * the reason and the Message-ID hash: enough to find and forward the email again.
 * @param {Record<string, any>} env
 * @param {{ mid: string, title: unknown, receivedAt: unknown, reason: string, status: number }} info
 * @param {Deps} deps
 */
async function notifyFailure(env, info, deps) {
  if (!env.NOTIFY || typeof env.NOTIFY.send !== 'function' || !env.NOTIFY_ADDRESS || !env.NOTIFY_FROM) {
    deps.log('notify_unconfigured', { mid: info.mid, reason: info.reason });
    return;
  }
  const lines = [
    'An email you forwarded could not be turned into a task. It was not stored.',
    '',
    `Title: ${typeof info.title === 'string' ? info.title : '(unknown)'}`,
    `Received: ${typeof info.receivedAt === 'string' ? info.receivedAt : '(unknown)'}`,
    `Reason: ${info.reason}${info.status ? ` (HTTP ${info.status})` : ''}`,
    `Reference: ${info.mid}`,
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
    deps.log('notify_sent', { mid: info.mid });
  } catch {
    deps.log('notify_failed', { mid: info.mid });
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

  // 1. Recipient: todo@ or todo+<list>@.
  const rcpt = parseRecipient(message.to, env.INGEST_LOCAL_PART || 'todo');
  if (!rcpt.ok) return reject(rcpt.reason === 'bad_list' ? 'bad_list' : 'recipient', rcpt.reason);

  // 2. Sender: exactly one From mailbox, on the allowlist. Checked from headers before reading the body.
  const from = parseSingleMailbox(message.headers.get('from'));
  if (!from) return reject('not_allowed', 'bad_from');
  const allow = parseAllowlist(env.ALLOWED_SENDERS);
  if (allow.size === 0) return reject('not_allowed', 'allowlist_empty');
  if (!allow.has(from.address)) return reject('not_allowed', 'not_allowed');

  // 3. Authentication: an aligned SPF, DKIM or DMARC pass in Cloudflare's Authentication-Results.
  const auth = checkAuth(
    firstAuthResults(message.headers),
    domainOf(from.address),
    env.TRUSTED_AUTHSERV_ID || CLOUDFLARE_AUTHSERV_ID,
  );
  if (!auth.ok) return reject('auth', auth.reason);

  // 4. Read at most MAX_RAW_BYTES and parse.
  const receivedAtMs = deps.now();
  const { bytes, truncated } = await readCapped(message.raw, MAX_RAW_BYTES);
  if (!hasHeaderEnd(bytes)) return reject('unparseable', 'headers_too_large');
  let email;
  try {
    email = await parseMime(bytes);
  } catch {
    return reject('unparseable', 'unparseable');
  }
  const messageId = await messageIdFor(message.headers.get('message-id'), bytes);
  const mid = await midHash(messageId);

  // 5. Payload: metadata only, plus a capped text body for #keep.
  let fitted;
  try {
    fitted = fitPayload(buildPayload({ messageId, list: rcpt.list, from, email, receivedAtMs, rawTruncated: truncated }));
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
  };

  // 6. Deliver now; queue on a transient failure.
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
  deps.log('queued', { mid, reason: res.reason, status: res.status, attempt: 1, delay_s: delay });
  return 'queued';
}

/**
 * Handles one batch from the retry queue.
 * @param {{ messages: readonly any[] }} batch
 * @param {Record<string, any>} env
 * @param {Deps} deps
 */
export async function handleQueue(batch, env, deps) {
  for (const msg of batch.messages) {
    const envelope = parseEnvelope(msg.body);
    if (!envelope) {
      deps.log('queue_invalid', {});
      msg.ack();
      continue;
    }
    const { mid, payload, accepted_at: acceptedAt, attempt } = envelope;
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
    const info = { mid, title: payload.title, receivedAt: payload.received_at, status: res.status };
    if (res.outcome === 'permanent') {
      deps.log('permanent_failure', { mid, status: res.status, attempt });
      msg.ack();
      await notifyFailure(env, { ...info, reason: 'server refused it' }, deps);
      continue;
    }
    const next = attempt + 1;
    const delay = retryDelaySeconds(next, acceptedAt, deps.now());
    if (delay === null) {
      deps.log('final_failure', { mid, reason: res.reason, status: res.status, attempt });
      msg.ack();
      await notifyFailure(env, { ...info, reason: 'server unreachable for 24 hours' }, deps);
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
 * @param {Partial<Deps>} [overrides]
 */
export function createWorker(overrides = {}) {
  /** @type {Deps} */
  const deps = {
    fetch: (...args) => fetch(...args),
    now: () => Date.now(),
    nonce: randomNonce,
    log: defaultLog,
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
     * @param {{ messages: readonly any[] }} batch
     * @param {Record<string, any>} env
     */
    async queue(batch, env) {
      await handleQueue(batch, env, deps);
    },
  };
}

export default createWorker();
