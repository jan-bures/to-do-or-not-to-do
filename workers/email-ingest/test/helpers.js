// Mocks for the Workers runtime objects: the email message, the queue, fetch and the logger.
// Every address here is a placeholder on a reserved example domain.

import { logRecord } from '../src/log.js';

export const OWNER = 'you@example.com';
export const SECRET = 'test-only-hmac-key-0123456789abcdef';
export const URL_ = 'https://todo.example.com/api/v1/ingest/email';
export const CF_AR = 'mx.cloudflare.net; dkim=pass header.d=example.com header.s=sel; dmarc=pass header.from=example.com; spf=pass smtp.mailfrom=you@example.com';

/**
 * Builds a raw MIME message (CRLF line endings).
 * @param {{ from?: string, to?: string, subject?: string, messageId?: string | null, text?: string,
 *   html?: string, extraHeaders?: string[] }} [o]
 */
export function mime(o = {}) {
  const headers = [
    ...(o.extraHeaders ?? []),
    `From: ${o.from ?? `Owner <${OWNER}>`}`,
    `To: ${o.to ?? 'todo@in.example.com'}`,
    `Subject: ${o.subject ?? 'Fwd: Quarterly report'}`,
    ...(o.messageId === null ? [] : [`Message-ID: ${o.messageId ?? '<fwd-1@mail.example.com>'}`]),
    'Date: Fri, 21 Nov 2025 08:00:00 +0000',
    'MIME-Version: 1.0',
  ];
  if (o.text !== undefined && o.html !== undefined) {
    const b = 'BOUNDARY42';
    return [
      ...headers,
      `Content-Type: multipart/alternative; boundary="${b}"`,
      '',
      `--${b}`,
      'Content-Type: text/plain; charset=utf-8',
      'Content-Transfer-Encoding: 8bit',
      '',
      o.text,
      `--${b}`,
      'Content-Type: text/html; charset=utf-8',
      '',
      o.html,
      `--${b}--`,
      '',
    ].join('\r\n');
  }
  if (o.html !== undefined) {
    return [...headers, 'Content-Type: text/html; charset=utf-8', '', o.html, ''].join('\r\n');
  }
  return [...headers, 'Content-Type: text/plain; charset=utf-8', 'Content-Transfer-Encoding: 8bit', '', o.text ?? forwardedText(), ''].join('\r\n');
}

export function forwardedText() {
  return [
    'Please handle this.',
    '',
    '---------- Forwarded message ---------',
    'From: Alice Example <alice@example.org>',
    'Date: Thu, 20 Nov 2025 at 17:00',
    'Subject: Quarterly report',
    'To: <you@example.com>',
    '',
    'Hello, the report is attached.',
  ].join('\r\n');
}

/**
 * A readable stream over bytes, delivered in chunks.
 * @param {Uint8Array} bytes
 * @param {number} [chunk]
 */
export function streamOf(bytes, chunk = 16 * 1024) {
  let off = 0;
  return new ReadableStream({
    pull(controller) {
      if (off >= bytes.length) {
        controller.close();
        return;
      }
      controller.enqueue(bytes.slice(off, off + chunk));
      off += chunk;
    },
  });
}

/**
 * A ForwardableEmailMessage stand-in. Header values come from the raw message unless overridden, and
 * Cloudflare's Authentication-Results is prepended like Email Routing does.
 * @param {{ raw?: string | Uint8Array, to?: string, envelopeFrom?: string, authResults?: string[] | null,
 *   headers?: Record<string, string> }} [o]
 */
export function emailMessage(o = {}) {
  const raw = typeof o.raw === 'string' || o.raw === undefined ? new TextEncoder().encode(o.raw ?? mime()) : o.raw;
  const headers = new Headers();
  for (const ar of o.authResults === undefined ? [CF_AR] : o.authResults ?? []) headers.append('authentication-results', ar);
  // Minimal header extraction from the raw message (unfolded, first block only).
  const text = new TextDecoder().decode(raw);
  const head = text.split(/\r?\n\r?\n/)[0].replace(/\r?\n[ \t]+/g, ' ');
  for (const line of head.split(/\r?\n/)) {
    const i = line.indexOf(':');
    if (i > 0) headers.append(line.slice(0, i).trim(), line.slice(i + 1).trim());
  }
  for (const [k, v] of Object.entries(o.headers ?? {})) headers.set(k, v);
  return {
    from: o.envelopeFrom ?? OWNER,
    to: o.to ?? 'todo@in.example.com',
    headers,
    raw: streamOf(raw),
    rawSize: raw.length,
    rejected: /** @type {string | null} */ (null),
    setReject(reason) {
      this.rejected = reason;
    },
  };
}

/** A queue producer that records what was sent; `fail` makes `send` throw. */
export function mockQueue({ fail = false } = {}) {
  return {
    /** @type {{ body: any, opts: any }[]} */
    sent: [],
    fail,
    async send(body, opts) {
      if (this.fail) throw new Error('queue unavailable');
      // Queues serialize with structured clone; JSON round-trip is close enough here.
      this.sent.push({ body: JSON.parse(JSON.stringify(body)), opts });
    },
  };
}

/**
 * A fetch stand-in. `responses` is consumed in order; each entry is a status code or an Error to throw.
 * @param {(number | Error)[]} responses
 */
export function mockFetch(responses) {
  /** @type {{ url: string, init: any, body: string }[]} */
  const calls = [];
  const fn = async (url, init) => {
    calls.push({ url, init, body: new TextDecoder().decode(init.body) });
    const next = responses.length > 1 ? responses.shift() : responses[0];
    if (next instanceof Error) throw next;
    return new Response(next === 204 ? null : '{}', { status: next });
  };
  fn.calls = calls;
  return fn;
}

/** A logger that records lines exactly as the real one would print them. */
export function mockLog() {
  /** @type {string[]} */
  const lines = [];
  const log = (event, fields) => {
    lines.push(JSON.stringify(logRecord(event, fields)));
  };
  log.lines = lines;
  return log;
}

/** Queue message stand-in. */
export function queueMessage(body) {
  return {
    body,
    acked: false,
    retried: /** @type {any} */ (null),
    ack() {
      this.acked = true;
    },
    retry(opts) {
      this.retried = opts ?? {};
    },
  };
}

export function baseEnv(overrides = {}) {
  return {
    ALLOWED_SENDERS: `${OWNER}, second@example.net`,
    INGEST_HMAC_SECRET: SECRET,
    INGEST_URL: URL_,
    INGEST_QUEUE: mockQueue(),
    ...overrides,
  };
}
