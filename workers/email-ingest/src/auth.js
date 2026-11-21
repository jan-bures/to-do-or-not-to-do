// Authentication-Results parsing (RFC 8601) and the sender alignment check.
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
 * Whether an authenticated domain is aligned with the From domain: equal, or one a subdomain of the
 * other (DMARC relaxed alignment, without the public suffix list). Both must contain a dot.
 * @param {string} authDomain
 * @param {string} fromDomain
 * @returns {boolean}
 */
export function aligned(authDomain, fromDomain) {
  const a = authDomain.toLowerCase().replace(/\.$/, '');
  const f = fromDomain.toLowerCase().replace(/\.$/, '');
  if (!a.includes('.') || !f.includes('.')) return false;
  return a === f || a.endsWith(`.${f}`) || f.endsWith(`.${a}`);
}

/**
 * The domain a property names: `smtp.mailfrom` may be an address or a bare domain; `header.i` is an
 * address-like identity.
 * @param {string | undefined} v
 * @returns {string}
 */
function propDomain(v) {
  if (!v) return '';
  const s = normalizeAddress(v);
  return s.includes('@') ? domainOf(s) : s;
}

/**
 * @typedef {{ ok: true, via: 'dkim' | 'spf' | 'dmarc' }
 *   | { ok: false, reason: 'auth_missing' | 'auth_untrusted' | 'auth_failed' | 'auth_unaligned' }} AuthVerdict
 */

/**
 * Requires a passing, aligned DKIM, SPF or DMARC result in the trusted Authentication-Results header.
 *
 * - DKIM passes if `dkim=pass` and `header.d` (or the domain of `header.i`) is aligned with the From domain.
 * - SPF passes if `spf=pass` and the domain of `smtp.mailfrom` is aligned with the From domain.
 * - DMARC passes if `dmarc=pass` and `header.from` is aligned with the From domain. DMARC itself
 *   only passes when SPF or DKIM passed in alignment with that domain.
 *
 * A pass whose domain isn't aligned (such as a spoofed From with another domain's valid SPF) fails.
 * @param {string | null | undefined} headerValue The first Authentication-Results header.
 * @param {string} fromDomain
 * @param {string} [trustedId]
 * @returns {AuthVerdict}
 */
export function checkAuth(headerValue, fromDomain, trustedId = CLOUDFLARE_AUTHSERV_ID) {
  const ar = parseAuthResults(headerValue);
  if (!ar) return { ok: false, reason: 'auth_missing' };
  if (ar.authservId !== trustedId.toLowerCase()) return { ok: false, reason: 'auth_untrusted' };
  let anyPass = false;
  for (const r of ar.results) {
    if (r.result !== 'pass') continue;
    anyPass = true;
    if (r.method === 'dkim') {
      const d = r.props['header.d'] || propDomain(r.props['header.i']);
      if (aligned(d, fromDomain)) return { ok: true, via: 'dkim' };
    } else if (r.method === 'spf') {
      if (aligned(propDomain(r.props['smtp.mailfrom']), fromDomain)) return { ok: true, via: 'spf' };
    } else if (r.method === 'dmarc') {
      if (aligned(propDomain(r.props['header.from']), fromDomain)) return { ok: true, via: 'dmarc' };
    }
  }
  return { ok: false, reason: anyPass ? 'auth_unaligned' : 'auth_failed' };
}
