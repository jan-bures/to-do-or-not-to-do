// Logging that can't leak: only allowlisted fields, and none of them can hold an address, a subject, a
// body or a secret. `mid` is a short hash of the Message-ID, which is enough to correlate retries.

import { sha256Hex } from './text.js';

/** Fields a log line may carry. Values must be numbers, booleans or short fixed tokens. */
const ALLOWED_FIELDS = new Set([
  'reason',
  'status',
  'mid',
  'attempt',
  'delay_s',
  'via',
  'has_list',
  'keep',
  'body_truncated',
  'raw_truncated',
  'parse_failed',
  'bytes',
  // DEBUG_AUTH_SHAPE: booleans only.
  'ar_present',
  'authserv_ok',
  'dkim_pass',
  'header_d_present',
  'header_d_aligned',
  'spf_pass',
  'mailfrom_present',
  'mailfrom_aligned',
  'dmarc_pass',
]);

/** Fields that may only ever carry a boolean. */
const BOOLEAN_FIELDS = new Set([
  'has_list',
  'keep',
  'body_truncated',
  'raw_truncated',
  'parse_failed',
  'ar_present',
  'authserv_ok',
  'dkim_pass',
  'header_d_present',
  'header_d_aligned',
  'spf_pass',
  'mailfrom_present',
  'mailfrom_aligned',
  'dmarc_pass',
]);

/** Shape of the tokens allowed in `reason` and `via`. */
const TOKEN_RE = /^[a-z0-9_]{1,40}$/;

/**
 * Short, non-reversible tag for a Message-ID, used as the dedupe hash in logs: the first 12 hex
 * characters of its SHA-256.
 * @param {string} messageId
 * @returns {Promise<string>}
 */
export async function midHash(messageId) {
  return (await sha256Hex(messageId)).slice(0, 12);
}

/**
 * Filters a log record to its safe fields.
 * @param {string} event
 * @param {Record<string, unknown>} [fields]
 * @returns {Record<string, string | number | boolean>}
 */
export function logRecord(event, fields = {}) {
  /** @type {Record<string, string | number | boolean>} */
  const rec = { evt: TOKEN_RE.test(event) ? event : 'invalid_event' };
  for (const [k, v] of Object.entries(fields)) {
    if (!ALLOWED_FIELDS.has(k)) continue;
    if (BOOLEAN_FIELDS.has(k)) {
      if (typeof v === 'boolean') rec[k] = v;
    } else if (typeof v === 'number' || typeof v === 'boolean') rec[k] = v;
    else if (typeof v === 'string' && (k === 'mid' ? /^[0-9a-f]{12}$/.test(v) : TOKEN_RE.test(v))) rec[k] = v;
  }
  return rec;
}

/**
 * @typedef {(event: string, fields?: Record<string, unknown>) => void} Logger
 */

/** @type {Logger} */
export function log(event, fields) {
  console.log(JSON.stringify(logRecord(event, fields)));
}
