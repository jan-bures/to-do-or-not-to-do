// Authentication-Results parsing (RFC 8601) and the exact sender alignment check.
//
// Cloudflare Email Routing checks SPF, DKIM and DMARC before it invokes the Worker, rejects mail that fails
// the sender domain's DMARC policy, and prepends its own Authentication-Results header with the
// authentication service identifier `mx.cloudflare.net`. Only that first header is trusted; others may
// come from the sender. See docs/ingest-email.md for the sources.

import { domainOf, normalizeAddress } from './address.js';

/** The authserv-id Cloudflare Email Routing writes. */
export const CLOUDFLARE_AUTHSERV_ID = 'mx.cloudflare.net';

/**
 * @typedef {{ method: string, result: string, props: Record<string, string> }} AuthResult
 * @typedef {{ authservId: string, results: AuthResult[] }} AuthResults
 */

/**
 * Splits a header value into top-level characters, dropping RFC 5322 comments `( … )` (which nest) and
 * keeping quoted strings intact, with their quotes removed. Returns segments split on top-level `;`.
 * A top-level `,` ends the value: `Headers.get()` joins repeated headers with ", ", and the
 * Authentication-Results grammar has no top-level commas.
 * @param {string} value
 * @returns {string[]}
 */
function segments(value) {
  /** @type {string[]} */
  const out = [];
  let cur = '';
  let depth = 0;
  let quoted = false;
  for (let i = 0; i < value.length; i += 1) {
    const c = value[i];
    if (quoted) {
      if (c === '\\' && i + 1 < value.length) {
        cur += value[i + 1];
        i += 1;
      } else if (c === '"') {
        quoted = false;
      } else {
        cur += c;
      }
      continue;
    }
    if (depth > 0) {
      if (c === '\\') i += 1;
      else if (c === '(') depth += 1;
      else if (c === ')') depth -= 1;
      continue;
    }
    if (c === '(') {
      depth = 1;
      cur += ' ';
    } else if (c === '"') {
      quoted = true;
    } else if (c === ';') {
      out.push(cur);
      cur = '';
    } else if (c === ',') {
      break;
    } else {
      cur += c;
    }
  }
  out.push(cur);
  return out.map((s) => s.replace(/\s+/g, ' ').trim());
}

/**
 * Parses one Authentication-Results header value. If `value` holds several headers joined by ", ", only
 * the first is parsed.
 * @param {string | null | undefined} value
 * @returns {AuthResults | null}
 */
export function parseAuthResults(value) {
  if (!value || value.length > 16 * 1024) return null;
  const segs = segments(value);
  const head = segs[0].split(' ');
  const authservId = (head[0] ?? '').toLowerCase();
  if (!authservId) return null;
  /** @type {AuthResult[]} */
  const results = [];
  for (const seg of segs.slice(1)) {
    if (!seg) continue;
    const tokens = seg.split(' ');
    const m = /^([a-z0-9_-]+)(?:\/\d+)?=([a-z0-9_-]+)$/i.exec(tokens[0] ?? '');
    if (!m) continue;
    /** @type {Record<string, string>} */
    const props = {};
    for (const t of tokens.slice(1)) {
      const eq = t.indexOf('=');
      if (eq <= 0) continue;
      const key = t.slice(0, eq).toLowerCase();
      if (!(key in props)) props[key] = t.slice(eq + 1);
    }
    results.push({ method: m[1].toLowerCase(), result: m[2].toLowerCase(), props });
  }
  return { authservId, results };
}

/**
 * Whether an authenticated domain is aligned with the From domain. Alignment is exact: the two
 * domains must be equal, ignoring case and a trailing dot. Relaxed (subdomain) alignment is not
 * accepted, because a sender authorized for a delegated subdomain would then pass for the parent.
 * @param {string} authDomain
 * @param {string} fromDomain
 * @returns {boolean}
 */
export function aligned(authDomain, fromDomain) {
  const a = authDomain.toLowerCase().replace(/\.$/, '');
  const f = fromDomain.toLowerCase().replace(/\.$/, '');
  return a.includes('.') && a === f;
}

/**
 * The domain of `smtp.mailfrom`, which may be an address or a bare domain.
 * @param {string | undefined} v
 * @returns {string}
 */
function mailfromDomain(v) {
  if (!v) return '';
  const s = normalizeAddress(v);
  return s.includes('@') ? domainOf(s) : s;
}

/**
 * @typedef {{ ok: true, via: 'dkim' | 'spf' }
 *   | { ok: false, reason: 'auth_missing' | 'auth_untrusted' | 'auth_failed' | 'auth_unaligned' }} AuthVerdict
 */

/**
 * Requires a passing DKIM or SPF result, for exactly the From domain, in the trusted
 * Authentication-Results header. DKIM is checked first.
 *
 * - DKIM: `dkim=pass` and `header.d` equal to the From domain.
 * - SPF: `spf=pass` and the domain of `smtp.mailfrom` equal to the From domain.
 *
 * `dmarc=pass` alone isn't accepted: DMARC's relaxed alignment lets a subdomain's pass count for its
 * parent. A pass for any other domain (such as a spoofed From with another domain's valid SPF) fails.
 * @param {string | null | undefined} headerValue The first Authentication-Results header.
 * @param {string} fromDomain
 * @param {string} [trustedId]
 * @returns {AuthVerdict}
 */
export function checkAuth(headerValue, fromDomain, trustedId = CLOUDFLARE_AUTHSERV_ID) {
  const ar = parseAuthResults(headerValue);
  if (!ar) return { ok: false, reason: 'auth_missing' };
  if (ar.authservId !== trustedId.toLowerCase()) return { ok: false, reason: 'auth_untrusted' };
  const passes = ar.results.filter((r) => r.result === 'pass');
  for (const r of passes) {
    if (r.method === 'dkim' && aligned(r.props['header.d'] ?? '', fromDomain)) return { ok: true, via: 'dkim' };
  }
  for (const r of passes) {
    if (r.method === 'spf' && aligned(mailfromDomain(r.props['smtp.mailfrom']), fromDomain)) return { ok: true, via: 'spf' };
  }
  const relevant = passes.some((r) => r.method === 'dkim' || r.method === 'spf');
  return { ok: false, reason: relevant ? 'auth_unaligned' : 'auth_failed' };
}

/**
 * The shape of the trusted Authentication-Results header as booleans only, for the
 * `DEBUG_AUTH_SHAPE` deploy check. It never returns a domain, an address or any header text.
 * @param {string | null | undefined} headerValue
 * @param {string} fromDomain
 * @param {string} [trustedId]
 * @returns {Record<string, boolean>}
 */
export function authShape(headerValue, fromDomain, trustedId = CLOUDFLARE_AUTHSERV_ID) {
  const ar = parseAuthResults(headerValue);
  const results = ar?.results ?? [];
  const dkim = results.filter((r) => r.method === 'dkim' && r.result === 'pass');
  const spf = results.filter((r) => r.method === 'spf' && r.result === 'pass');
  return {
    ar_present: ar !== null,
    authserv_ok: ar !== null && ar.authservId === trustedId.toLowerCase(),
    dkim_pass: dkim.length > 0,
    header_d_present: dkim.some((r) => Boolean(r.props['header.d'])),
    header_d_aligned: dkim.some((r) => aligned(r.props['header.d'] ?? '', fromDomain)),
    spf_pass: spf.length > 0,
    mailfrom_present: spf.some((r) => Boolean(r.props['smtp.mailfrom'])),
    mailfrom_aligned: spf.some((r) => aligned(mailfromDomain(r.props['smtp.mailfrom']), fromDomain)),
    dmarc_pass: results.some((r) => r.method === 'dmarc' && r.result === 'pass'),
  };
}
