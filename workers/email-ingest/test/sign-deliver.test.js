import assert from 'node:assert/strict';
import { createHash, createHmac } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, it } from 'node:test';
import {
  checkEnvelope,
  classifyStatus,
  configProblem,
  parseEnvelope,
  postPayload,
  retryDelaySeconds,
  retrySchedule,
} from '../src/deliver.js';
import { MAX_RETRIES, MAX_RETRY_DELAY_S, RETRY_WINDOW_MS } from '../src/limits.js';
import { midHash, logRecord } from '../src/log.js';
import { randomNonce, sign, signedHeaders } from '../src/sign.js';
import { mockFetch, SECRET, URL_ } from './helpers.js';

const vector = JSON.parse(readFileSync(new URL('../../../docs/ingest-email-signature.json', import.meta.url), 'utf8'));
const enc = new TextEncoder();

describe('signing', () => {
  it('matches the shared test vector', async () => {
    const body = enc.encode(vector.body);
    assert.equal(createHash('sha256').update(body).digest('hex'), vector.body_sha256);
    assert.equal(await sign(vector.secret, vector.timestamp, vector.nonce, body), vector.signature);
  });

  it('agrees with node:crypto on arbitrary input', async () => {
    const body = enc.encode('{"a":"ž✓\\n"}');
    const expected = createHmac('sha256', 'k'.repeat(40)).update('1700000000\nabcd\n').update(body).digest('hex');
    assert.equal(await sign('k'.repeat(40), '1700000000', 'abcd', body), expected);
  });

  it('builds headers with unix seconds, the nonce and a hex signature', async () => {
    const body = enc.encode('{}');
    const h = await signedHeaders(SECRET, body, { nowMs: 1_763_712_000_999, nonce: 'ab'.repeat(16) });
    assert.equal(h['x-todo-timestamp'], '1763712000');
    assert.equal(h['x-todo-nonce'], 'ab'.repeat(16));
    assert.match(h['x-todo-signature'], /^[0-9a-f]{64}$/);
    assert.equal(h['content-type'], 'application/json');
  });

  it('nonces are 128-bit hex and fresh', () => {
    const a = randomNonce();
    assert.match(a, /^[0-9a-f]{32}$/);
    assert.notEqual(a, randomNonce());
  });
});

describe('classifyStatus', () => {
  it('separates delivered, transient and permanent', () => {
    for (const s of [200, 201, 204]) assert.equal(classifyStatus(s), 'ok', String(s));
    for (const s of [408, 429, 500, 502, 503, 504, 521, 522, 530]) assert.equal(classifyStatus(s), 'transient', String(s));
    for (const s of [301, 302, 400, 401, 403, 404, 409, 413, 415, 422]) assert.equal(classifyStatus(s), 'permanent', String(s));
  });
});

describe('postPayload', () => {
  const env = { INGEST_HMAC_SECRET: SECRET, INGEST_URL: URL_ };
  const deps = (fetch) => ({ fetch, now: () => 1_763_712_000_000, nonce: () => '0f'.repeat(16) });

  it('posts a correctly signed body without following redirects', async () => {
    const f = mockFetch([201]);
    const r = await postPayload(env, '{"version":1}', deps(f));
    assert.deepEqual(r, { outcome: 'ok', status: 201, reason: 'delivered' });
    const { url, init, body } = f.calls[0];
    assert.equal(url, URL_);
    assert.equal(init.method, 'POST');
    assert.equal(init.redirect, 'manual');
    assert.ok(init.signal);
    const expected = createHmac('sha256', SECRET).update(`1763712000\n${'0f'.repeat(16)}\n${body}`).digest('hex');
    assert.equal(init.headers['x-todo-signature'], expected);
  });

  it('treats network errors and 5xx/429 as transient, other 4xx as permanent', async () => {
    assert.equal((await postPayload(env, '{}', deps(mockFetch([new TypeError('fetch failed')])))).outcome, 'transient');
    assert.equal((await postPayload(env, '{}', deps(mockFetch([503])))).outcome, 'transient');
    assert.equal((await postPayload(env, '{}', deps(mockFetch([429])))).outcome, 'transient');
    assert.deepEqual(await postPayload(env, '{}', deps(mockFetch([400]))), { outcome: 'permanent', status: 400, reason: 'http_permanent' });
  });

  it('treats bad configuration as transient without calling fetch', async () => {
    const f = mockFetch([200]);
    assert.deepEqual(await postPayload({ INGEST_URL: URL_ }, '{}', deps(f)), { outcome: 'transient', status: 0, reason: 'config_secret' });
    assert.equal(f.calls.length, 0);
  });

  it('configProblem checks the secret and the URL', () => {
    assert.equal(configProblem(env), null);
    assert.equal(configProblem({ ...env, INGEST_HMAC_SECRET: 'short' }), 'config_secret');
    assert.equal(configProblem({ ...env, INGEST_URL: 'http://todo.example.com/x' }), 'config_url');
    assert.equal(configProblem({ ...env, INGEST_URL: 'not a url' }), 'config_url');
    assert.equal(configProblem({ ...env, INGEST_URL: 'http://localhost:8787/api/v1/ingest/email' }), null);
  });
});

describe('retry schedule', () => {
  it('backs off, then holds at the maximum delay', () => {
    const delays = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((n) => retryDelaySeconds(n, 0, 0));
    assert.deepEqual(delays, [30, 60, 120, 300, 600, 1200, 1800, 3600, MAX_RETRY_DELAY_S, MAX_RETRY_DELAY_S]);
  });

  it('ends exactly at the 24 h window, within the retry cap', () => {
    const s = retrySchedule();
    assert.equal(s.at(-1), RETRY_WINDOW_MS / 1000);
    assert.ok(s.length <= MAX_RETRIES);
    for (let i = 1; i < s.length; i += 1) assert.ok(s[i] > s[i - 1]);
    assert.deepEqual(s.slice(0, 4), [30, 90, 210, 510]);
    assert.equal(s.length, 19, 'docs/ingest-email.md states 19 retries');
  });

  it('shortens the last delay and gives up after the window', () => {
    const accepted = 0;
    assert.equal(retryDelaySeconds(12, accepted, RETRY_WINDOW_MS - 100_000), 100);
    assert.equal(retryDelaySeconds(12, accepted, RETRY_WINDOW_MS - 1500), 1);
    assert.equal(retryDelaySeconds(12, accepted, RETRY_WINDOW_MS), null);
    assert.equal(retryDelaySeconds(MAX_RETRIES + 1, accepted, 0), null);
    assert.equal(retryDelaySeconds(0, accepted, 0), null);
  });
});

describe('envelopes', () => {
  const ok = { v: 1, accepted_at: 1, attempt: 1, mid: 'abcdef012345', payload: { title: 't' } };

  it('round-trips a valid envelope', () => {
    assert.deepEqual(parseEnvelope(checkEnvelope(ok)), ok);
  });

  it('rejects malformed envelopes', () => {
    for (const bad of [null, 'x', { ...ok, v: 2 }, { ...ok, attempt: -1 }, { ...ok, mid: 'nothex' }, { ...ok, payload: null }]) {
      assert.equal(parseEnvelope(bad), null);
    }
  });

  it('refuses oversized queue messages', () => {
    assert.throws(() => checkEnvelope({ ...ok, payload: { body: 'x'.repeat(130_000) } }), RangeError);
    assert.equal(parseEnvelope({ ...ok, payload: { body: 'x'.repeat(130_000) } }), null);
  });
});

describe('logging', () => {
  it('keeps only safe fields and tokens', async () => {
    const mid = await midHash('<a@example.com>');
    assert.match(mid, /^[0-9a-f]{12}$/);
    const rec = logRecord('rejected', {
      reason: 'not_allowed',
      mid,
      status: 400,
      from: 'you@example.com',
      subject: 'secret plans',
      list: 'shopping',
      via: 'you@example.com',
    });
    assert.deepEqual(rec, { evt: 'rejected', reason: 'not_allowed', mid, status: 400 });
    assert.deepEqual(logRecord('Bad Event!', { mid: 'you@example.com' }), { evt: 'invalid_event' });
  });
});
