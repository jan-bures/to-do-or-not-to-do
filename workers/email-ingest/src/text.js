// Small string helpers shared by the other modules.

const encoder = new TextEncoder();

/** C0 and C1 controls except tab and LF, plus U+2028, U+2029 and U+FEFF. */
const CONTROL_RE = new RegExp(
  `[\x00-\x08\x0b-\x1f\x7f-\x9f${String.fromCharCode(0x2028, 0x2029, 0xfeff)}]`,
  'gu',
);

/**
 * Cuts a string to at most `max` Unicode code points.
 * @param {string} s
 * @param {number} max
 * @returns {string}
 */
export function capChars(s, max) {
  // Fast path: a string with at most `max` UTF-16 units has at most `max` code points.
  if (s.length <= max) return s;
  let out = '';
  let n = 0;
  for (const ch of s) {
    if (n === max) break;
    out += ch;
    n += 1;
  }
  return out;
}

/**
 * Number of Unicode code points in a string.
 * @param {string} s
 * @returns {number}
 */
export function charCount(s) {
  let n = 0;
  for (const _ of s) n += 1;
  return n;
}

/**
 * Byte length of a string encoded as UTF-8.
 * @param {string} s
 * @returns {number}
 */
export function utf8Length(s) {
  return encoder.encode(s).length;
}

/**
 * Cuts a string to at most `maxBytes` of UTF-8 without splitting a character.
 * @param {string} s
 * @param {number} maxBytes
 * @returns {{ text: string, truncated: boolean }}
 */
export function truncateUtf8(s, maxBytes) {
  const bytes = encoder.encode(s);
  if (bytes.length <= maxBytes) return { text: s, truncated: false };
  let end = Math.max(0, maxBytes);
  // Back off over continuation bytes (10xxxxxx) so the cut falls on a character boundary.
  while (end > 0 && (bytes[end] & 0xc0) === 0x80) end -= 1;
  return { text: new TextDecoder().decode(bytes.subarray(0, end)), truncated: true };
}

/**
 * Collapses runs of whitespace (including line breaks) to single spaces and trims.
 * @param {string} s
 * @returns {string}
 */
export function collapseWhitespace(s) {
  return s.replace(/\s+/gu, ' ').trim();
}

/**
 * Removes control characters except tab and line feed, and normalizes line endings to LF.
 * @param {string} s
 * @returns {string}
 */
export function stripControl(s) {
  return s
    .replace(/\r\n?/g, '\n')
    .replace(CONTROL_RE, '');
}

/**
 * Lowercase hex encoding.
 * @param {ArrayBuffer | Uint8Array} buf
 * @returns {string}
 */
export function toHex(buf) {
  const bytes = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  let out = '';
  for (const b of bytes) out += b.toString(16).padStart(2, '0');
  return out;
}

/**
 * Hex SHA-256 of a string or bytes.
 * @param {string | Uint8Array} data
 * @returns {Promise<string>}
 */
export async function sha256Hex(data) {
  const bytes = typeof data === 'string' ? encoder.encode(data) : data;
  return toHex(await crypto.subtle.digest('SHA-256', bytes));
}
