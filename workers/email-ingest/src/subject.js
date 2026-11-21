// Subject cleanup: forward and reply prefixes, the `#keep` marker, and length caps.

import { MAX_SUBJECT_CHARS, MAX_TITLE_CHARS } from './limits.js';
import { capChars, collapseWhitespace, stripControl } from './text.js';

/**
 * One forward or reply prefix: `FW:`, `Fwd:`, `WG:` (German "weitergeleitet"), `Re:` and `AW:` (German
 * "Antwort"), case-insensitive, optionally numbered as in `Re[2]:` or `Re(2):`, and with a full-width
 * colon too.
 */
const PREFIX_RE = /^(?:fwd?|wg|re|aw)\s*(?:\[\d+\]|\(\d+\))?\s*[:：]\s*/iu;

/**
 * `#keep` as a whole whitespace-separated token, case-insensitive, optionally followed by sentence
 * punctuation (`#keep.`, `#keep!`). `#keeper` and `x#keep` don't count.
 */
const KEEP_TOKEN_RE = /^#keep[.,;:!?)\]]*$/i;

/** Title used when nothing is left of the subject. */
export const EMPTY_TITLE = '(no subject)';

/**
 * Strips every leading forward or reply prefix.
 * @param {string} s
 * @returns {string}
 */
function stripPrefixes(s) {
  let cur = s.trimStart();
  for (;;) {
    const next = cur.replace(PREFIX_RE, '');
    if (next === cur) return cur;
    cur = next;
  }
}

/**
 * @typedef {{ original: string, title: string, keep: boolean }} CleanSubject
 */

/**
 * Cleans a decoded subject.
 *
 * The `#keep` decision is made on the whole subject, after forward prefixes are stripped
 * (`Fwd:#keep` counts), and before anything is truncated, so a cut can never turn a longer hashtag
 * into `#keep`. postal-mime bounds the input (all headers together are at most 256 KiB).
 * @param {string | null | undefined} subject
 * @returns {CleanSubject}
 */
export function cleanSubject(subject) {
  const full = collapseWhitespace(stripControl(subject ?? ''));
  let keep = false;
  // Prefixes can sit on either side of the marker ("Fwd: #keep Re: x"), so strip, drop the marker
  // tokens, and strip again.
  const tokens = stripPrefixes(full)
    .split(' ')
    .filter((t) => {
      if (KEEP_TOKEN_RE.test(t)) {
        keep = true;
        return false;
      }
      return t !== '';
    });
  const title = collapseWhitespace(stripPrefixes(tokens.join(' ')));
  return {
    original: capChars(full, MAX_SUBJECT_CHARS),
    title: capChars(title || EMPTY_TITLE, MAX_TITLE_CHARS),
    keep,
  };
}
