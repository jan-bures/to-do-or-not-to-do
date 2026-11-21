// Delivery to the server: the signed POST, failure classification, the retry schedule and the queued
// envelope.

import {
  MAX_PAYLOAD_BYTES,
  MAX_QUEUE_MESSAGE_BYTES,
  MAX_RETRIES,
  MAX_RETRY_DELAY_S,
  MIN_SECRET_CHARS,
  POST_TIMEOUT_MS,
  RETRY_DELAYS_S,
  RETRY_WINDOW_MS,
} from './limits.js';
import { signedHeaders } from './sign.js';
import { utf8Length } from './text.js';

const encoder = new TextEncoder();

/**
 * @typedef {'ok' | 'transient' | 'permanent'} Outcome
 * @typedef {{ outcome: Outcome, status: number, reason: string }} DeliveryResult
 */

/**
 * Classifies an HTTP status. 2xx is delivered. 429, 408 and 5xx are transient. Every other status
 * (other 4xx, and 3xx, which `redirect: "manual"` surfaces) is permanent.
 * @param {number} status
 * @returns {Outcome}
 */
export function classifyStatus(status) {
  if (status >= 200 && status < 300) return 'ok';
  if (status === 429 || status === 408 || status >= 500) return 'transient';
  return 'permanent';
}

/**
 * Checks the delivery configuration. A missing or bad value is treated as transient, so mail accepted
 * while the Worker is misconfigured is queued and delivered once the configuration is fixed.
 * @param {Record<string, unknown>} env
 * @returns {string | null} A reason token, or null if the configuration is usable.
 */
export function configProblem(env) {
  const secret = env.INGEST_HMAC_SECRET;
  if (typeof secret !== 'string' || secret.length < MIN_SECRET_CHARS) return 'config_secret';
  const url = env.INGEST_URL;
  if (typeof url !== 'string') return 'config_url';
  try {
    const u = new URL(url);
    if (u.protocol !== 'https:' && !(u.protocol === 'http:' && u.hostname === 'localhost')) return 'config_url';
  } catch {
    return 'config_url';
  }
  return null;
}

/**
 * POSTs a payload with a fresh timestamp, nonce and signature.
 * @param {Record<string, any>} env
 * @param {string} body Serialized payload.
 * @param {{ fetch: typeof fetch, now: () => number, nonce: () => string }} deps
 * @returns {Promise<DeliveryResult>}
 */
export async function postPayload(env, body, deps) {
  const problem = configProblem(env);
  if (problem) return { outcome: 'transient', status: 0, reason: problem };
  const bytes = encoder.encode(body);
  const headers = await signedHeaders(env.INGEST_HMAC_SECRET, bytes, { nowMs: deps.now(), nonce: deps.nonce() });
  let res;
  try {
    res = await deps.fetch(env.INGEST_URL, {
      method: 'POST',
      headers,
      body: bytes,
      redirect: 'manual',
      signal: AbortSignal.timeout(POST_TIMEOUT_MS),
    });
  } catch {
    return { outcome: 'transient', status: 0, reason: 'network' };
  }
  // The response body is never needed; release it.
  try {
    await res.body?.cancel();
  } catch {
    // Ignore: nothing depends on it.
  }
  const outcome = classifyStatus(res.status);
  return { outcome, status: res.status, reason: outcome === 'ok' ? 'delivered' : `http_${outcome}` };
}

/**
 * Delay before retry `n` (1-based), in seconds, or null when the Worker should give up: the retry
 * window since acceptance has passed, or `MAX_RETRIES` retries were made. The last delay is shortened
 * so the final attempt lands at the end of the window.
 * @param {number} n
 * @param {number} acceptedAtMs
 * @param {number} nowMs
 * @returns {number | null}
 */
export function retryDelaySeconds(n, acceptedAtMs, nowMs) {
  if (!Number.isInteger(n) || n < 1 || n > MAX_RETRIES) return null;
  const remaining = Math.floor((acceptedAtMs + RETRY_WINDOW_MS - nowMs) / 1000);
  if (remaining <= 0) return null;
  const base = RETRY_DELAYS_S[n - 1] ?? MAX_RETRY_DELAY_S;
  return Math.max(1, Math.min(base, remaining));
}

/**
 * The full schedule from acceptance, assuming each attempt takes no time: offsets of each retry from
 * acceptance, in seconds. Used by tests and the docs.
 * @returns {number[]}
 */
export function retrySchedule() {
  const out = [];
  let t = 0;
  for (let n = 1; ; n += 1) {
    const d = retryDelaySeconds(n, 0, t * 1000);
    if (d === null) break;
    t += d;
    out.push(t);
  }
  return out;
}

/**
 * @typedef {{ v: 1, accepted_at: number, attempt: number, mid: string, payload: Record<string, unknown> }} Envelope
 */

/**
 * Serializes a queue envelope, refusing one over the queue size bound.
 * @param {Envelope} envelope
 * @returns {Envelope}
 */
export function checkEnvelope(envelope) {
  if (utf8Length(JSON.stringify(envelope)) > MAX_QUEUE_MESSAGE_BYTES) {
    throw new RangeError('queue message too large');
  }
  return envelope;
}

/**
 * Validates an envelope read back from the queue.
 * @param {unknown} body
 * @returns {Envelope | null}
 */
export function parseEnvelope(body) {
  if (!body || typeof body !== 'object') return null;
  const e = /** @type {Record<string, any>} */ (body);
  if (e.v !== 1) return null;
  if (!Number.isFinite(e.accepted_at) || !Number.isInteger(e.attempt) || e.attempt < 0) return null;
  if (typeof e.mid !== 'string' || !/^[0-9a-f]{12}$/.test(e.mid)) return null;
  if (!e.payload || typeof e.payload !== 'object') return null;
  if (utf8Length(JSON.stringify(e.payload)) > MAX_PAYLOAD_BYTES) return null;
  return /** @type {Envelope} */ (e);
}
