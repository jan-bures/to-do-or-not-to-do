// Body handling: HTML to text, the 64 KiB cap, and finding the original sender in a forwarded message.

import { isAddress, normalizeAddress } from './address.js';
import { MAX_BODY_BYTES, MAX_NAME_CHARS, MAX_SCAN_CHARS } from './limits.js';
import { capChars, collapseWhitespace, stripControl, truncateUtf8 } from './text.js';

/** @type {Record<string, string>} */
const ENTITIES = {
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
  nbsp: ' ',
  ndash: '–',
  mdash: '—',
  hellip: '…',
  lsquo: '‘',
  rsquo: '’',
  ldquo: '“',
  rdquo: '”',
  copy: '©',
  reg: '®',
  euro: '€',
  shy: '',
  zwnj: '',
  zwj: '',
};

/**
 * Decodes HTML character references: numeric ones and a few common named ones.
 * @param {string} s
 * @returns {string}
 */
function decodeEntities(s) {
  return s.replace(/&(#x[0-9a-f]{1,6}|#[0-9]{1,7}|[a-z]{2,8});/gi, (m, ref) => {
    if (ref[0] === '#') {
      const cp = ref[1] === 'x' || ref[1] === 'X' ? parseInt(ref.slice(2), 16) : parseInt(ref.slice(1), 10);
      if (!Number.isFinite(cp) || cp <= 0 || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return '';
      return String.fromCodePoint(cp);
    }
    const named = ENTITIES[ref.toLowerCase()];
    return named ?? m;
  });
}

/** Elements dropped with their content. */
const SKIP_TAGS = new Set(['head', 'style', 'script', 'title', 'template', 'svg', 'noscript']);

/** Elements that start or end a line. */
const BLOCK_TAGS = new Set([
  'p', 'div', 'tr', 'table', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'ul', 'ol', 'blockquote', 'section',
  'article', 'header', 'footer', 'pre', 'hr',
]);

/** The inside of a tag: everything up to `>` that isn't `<` or `>`. Sticky, so it never scans past a `<`. */
const TAG_BODY_RE = /([^<>]*)>/y;

/**
 * Strips HTML to readable plain text. Not a sanitizer: the result is only ever treated as text.
 * @param {string} html
 * @returns {string}
 */
export function htmlToText(html) {
  // A single left-to-right scan, so the cost stays linear even for malformed input (unclosed
  // comments, stray `<`, unclosed <style>), unlike lazy regular expressions.
  const lower = html.toLowerCase();
  const n = html.length;
  let out = '';
  let i = 0;
  while (i < n) {
    const lt = html.indexOf('<', i);
    if (lt < 0) {
      out += html.slice(i);
      break;
    }
    out += html.slice(i, lt);
    if (html.startsWith('<!--', lt)) {
      const end = html.indexOf('-->', lt + 4);
      i = end < 0 ? n : end + 3;
      continue;
    }
    // The tag runs to the next `>`, unless another `<` comes first: then this `<` is text.
    TAG_BODY_RE.lastIndex = lt + 1;
    const m = TAG_BODY_RE.exec(html);
    if (!m) {
      out += '<';
      i = lt + 1;
      continue;
    }
    const inner = m[1];
    const gt = lt + 1 + inner.length;
    i = gt + 1;
    const nameMatch = /^(\/?)([a-z][a-z0-9]*)/i.exec(inner);
    if (!nameMatch) {
      // `<!DOCTYPE …>`, `<?xml …?>` and the like are dropped; anything else is text.
      if (inner[0] !== '!' && inner[0] !== '?') out += html.slice(lt, i);
      continue;
    }
    const closing = nameMatch[1] === '/';
    const tag = nameMatch[2].toLowerCase();
    if (!closing && SKIP_TAGS.has(tag) && !inner.endsWith('/')) {
      const close = lower.indexOf(`</${tag}`, i);
      const closeEnd = close < 0 ? -1 : html.indexOf('>', close);
      i = closeEnd < 0 ? n : closeEnd + 1;
      continue;
    }
    if (tag === 'br') out += '\n';
    else if (tag === 'li' && !closing) out += '\n- ';
    else if ((tag === 'td' || tag === 'th') && closing) out += '\t';
    else if (BLOCK_TAGS.has(tag)) out += '\n';
  }
  let s = decodeEntities(out);
  s = stripControl(s)
    .split('\n')
    .map((line) => line.replace(/[ \t ]+/g, ' ').trim())
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
  return s;
}

/**
 * Picks the body to keep for `#keep`: text/plain if the message has it, otherwise its HTML stripped to
 * text, capped at `MAX_BODY_BYTES` of UTF-8.
 * @param {{ text?: string, html?: string }} email
 * @param {string} [htmlText] `htmlToText(email.html)`, if the caller already has it.
 * @returns {{ text: string, source: 'text' | 'html', truncated: boolean } | null}
 */
export function pickBody(email, htmlText) {
  /** @type {'text' | 'html'} */
  let source = 'text';
  let text = email.text && email.text.trim() ? stripControl(email.text).trim() : '';
  if (!text && email.html) {
    source = 'html';
    text = htmlText ?? htmlToText(email.html);
  }
  if (!text) return null;
  const cut = truncateUtf8(text, MAX_BODY_BYTES);
  return { text: cut.text, source, truncated: cut.truncated };
}

/**
 * Localized names of the "From" field in forwarded header blocks: English, German, Czech, Slovak,
 * French, Spanish, Italian, Dutch, Swedish, Danish/Norwegian and Polish.
 */
const FROM_LINE_RE =
  /^[ \t>*_]*(?:from|von|od|de|da|van|från|fra|nadawca)[ \t*_]*:[ \t*_]*(.+)$/gimu;

/** An address in angle brackets, an Outlook-style `[mailto:…]`, or a bare one. */
const ANGLE_RE = /<\s*(?:mailto:)?([^<>\s]+@[^<>\s]+)\s*>/i;
const MAILTO_RE = /\[\s*mailto:([^\]\s]+@[^\]\s]+)\s*\]/i;
const BARE_RE = /([^\s<>()[\]\\,;:"']+@[^\s<>()[\]\\,;:"']+)/;

/**
 * Finds the original sender in the first forwarded header block of a message's text, such as
 * `From: Alice Example <alice@example.com>` under "---------- Forwarded message ---------", or
 * Outlook's `From: Alice Example [mailto:alice@example.com]`. Returns null if no such line has an
 * address in it.
 * @param {string | undefined} text
 * @returns {{ address: string, name: string | null } | null}
 */
export function extractOriginalSender(text) {
  if (!text) return null;
  const scan = text.length > MAX_SCAN_CHARS ? text.slice(0, MAX_SCAN_CHARS) : text;
  FROM_LINE_RE.lastIndex = 0;
  for (const m of scan.matchAll(FROM_LINE_RE)) {
    const line = m[1].trim();
    const hit = ANGLE_RE.exec(line) ?? MAILTO_RE.exec(line) ?? BARE_RE.exec(line);
    if (!hit) continue;
    const address = normalizeAddress(hit[1].replace(/^mailto:/i, '').replace(/[.]+$/, ''));
    if (!isAddress(address)) continue;
    let name = line.slice(0, hit.index).replace(/["'*_]/g, '');
    name = collapseWhitespace(name);
    if (name.toLowerCase() === address) name = '';
    return { address, name: name ? capChars(name, MAX_NAME_CHARS) : null };
  }
  return null;
}
