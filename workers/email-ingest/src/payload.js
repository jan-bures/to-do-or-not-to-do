// Building the request payload (schema version 1) from an accepted message.

import PostalMime, { decodeWords } from 'postal-mime';
import { extractOriginalSender, htmlToText, pickBody } from './body.js';
import { MAX_MESSAGE_ID_CHARS, MAX_NAME_CHARS, MAX_PAYLOAD_BYTES, MAX_RAW_BYTES, PARSER_LIMITS } from './limits.js';
import { cleanSubject } from './subject.js';
import { capChars, collapseWhitespace, sha256Hex, truncateUtf8, utf8Length } from './text.js';

/**
 * Reads at most `max` bytes of a stream and cancels the rest.
 * @param {ReadableStream<Uint8Array>} stream
 * @param {number} max
 * @returns {Promise<{ bytes: Uint8Array, truncated: boolean }>}
 */
export async function readCapped(stream, max = MAX_RAW_BYTES) {
  const reader = stream.getReader();
  /** @type {Uint8Array[]} */
  const chunks = [];
  let total = 0;
  let truncated = false;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    const room = max - total;
    if (value.length >= room) {
      chunks.push(value.subarray(0, room));
      total += room;
      // Exactly `max` bytes with nothing after them isn't truncation, but finding out would mean
      // reading on; treat it as truncated.
      truncated = true;
      break;
    }
    chunks.push(value);
    total += value.length;
  }
  if (truncated) {
    try {
      await reader.cancel();
    } catch {
      // The stream may already be closed.
    }
  }
  reader.releaseLock();
  const bytes = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    bytes.set(c, off);
    off += c.length;
  }
  return { bytes, truncated };
}

/**
 * Whether the bytes contain the blank line that ends the top-level header section.
 * @param {Uint8Array} raw
 * @returns {boolean}
 */
export function hasHeaderEnd(raw) {
  for (let i = 0; i + 1 < raw.length; i += 1) {
    if (raw[i] !== 0x0a) continue;
    if (raw[i + 1] === 0x0a) return true;
    if (raw[i + 1] === 0x0d && raw[i + 2] === 0x0a) return true;
  }
  return false;
}

/**
 * Parses MIME with tight limits. Attachments are returned as small base64 strings we never read; their
 * content is dropped right away.
 * @param {Uint8Array} raw
 * @returns {Promise<import('postal-mime').Email>}
 */
export async function parseMime(raw) {
  const email = await PostalMime.parse(raw, { ...PARSER_LIMITS, attachmentEncoding: 'arraybuffer' });
  email.attachments = [];
  return email;
}

/**
 * A stand-in for a parsed email when postal-mime refuses the message (over a limit, or malformed):
 * only the decoded subject, from the header Email Routing already parsed. No body is used.
 * @param {Headers} headers
 * @returns {import('postal-mime').Email}
 */
export function headersOnly(headers) {
  let subject = headers.get('subject') ?? '';
  try {
    subject = decodeWords(subject);
  } catch {
    // Keep the raw value.
  }
  return { headers: [], headerLines: [], attachments: [], subject };
}

/**
 * A usable Message-ID from the header, or a deterministic one derived from the raw bytes, so that a
 * redelivery of the same message still dedupes.
 * @param {string | null | undefined} header
 * @param {Uint8Array} raw
 * @returns {Promise<string>}
 */
export async function messageIdFor(header, raw) {
  const id = collapseWhitespace(header ?? '');
  if (/^<[^<>\s]+>$/.test(id) && id.length <= MAX_MESSAGE_ID_CHARS) return id;
  return `<sha256-${await sha256Hex(raw)}@email-ingest.invalid>`;
}

/**
 * @typedef {object} PayloadInput
 * @property {string} messageId
 * @property {string | null} list
 * @property {{ address: string, name: string }} from The forward's From mailbox (already allowlisted).
 * @property {import('postal-mime').Email} email
 * @property {number} receivedAtMs
 * @property {boolean} rawTruncated
 * @property {boolean} [parseFailed]
 */

/**
 * Builds the payload object. Field meanings are documented in docs/ingest-email.md.
 * @param {PayloadInput} input
 * @returns {Record<string, unknown>}
 */
export function buildPayload({ messageId, list, from, email, receivedAtMs, rawTruncated, parseFailed = false }) {
  const subject = cleanSubject(email.subject);
  const hasText = Boolean(email.text && email.text.trim());
  const htmlText = !hasText && email.html ? htmlToText(email.html) : undefined;
  const forwarded = extractOriginalSender(hasText ? email.text : htmlText);
  const original = forwarded
    ? { address: forwarded.address, name: forwarded.name, source: 'forwarded_header' }
    : { address: from.address, name: from.name ? capChars(from.name, MAX_NAME_CHARS) : null, source: 'from' };
  const body = subject.keep ? pickBody(email, htmlText) : null;
  return {
    version: 1,
    message_id: messageId,
    list,
    title: subject.title,
    subject: subject.original,
    keep: subject.keep,
    original_sender: original,
    forwarded_by: from.address,
    received_at: new Date(receivedAtMs).toISOString(),
    body,
    raw_truncated: rawTruncated,
    parse_failed: parseFailed,
  };
}

/**
 * Serializes a payload, shortening the body until the result fits `MAX_PAYLOAD_BYTES`.
 * @param {Record<string, any>} payload
 * @returns {{ payload: Record<string, any>, json: string }}
 */
export function fitPayload(payload) {
  let json = JSON.stringify(payload);
  for (let i = 0; i < 8; i += 1) {
    const size = utf8Length(json);
    if (size <= MAX_PAYLOAD_BYTES) return { payload, json };
    const body = payload.body;
    if (!body) break;
    // Scale the body by the share of the serialized size it may take. Escaping makes the ratio uneven
    // along the text, so loop until it fits.
    const textBytes = utf8Length(body.text);
    const textJsonBytes = utf8Length(JSON.stringify(body.text));
    const room = MAX_PAYLOAD_BYTES - (size - textJsonBytes) - 64;
    const target = Math.max(0, Math.floor((textBytes * room) / textJsonBytes));
    const cut = truncateUtf8(body.text, target);
    payload = { ...payload, body: cut.text ? { ...body, text: cut.text, truncated: true } : null };
    json = JSON.stringify(payload);
  }
  if (utf8Length(json) > MAX_PAYLOAD_BYTES) throw new RangeError('payload too large');
  return { payload, json };
}
