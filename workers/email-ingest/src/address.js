// Address handling: the allowlist, the recipient's plus address, and the sender's From header.

import { addressParser } from 'postal-mime';
import { MAX_ADDRESS_CHARS } from './limits.js';

/** A conservative shape check. Real validation happened at SMTP time; this keeps junk out of the payload. */
const ADDRESS_RE = /^[^\s@<>()[\]\\,;:"]+@[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+$/i;

/** Allowed list keys after `+`: lowercase letters, digits, `-` and `_`, at most 64 characters. */
const LIST_KEY_RE = /^[a-z0-9][a-z0-9_-]{0,63}$/;

/**
 * Lowercases and trims an address, and strips surrounding angle brackets.
 * @param {string} s
 * @returns {string}
 */
export function normalizeAddress(s) {
  return s.trim().replace(/^<(.*)>$/s, '$1').trim().toLowerCase();
}

/**
 * Whether a string looks like a single plain email address.
 * @param {string} s
 * @returns {boolean}
 */
export function isAddress(s) {
  return s.length <= MAX_ADDRESS_CHARS && ADDRESS_RE.test(s);
}

/**
 * The domain part of an address, lowercased.
 * @param {string} address
 * @returns {string}
 */
export function domainOf(address) {
  const at = address.lastIndexOf('@');
  return at < 0 ? '' : address.slice(at + 1).toLowerCase();
}

/**
 * Parses `ALLOWED_SENDERS`: addresses separated by commas, semicolons or whitespace.
 * Entries that aren't addresses are ignored.
 * @param {string | undefined} value
 * @returns {Set<string>}
 */
export function parseAllowlist(value) {
  const out = new Set();
  for (const part of (value ?? '').split(/[\s,;]+/)) {
    const a = normalizeAddress(part);
    if (a && isAddress(a)) out.add(a);
  }
  return out;
}

/**
 * @typedef {{ ok: true, list: string | null } | { ok: false, reason: 'wrong_recipient' | 'bad_list' }} RecipientResult
 */

/**
 * Reads the envelope recipient. `todo@…` carries no list; `todo+shopping@…` carries `shopping`.
 * The domain isn't checked here: Email Routing only invokes the Worker for its own rules.
 * @param {string} to Envelope recipient (RCPT TO).
 * @param {string} localPart Expected local part before any `+`, such as `todo`.
 * @returns {RecipientResult}
 */
export function parseRecipient(to, localPart) {
  const addr = normalizeAddress(to ?? '');
  const at = addr.lastIndexOf('@');
  if (at <= 0) return { ok: false, reason: 'wrong_recipient' };
  const local = addr.slice(0, at);
  const plus = local.indexOf('+');
  const base = plus < 0 ? local : local.slice(0, plus);
  if (base !== localPart.toLowerCase()) return { ok: false, reason: 'wrong_recipient' };
  if (plus < 0) return { ok: true, list: null };
  const detail = local.slice(plus + 1);
  if (detail === '') return { ok: true, list: null };
  if (!LIST_KEY_RE.test(detail)) return { ok: false, reason: 'bad_list' };
  return { ok: true, list: detail };
}

/**
 * Parses a From header value that must hold exactly one mailbox.
 * Headers.get() joins repeated headers with ", ", so two From headers also fail here.
 * @param {string | null | undefined} value
 * @returns {{ address: string, name: string } | null}
 */
export function parseSingleMailbox(value) {
  if (!value || value.length > 4 * MAX_ADDRESS_CHARS) return null;
  let parsed;
  try {
    parsed = addressParser(value, { flatten: true });
  } catch {
    return null;
  }
  if (!Array.isArray(parsed) || parsed.length !== 1) return null;
  const box = parsed[0];
  const address = normalizeAddress(box.address ?? '');
  if (!isAddress(address)) return null;
  return { address, name: (box.name ?? '').trim() };
}
