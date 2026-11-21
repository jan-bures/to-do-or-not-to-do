// Request signing. The contract is in docs/ingest-email.md; the shared test vector is
// docs/ingest-email-signature.json.

import { toHex } from './text.js';

const encoder = new TextEncoder();

/**
 * A fresh 128-bit nonce as 32 lowercase hex characters.
 * @returns {string}
 */
export function randomNonce() {
  return toHex(crypto.getRandomValues(new Uint8Array(16)));
}

/**
 * The bytes that are signed: `timestamp + "\n" + nonce + "\n" + body`.
 * @param {string} timestamp Unix seconds, in decimal.
 * @param {string} nonce
 * @param {Uint8Array} body The exact request body bytes.
 * @returns {Uint8Array}
 */
export function signingInput(timestamp, nonce, body) {
  const prefix = encoder.encode(`${timestamp}\n${nonce}\n`);
  const out = new Uint8Array(prefix.length + body.length);
  out.set(prefix, 0);
  out.set(body, prefix.length);
  return out;
}

/**
 * Hex HMAC-SHA256 over the signing input. The key is the UTF-8 bytes of the secret string as configured
 * (it is not hex- or base64-decoded first).
 * @param {string} secret
 * @param {string} timestamp
 * @param {string} nonce
 * @param {Uint8Array} body
 * @returns {Promise<string>}
 */
export async function sign(secret, timestamp, nonce, body) {
  const key = await crypto.subtle.importKey(
    'raw',
    encoder.encode(secret),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  );
  return toHex(await crypto.subtle.sign('HMAC', key, signingInput(timestamp, nonce, body)));
}

/**
 * Builds the signed request headers for a body.
 * @param {string} secret
 * @param {Uint8Array} body
 * @param {{ nowMs: number, nonce: string }} opts
 * @returns {Promise<Record<string, string>>}
 */
export async function signedHeaders(secret, body, { nowMs, nonce }) {
  const timestamp = String(Math.floor(nowMs / 1000));
  return {
    'content-type': 'application/json',
    'x-todo-timestamp': timestamp,
    'x-todo-nonce': nonce,
    'x-todo-signature': await sign(secret, timestamp, nonce, body),
  };
}
