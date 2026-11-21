// Subject cleanup: forward and reply prefixes, the `#keep` marker, and length caps.

import { MAX_SUBJECT_CHARS, MAX_TITLE_CHARS } from './limits.js';
import { capChars, collapseWhitespace, stripControl } from './text.js';

/**
 * One forward or reply prefix: `FW:`, `Fwd:`, `WG:` (German "weitergeleitet"), `Re:` and `AW:` (German
 * "Antwort"), case-insensitive, optionally numbered as in `Re[2]:` or `Re(2):`, and with a full-width
 * colon too.
 */
const PREFIX_RE = /^(?:fwd?|wg|re|aw)\s*(?:\[\d+\]|\(\d+\))?\s*[:：]\s*/iu;

/** `#keep` as a word of its own, case-insensitive. */
const KEEP_RE = /(^|\s)#keep(?=$|[\s.,;:!?)\]])/giu;

/** Title used when nothing is left of the subject. */
export const EMPTY_TITLE = '(no subject)';

/**
 * @typedef {{ original: string, title: string, keep: boolean }} CleanSubject
 */

/**
 * Cleans a decoded subject.
 * @param {string | null | undefined} subject
 * @returns {CleanSubject}
 */
export function cleanSubject(subject) {
  const original = capChars(collapseWhitespace(stripControl(subject ?? '')), MAX_SUBJECT_CHARS);
  let keep = false;
  let s = original.replace(KEEP_RE, (_m, pre) => {
    keep = true;
    return pre;
  });
  s = collapseWhitespace(s);
  for (;;) {
    const next = s.replace(PREFIX_RE, '');
    if (next === s) break;
    s = next;
  }
  s = collapseWhitespace(s);
  return { original, title: capChars(s || EMPTY_TITLE, MAX_TITLE_CHARS), keep };
}
