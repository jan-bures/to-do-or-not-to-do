import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { aligned, checkAuth, parseAuthResults } from '../src/auth.js';

// A realistic Email Routing header, with comments, as Cloudflare prepends it.
const REAL =
  'mx.cloudflare.net; dkim=pass header.d=example.com header.s=sel1 header.b=AbCd; ' +
  'dmarc=pass header.from=example.com policy.dmarc=reject; ' +
  'spf=pass (mx.cloudflare.net: domain of you@example.com designates 192.0.2.1 as permitted sender, ok) ' +
  'smtp.mailfrom=you@example.com; arc=none smtp.remote-ip=192.0.2.1';

describe('parseAuthResults', () => {
  it('parses methods, results and properties, ignoring comments', () => {
    const ar = parseAuthResults(REAL);
    assert.equal(ar?.authservId, 'mx.cloudflare.net');
    assert.deepEqual(
      ar?.results.map((r) => [r.method, r.result]),
      [
        ['dkim', 'pass'],
        ['dmarc', 'pass'],
        ['spf', 'pass'],
        ['arc', 'none'],
      ],
    );
    assert.equal(ar?.results[2].props['smtp.mailfrom'], 'you@example.com');
    assert.equal(ar?.results[0].props['header.d'], 'example.com');
  });

  it('reads only the first of several headers joined by Headers.get()', () => {
    const joined = 'mx.cloudflare.net; spf=fail smtp.mailfrom=x@evil.example, evil.example; dkim=pass header.d=example.com';
    const ar = parseAuthResults(joined);
    assert.equal(ar?.authservId, 'mx.cloudflare.net');
    assert.deepEqual(ar?.results.map((r) => r.method), ['spf']);
  });

  it('keeps commas and semicolons inside comments and quoted strings', () => {
    const ar = parseAuthResults('mx.cloudflare.net; dkim=pass (a; b, c) reason="x; y, z" header.d=example.com');
    assert.equal(ar?.results.length, 1);
    assert.equal(ar?.results[0].props['header.d'], 'example.com');
  });

  it('handles nested comments and a version number', () => {
    const ar = parseAuthResults('mx.cloudflare.net 1; spf=pass (outer (inner; x) y) smtp.mailfrom=example.com');
    assert.equal(ar?.authservId, 'mx.cloudflare.net');
    assert.equal(ar?.results[0].props['smtp.mailfrom'], 'example.com');
  });

  it('returns null for empty or oversized input', () => {
    assert.equal(parseAuthResults(null), null);
    assert.equal(parseAuthResults(''), null);
    assert.equal(parseAuthResults(`mx.cloudflare.net; ${'x'.repeat(20000)}`), null);
  });
});

describe('aligned', () => {
  it('accepts equal domains and subdomains either way', () => {
    assert.ok(aligned('example.com', 'example.com'));
    assert.ok(aligned('EXAMPLE.com.', 'example.com'));
    assert.ok(aligned('mail.example.com', 'example.com'));
    assert.ok(aligned('example.com', 'mail.example.com'));
  });

  it('rejects other domains, lookalikes and dotless names', () => {
    assert.ok(!aligned('evil.example', 'example.com'));
    assert.ok(!aligned('badexample.com', 'example.com'));
    assert.ok(!aligned('com', 'example.com'));
    assert.ok(!aligned('', 'example.com'));
  });
});

describe('checkAuth', () => {
  it('passes the real header via DKIM', () => {
    assert.deepEqual(checkAuth(REAL, 'example.com'), { ok: true, via: 'dkim' });
  });

  it('passes on aligned SPF alone', () => {
    assert.deepEqual(checkAuth('mx.cloudflare.net; dkim=none; spf=pass smtp.mailfrom=bounce@mail.example.com', 'example.com'), {
      ok: true,
      via: 'spf',
    });
  });

  it('passes on aligned DMARC alone', () => {
    assert.deepEqual(checkAuth('mx.cloudflare.net; dmarc=pass header.from=example.com', 'example.com'), { ok: true, via: 'dmarc' });
  });

  it('fails a spoofed From with another domain\'s valid SPF and DKIM', () => {
    const ar = 'mx.cloudflare.net; dkim=pass header.d=evil.example; spf=pass smtp.mailfrom=x@evil.example; dmarc=none';
    assert.deepEqual(checkAuth(ar, 'example.com'), { ok: false, reason: 'auth_unaligned' });
  });

  it('fails a DMARC pass for another From domain', () => {
    assert.deepEqual(checkAuth('mx.cloudflare.net; dmarc=pass header.from=evil.example', 'example.com'), {
      ok: false,
      reason: 'auth_unaligned',
    });
  });

  it('fails when nothing passed', () => {
    const ar = 'mx.cloudflare.net; dkim=fail header.d=example.com; spf=softfail smtp.mailfrom=example.com; dmarc=fail';
    assert.deepEqual(checkAuth(ar, 'example.com'), { ok: false, reason: 'auth_failed' });
  });

  it('fails when results are missing', () => {
    assert.deepEqual(checkAuth(null, 'example.com'), { ok: false, reason: 'auth_missing' });
    assert.deepEqual(checkAuth('mx.cloudflare.net', 'example.com'), { ok: false, reason: 'auth_failed' });
    assert.deepEqual(checkAuth('mx.cloudflare.net; dkim=pass', 'example.com'), { ok: false, reason: 'auth_unaligned' });
  });

  it('fails a header from any other authentication service', () => {
    const forged = 'mx.evil.example; dkim=pass header.d=example.com; spf=pass smtp.mailfrom=example.com';
    assert.deepEqual(checkAuth(forged, 'example.com'), { ok: false, reason: 'auth_untrusted' });
  });

  it('ignores a forged header that comes after Cloudflare\'s', () => {
    const joined = `mx.cloudflare.net; spf=fail smtp.mailfrom=x@evil.example; dkim=none, ${REAL}`;
    assert.deepEqual(checkAuth(joined, 'example.com'), { ok: false, reason: 'auth_failed' });
  });

  it('accepts a configured authserv-id', () => {
    assert.ok(checkAuth('mx.test.example; spf=pass smtp.mailfrom=example.com', 'example.com', 'mx.test.example').ok);
  });
});
