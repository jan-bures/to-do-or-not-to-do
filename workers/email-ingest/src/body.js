// Body handling: HTML to text, the 64 KiB cap, and finding the original sender in a forwarded message.

import { isAddress, normalizeAddress } from './address.js';
import { MAX_BODY_BYTES, MAX_LINE_CHARS, MAX_NAME_CHARS, MAX_SCAN_CHARS, MAX_SCAN_LINES, MAX_TAG_CHARS } from './limits.js';
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

/**
 * Scans a tag body starting just after its `<`. Quote-aware: a `>` inside an attribute value quoted
 * after `=` doesn't end the tag. Stops at an unquoted `<`, or after `MAX_TAG_CHARS`.
 * @param {string} html
 * @param {number} start
 * @returns {{ gt: number } | { stray: number }} The index of the closing `>`, or where scanning stopped
 *   when this isn't a tag.
 */
function scanTag(html, start) {
  const limit = Math.min(html.length, start + MAX_TAG_CHARS);
  let quote = 0;
  let lastNonSpace = 0;
  for (let j = start; j < limit; j += 1) {
    const c = html.charCodeAt(j);
    if (quote) {
      if (c === quote) {
        quote = 0;
        lastNonSpace = c;
      }
      continue;
    }
    if ((c === 0x22 || c === 0x27) && lastNonSpace === 0x3d) {
      quote = c; // " or ' right after =
    } else if (c === 0x3e) {
      return { gt: j };
    } else if (c === 0x3c) {
      return { stray: j };
    }
    if (c !== 0x20 && c !== 0x09 && c !== 0x0a && c !== 0x0d) lastNonSpace = c;
  }
  return { stray: limit };
}

/**
 * Strips HTML to readable plain text. Not a sanitizer: the result is only ever treated as text.
 *
 * A single left-to-right scan in which every character is looked at a bounded number of times, so the
 * cost stays linear for malformed input too (unclosed comments, stray `<`, unclosed quotes or
 * <style>), unlike lazy regular expressions.
 * @param {string} html
 * @returns {string}
 */
export function htmlToText(html) {
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
    const scanned = scanTag(html, lt + 1);
    if ('stray' in scanned) {
      // Not a tag: keep what was scanned as text and carry on after it, so nothing is rescanned.
      out += html.slice(lt, scanned.stray);
      i = scanned.stray;
      continue;
    }
    const inner = html.slice(lt + 1, scanned.gt);
    i = scanned.gt + 1;
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

/** Header labels in forwarded blocks, by field. English, German, Czech, Slovak, French, Spanish,
 * Italian, Dutch, Swedish, Danish/Norwegian and Polish. */
const LABELS = new Map([
  ...['from', 'von', 'od', 'de', 'da', 'van', 'från', 'fra', 'nadawca'].map((l) => [l, 'from']),
  ...['sent', 'date', 'datum', 'odesláno', 'odoslané', 'gesendet', 'envoyé', 'enviado', 'fecha', 'inviato', 'data',
    'verzonden', 'skickat', 'sendt', 'wysłano'].map((l) => [l, 'date']),
  ...['to', 'an', 'komu', 'à', 'a', 'para', 'aan', 'till', 'til', 'do'].map((l) => [l, 'to']),
  ...['cc', 'kopie', 'kopia', 'copie'].map((l) => [l, 'cc']),
  ...['subject', 'betreff', 'předmět', 'predmet', 'objet', 'asunto', 'oggetto', 'onderwerp', 'ämne', 'emne',
    'temat'].map((l) => [l, 'subject']),
]);

/** Separator lines that open a forwarded block, after quote markers and spaces are trimmed. */
// The dashed form ("-----Original Message-----", "---------- Forwarded message ---------") is checked by
// dashedSeparator() instead: as a regex, /^-{5,}[^-]{0,60}-{2,}$/ backtracks quadratically on a long
// run of dashes followed by another character.
const SEPARATOR_RES = [
  /^_{10,}$/, // Outlook's rule above the header block
  /^(?:begin forwarded message|anfang der weitergeleiteten nachricht|začátek přeposlané zprávy)\s*:?$/i,
];

/** Characters a bare address may not contain. */
const NOT_ADDRESS_CHARS = new Set([...' \t<>()[]\\,;:"\'']);

/**
 * Splits a header-like line into its field kind and value, without regular expressions:
 * optional quote markers and emphasis (`>`, `*`, `_`), a label of at most 24 characters, `:`.
 * @param {string} line
 * @returns {{ kind: string, value: string } | null}
 */
function headerLine(line) {
  let i = 0;
  while (i < line.length && ' \t>*_'.includes(line[i])) i += 1;
  const colon = line.indexOf(':', i);
  if (colon < 0 || colon - i > 24) return null;
  let end = colon;
  while (end > i && ' \t*_'.includes(line[end - 1])) end -= 1;
  const kind = LABELS.get(line.slice(i, end).toLowerCase());
  if (!kind) return null;
  let v = colon + 1;
  while (v < line.length && ' \t*_'.includes(line[v])) v += 1;
  return { kind, value: line.slice(v) };
}

/**
 * @param {string} line
 * @returns {boolean}
 */
function isSeparator(line) {
  let t = line.trim();
  while (t.startsWith('>')) t = t.slice(1).trimStart();
  return t.length >= 5 && (dashedSeparator(t) || SEPARATOR_RES.some((re) => re.test(t)));
}

/**
 * At least five dashes, at most 60 non-dash characters, then at least two dashes. One linear pass.
 * @param {string} t
 * @returns {boolean}
 */
export function dashedSeparator(t) {
  let lead = 0;
  while (lead < t.length && t[lead] === '-') lead += 1;
  if (lead < 5) return false;
  if (lead === t.length) return t.length >= 7;
  let trail = 0;
  while (trail < t.length - lead && t[t.length - 1 - trail] === '-') trail += 1;
  if (trail < 2) return false;
  const middle = t.length - lead - trail;
  return middle <= 60 && !t.slice(lead, t.length - trail).includes('-');
}

/**
 * Extracts a mailbox from a header value: `Name <addr>`, Outlook's `Name [mailto:addr]`, or a bare
 * address. Linear: it finds the brackets or the `@` with indexOf and expands over allowed characters.
 * @param {string} v
 * @returns {{ address: string, name: string | null } | null}
 */
function mailboxFrom(v) {
  let cand = '';
  let nameEnd = 0;
  const lt = v.indexOf('<');
  const gt = lt < 0 ? -1 : v.indexOf('>', lt);
  const mailto = v.toLowerCase().indexOf('[mailto:');
  if (gt > lt && lt >= 0) {
    cand = v.slice(lt + 1, gt).trim().replace(/^mailto:/i, '');
    nameEnd = lt;
  } else if (mailto >= 0) {
    const close = v.indexOf(']', mailto);
    cand = v.slice(mailto + 8, close < 0 ? v.length : close).trim();
    nameEnd = mailto;
  } else {
    const at = v.indexOf('@');
    if (at < 0) return null;
    let l = at;
    while (l > 0 && !NOT_ADDRESS_CHARS.has(v[l - 1])) l -= 1;
    let r = at + 1;
    while (r < v.length && !NOT_ADDRESS_CHARS.has(v[r])) r += 1;
    cand = v.slice(l, r);
    nameEnd = l;
  }
  while (cand.endsWith('.')) cand = cand.slice(0, -1);
  const address = normalizeAddress(cand);
  if (!isAddress(address)) return null;
  let name = '';
  for (const ch of v.slice(0, nameEnd)) if (!'"\'*_'.includes(ch)) name += ch;
  name = collapseWhitespace(name);
  if (name.toLowerCase() === address) name = '';
  return { address, name: name ? capChars(name, MAX_NAME_CHARS) : null };
}

/**
 * Finds the original sender in the first forwarded header block of a message's text, such as
 * `From: Alice Example <alice@example.com>` under "---------- Forwarded message ---------", or
 * Outlook's `From: Alice Example [mailto:alice@example.com]` followed by `Sent:`.
 *
 * A `From:` line counts only with evidence of a header block: a separator line among the three
 * non-blank lines before it, or a neighbouring `Sent`/`Date`/`To`/`Cc`/`Subject` line (or a
 * localized label). Only the first `MAX_SCAN_CHARS` characters and `MAX_SCAN_LINES` lines are
 * looked at, and lines longer than `MAX_LINE_CHARS` (the RFC 5322 limit) are skipped. Everything is
 * linear in the input.
 * @param {string | undefined} text
 * @returns {{ address: string, name: string | null } | null}
 */
export function extractOriginalSender(text) {
  if (!text) return null;
  const lines = (text.length > MAX_SCAN_CHARS ? text.slice(0, MAX_SCAN_CHARS) : text).split('\n', MAX_SCAN_LINES);
  const kindAt = (/** @type {number} */ k) => {
    const l = lines[k];
    if (l === undefined || l.length > MAX_LINE_CHARS) return null;
    return headerLine(l.replace(/\r$/, ''))?.kind ?? null;
  };
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i].replace(/\r$/, '');
    if (line.length > MAX_LINE_CHARS) continue;
    const h = headerLine(line);
    if (!h || h.kind !== 'from') continue;
    const neighbour = [kindAt(i - 1), kindAt(i + 1)].some((k) => k !== null && k !== 'from');
    let separated = false;
    for (let k = i - 1, seen = 0; k >= 0 && seen < 3; k -= 1) {
      if (!lines[k].trim()) continue;
      seen += 1;
      if (lines[k].length <= MAX_LINE_CHARS && isSeparator(lines[k])) {
        separated = true;
        break;
      }
    }
    if (!neighbour && !separated) continue;
    const box = mailboxFrom(h.value);
    if (box) return box;
  }
  return null;
}
