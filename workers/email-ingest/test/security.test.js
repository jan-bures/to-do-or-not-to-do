// Review fixes: the recipient secret, exact alignment end to end, the admission budget, the
// booleans-only auth debug log, the dead-letter queue, and a worst-case CPU budget.

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { constantTimeEqual, parseRecipient, recipientSecret } from '../src/address.js';
import { createAdmission, DEFAULT_DEAD_LETTER_QUEUE, handleEmail, handleQueue } from '../src/index.js';
import { DEFAULT_MAX_ACCEPTED_PER_DAY } from '../src/limits.js';
import { baseEnv, emailMessage, mime, mockFetch, mockLog, OWNER, queueMessage, SECRET } from './helpers.js';

const T0 = Date.UTC(2025, 10, 21, 8, 0, 0);
const RSECRET = 'testsecrettestsecret';

function deps(fetch, now = T0) {
  const clock = { t: now };
  let n = 0;
  return {
    fetch,
    now: () => clock.t,
    nonce: () => (n += 1).toString(16).padStart(32, '0'),
    log: mockLog(),
    admission: createAdmission(),
    clock,
  };
}

const evts = (d) => d.log.lines.map((l) => JSON.parse(l));

describe('recipient secret', () => {
  it('validates the configured value', () => {
    assert.deepEqual(recipientSecret(undefined), { secret: null });
    assert.deepEqual(recipientSecret(''), { secret: null });
    assert.deepEqual(recipientSecret(` ${RSECRET.toUpperCase()} `), { secret: RSECRET });
    for (const bad of ['short', 'has-dash-0123456789', 'x'.repeat(65), 42]) assert.deepEqual(recipientSecret(bad), { error: true }, String(bad));
  });

  it('accepts only todo-<secret>@ and todo-<secret>+<list>@', () => {
    assert.deepEqual(parseRecipient(`todo-${RSECRET}@in.example.com`, 'todo', RSECRET), { ok: true, list: null });
    assert.deepEqual(parseRecipient(`TODO-${RSECRET.toUpperCase()}+Shopping@in.example.com`, 'todo', RSECRET), { ok: true, list: 'shopping' });
    for (const to of ['todo@in.example.com', 'todo+shopping@in.example.com', `todo-${RSECRET.slice(1)}@in.example.com`, `todo-${RSECRET}x@in.example.com`, `todo${RSECRET}@in.example.com`]) {
      assert.deepEqual(parseRecipient(to, 'todo', RSECRET), { ok: false, reason: 'wrong_recipient' }, to);
    }
  });

  it('constantTimeEqual', () => {
    assert.ok(constantTimeEqual('abc', 'abc'));
    assert.ok(!constantTimeEqual('abc', 'abd'));
    assert.ok(!constantTimeEqual('abc', 'abcd'));
    assert.ok(!constantTimeEqual('', 'a'));
  });

  it('end to end: plain todo@ is rejected, the secret address works, and the secret is never logged', async () => {
    const env = baseEnv({ RECIPIENT_SECRET: RSECRET });
    const d = deps(mockFetch([201]));
    const plain = emailMessage();
    assert.equal(await handleEmail(plain, env, d), 'rejected:wrong_recipient');
    assert.ok(!plain.rejected.includes(RSECRET));
    const f = mockFetch([201]);
    const d2 = deps(f);
    assert.equal(await handleEmail(emailMessage({ to: `todo-${RSECRET}+shopping@in.example.com` }), env, d2), 'delivered');
    assert.equal(JSON.parse(f.calls[0].body).list, 'shopping');
    for (const line of [...d.log.lines, ...d2.log.lines]) assert.ok(!line.includes(RSECRET), line);
  });

  it('a malformed secret rejects everything instead of falling back to todo@', async () => {
    const d = deps(mockFetch([201]));
    assert.equal(await handleEmail(emailMessage(), baseEnv({ RECIPIENT_SECRET: 'tooshort' }), d), 'rejected:config_recipient_secret');
  });
});

describe('exact alignment, end to end', () => {
  it('rejects an allowlisted From whose pass is for a delegated subdomain', async () => {
    const msg = emailMessage({ authResults: ['mx.cloudflare.net; dkim=pass header.d=mail.example.com; spf=pass smtp.mailfrom=b@mail.example.com; dmarc=pass header.from=example.com'] });
    const f = mockFetch([201]);
    assert.equal(await handleEmail(msg, baseEnv(), deps(f)), 'rejected:auth_unaligned');
    assert.equal(f.calls.length, 0);
  });
});

describe('admission budget', () => {
  it('defaults to the documented value', () => {
    assert.equal(DEFAULT_MAX_ACCEPTED_PER_DAY, 50);
  });

  it('throws (temporary failure) once the per-isolate budget is spent, and recovers after 24 h', async () => {
    const env = baseEnv({ MAX_ACCEPTED_PER_DAY: '2' });
    const d = deps(mockFetch([201]));
    assert.equal(await handleEmail(emailMessage(), env, d), 'delivered');
    assert.equal(await handleEmail(emailMessage(), env, d), 'delivered');
    const third = emailMessage();
    await assert.rejects(handleEmail(third, env, d), /admission budget/);
    assert.equal(third.rejected, null, 'not a permanent rejection');
    assert.equal(evts(d).at(-1).evt, 'admission_limited');
    d.clock.t += 24 * 60 * 60 * 1000 + 1;
    assert.equal(await handleEmail(emailMessage(), env, d), 'delivered');
  });

  it('only counts mail that passed the sender checks', async () => {
    const env = baseEnv({ MAX_ACCEPTED_PER_DAY: '1' });
    const d = deps(mockFetch([201]));
    for (let i = 0; i < 5; i += 1) await handleEmail(emailMessage({ authResults: null }), env, d);
    assert.equal(await handleEmail(emailMessage(), env, d), 'delivered');
  });

  it('ignores invalid configuration', async () => {
    const adm = createAdmission();
    for (let i = 0; i < DEFAULT_MAX_ACCEPTED_PER_DAY; i += 1) assert.ok(adm.admit(T0, DEFAULT_MAX_ACCEPTED_PER_DAY));
    assert.ok(!adm.admit(T0, DEFAULT_MAX_ACCEPTED_PER_DAY));
    const d = deps(mockFetch([201]));
    assert.equal(await handleEmail(emailMessage(), baseEnv({ MAX_ACCEPTED_PER_DAY: 'lots' }), d), 'delivered');
  });
});

describe('DEBUG_AUTH_SHAPE', () => {
  it('logs only booleans, never the header or an address', async () => {
    const d = deps(mockFetch([201]));
    await handleEmail(emailMessage(), baseEnv({ DEBUG_AUTH_SHAPE: 'true' }), d);
    const shape = evts(d).find((e) => e.evt === 'auth_shape');
    assert.ok(shape);
    for (const [k, v] of Object.entries(shape)) if (k !== 'evt') assert.equal(typeof v, 'boolean', k);
    assert.equal(shape.header_d_aligned, true);
    for (const line of d.log.lines) assert.ok(!line.includes('@') && !line.includes('example'), line);
  });

  it('is off by default', async () => {
    const d = deps(mockFetch([201]));
    await handleEmail(emailMessage(), baseEnv(), d);
    assert.ok(!evts(d).some((e) => e.evt === 'auth_shape'));
  });
});

describe('dead-letter queue', () => {
  const payload = { version: 1, title: 'Quarterly report', received_at: new Date(T0).toISOString() };
  const envelope = { v: 1, accepted_at: T0, attempt: 4, mid: 'abcdef012345', payload };
  const notifyEnv = (send) => baseEnv({ NOTIFY: { send }, NOTIFY_ADDRESS: OWNER, NOTIFY_FROM: 'n@in.example.com' });

  it('sends the notice, then acks, without posting', async () => {
    const sent = [];
    const msg = queueMessage(envelope);
    const f = mockFetch([201]);
    const d = deps(f);
    await handleQueue({ queue: DEFAULT_DEAD_LETTER_QUEUE, messages: [msg] }, notifyEnv(async (m) => sent.push({ ...m, acked: msg.acked })), d);
    assert.equal(f.calls.length, 0);
    assert.equal(sent.length, 1);
    assert.equal(sent[0].acked, false);
    assert.match(sent[0].text, /repeated errors/);
    assert.ok(msg.acked);
    assert.deepEqual(evts(d).map((e) => e.evt), ['dead_letter', 'notify_sent']);
  });

  it('retries when the notice fails', async () => {
    const msg = queueMessage(envelope);
    await handleQueue({ queue: DEFAULT_DEAD_LETTER_QUEUE, messages: [msg] }, notifyEnv(async () => { throw new Error('x'); }), deps(mockFetch([201])));
    assert.equal(msg.acked, false);
    assert.deepEqual(msg.retried, { delaySeconds: 300 });
  });

  it('still notifies for a malformed dead letter', async () => {
    const sent = [];
    const msg = queueMessage({ junk: true });
    await handleQueue({ queue: DEFAULT_DEAD_LETTER_QUEUE, messages: [msg] }, notifyEnv(async (m) => sent.push(m)), deps(mockFetch([201])));
    assert.match(sent[0].text, /Reference: unknown/);
    assert.ok(msg.acked);
  });

  it('honours a configured dead-letter queue name', async () => {
    const msg = queueMessage(envelope);
    const f = mockFetch([201]);
    await handleQueue({ queue: 'custom-dlq', messages: [msg] }, baseEnv({ DEAD_LETTER_QUEUE: 'custom-dlq' }), deps(f));
    assert.equal(f.calls.length, 0);
    assert.ok(msg.acked);
  });
});

const HOSTILE_HTML = `<div>${'<a x="'.repeat(20)}${'<'.repeat(300)}<!--${'<style>'.repeat(20)}</div>`;
const SHORT_FROM = `From: ${'c'.repeat(200)}\r\nSent: y`;
const DASHES = `${'-'.repeat(990)}x`;
const FROM_LINES = [DASHES, DASHES, DASHES, `From: ${'a'.repeat(990)}`, `From: ${'@'.repeat(990)}`, `From: ${'a.'.repeat(495)}`, 'Sent: x'].join('\r\n');
const TARGET_BYTES = 250 * 1024;

const textPart = (body) => `Content-Type: text/plain; charset=utf-8\r\n\r\n${body}\r\n`;
const htmlPart = (body) => `Content-Type: text/html; charset=utf-8\r\n\r\n${body}\r\n`;

/** message/rfc822 nested `depth` levels deep, each level a multipart with one leaf and the next level. */
function nested(depth, leaf) {
  if (depth === 0) return leaf(0);
  const b = `n${depth}`;
  return [
    'Content-Type: message/rfc822',
    '',
    `From: Inner <inner${depth}@example.org>`,
    `Subject: nested ${depth}`,
    `Content-Type: multipart/mixed; boundary="${b}"`,
    '',
    `--${b}`,
    leaf(depth),
    `--${b}`,
    nested(depth - 1, leaf),
    `--${b}--`,
    '',
  ].join('\r\n');
}

/**
 * A hostile message: 40 long headers, `count` outer parts built by `leaf`, nested message/rfc822 at the
 * deepest level the parser allows, then padded with one more part to just under the 256 KiB read cap.
 */
function hostileMime(count, leaf, pad = true) {
  const b = 'OUTER';
  const parts = [];
  for (let i = 0; i < count; i += 1) parts.push(`--${b}\r\n${leaf(i)}`);
  parts.push(`--${b}\r\n${nested(3, leaf)}`);
  const head = [
    ...Array.from({ length: 40 }, (_, i) => `X-Long-${i}: ${'h'.repeat(900)}`),
    `From: Owner <${OWNER}>`,
    'To: todo@in.example.com',
    'Subject: Fwd: worst case #keep',
    'Message-ID: <worst@mail.example.com>',
    'MIME-Version: 1.0',
    `Content-Type: multipart/mixed; boundary="${b}"`,
    '',
  ].join('\r\n');
  const build = (extra) => [head, ...parts, ...(extra ? [`--${b}\r\n${extra}`] : []), `--${b}--`, ''].join('\r\n');
  if (!pad) return build(null);
  const room = TARGET_BYTES - build(leaf(1)).length;
  const unit = leaf(1).split('\r\n\r\n')[1];
  return build(leaf(1).replace(unit, unit.repeat(Math.max(1, Math.floor(room / unit.length)))));
}

const CASES = [
  {
    name: 'mixed text and HTML within the part limit',
    raw: () => hostileMime(80, (i) => (i % 10 === 0 ? textPart(FROM_LINES) : i % 2 ? htmlPart(HOSTILE_HTML) : textPart(SHORT_FROM))),
    check: (p) => assert.ok(p.body && !p.parse_failed, '#keep body built'),
  },
  {
    name: 'HTML only, so every part goes through htmlToText',
    raw: () => hostileMime(80, () => htmlPart(HOSTILE_HTML)),
    check: (p) => assert.ok(p.body && !p.parse_failed, '#keep body built'),
  },
  {
    name: 'over the part limit, so the headers-only fallback is used',
    raw: () => hostileMime(300, (i) => (i % 2 ? htmlPart(HOSTILE_HTML) : textPart(SHORT_FROM)), false),
    check: (p) => assert.ok(p.body === null && p.parse_failed === true, 'headers-only task'),
  },
];

describe('CPU budget', () => {
  for (const c of CASES) {
    it(`the full email handler stays far below a generous budget: ${c.name}`, async (t) => {
      const raw = c.raw();
      assert.ok(raw.length < 256 * 1024, `fits the read cap, so every part is parsed (${raw.length})`);
      const samples = [];
      for (let i = 0; i < 7; i += 1) {
        const f = mockFetch([201]);
        const msg = emailMessage({ raw });
        // Wall time is an upper bound on CPU time here: the mocks resolve immediately, and
        // process.cpuUsage() is too coarse on some platforms (15.6 ms ticks on Windows).
        const before = performance.now();
        const outcome = await handleEmail(msg, baseEnv(), deps(f));
        samples.push(performance.now() - before);
        assert.equal(outcome, 'delivered');
        c.check(JSON.parse(f.calls[0].body));
      }
      samples.sort((a, b) => a - b);
      const median = samples[Math.floor(samples.length / 2)];
      t.diagnostic(`raw ${raw.length} bytes; ms per run, median ${median.toFixed(1)}: ${samples.map((s) => s.toFixed(1)).join(' ')}`);
      // The Workers Free plan allows 10 ms. Node on a CI runner is noisier and often slower, so the
      // assertion is generous; docs/ingest-email.md records the measured margin.
      assert.ok(median < 100, `median ${median.toFixed(1)} ms`);
    });
  }
});

void SECRET;
