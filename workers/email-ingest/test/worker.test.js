import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import { describe, it } from 'node:test';
import { createAdmission, DEFAULT_DEAD_LETTER_QUEUE, handleEmail, handleQueue } from '../src/index.js';
import { MAX_PAYLOAD_BYTES, MAX_RAW_BYTES, RETRY_WINDOW_MS } from '../src/limits.js';
import { fitPayload, readCapped } from '../src/payload.js';
import { utf8Length } from '../src/text.js';
import { baseEnv, emailMessage, mime, mockFetch, mockLog, mockQueue, OWNER, queueMessage, SECRET, streamOf } from './helpers.js';

const T0 = Date.UTC(2025, 10, 21, 8, 0, 0);

function deps(fetch, { now = T0 } = {}) {
  let n = 0;
  const clock = { t: now };
  return {
    fetch,
    now: () => clock.t,
    nonce: () => (n += 1).toString(16).padStart(32, '0'),
    log: mockLog(),
    admission: createAdmission(),
    archiveAdmission: createAdmission(),
    clock,
  };
}

/** Asserts no log line carries an address, a subject word or the secret. */
function assertCleanLogs(d, ...forbidden) {
  for (const line of d.log.lines) {
    assert.ok(!line.includes('@'), line);
    assert.ok(!line.includes(SECRET), line);
    for (const f of forbidden) assert.ok(!line.toLowerCase().includes(f.toLowerCase()), `${line} contains ${f}`);
  }
}

function verify(call) {
  const h = call.init.headers;
  const expected = createHmac('sha256', SECRET).update(`${h['x-todo-timestamp']}\n${h['x-todo-nonce']}\n${call.body}`).digest('hex');
  assert.equal(h['x-todo-signature'], expected);
  return JSON.parse(call.body);
}

describe('email handler: accepted mail', () => {
  it('posts metadata only, with the original sender from the forwarded block', async () => {
    const f = mockFetch([201]);
    const d = deps(f);
    const msg = emailMessage();
    assert.equal(await handleEmail(msg, baseEnv(), d), 'delivered');
    assert.equal(msg.rejected, null);
    const p = verify(f.calls[0]);
    assert.deepEqual(p, {
      version: 1,
      message_id: '<fwd-1@mail.example.com>',
      list: null,
      title: 'Quarterly report',
      subject: 'Fwd: Quarterly report',
      keep: false,
      original_sender: { address: 'alice@example.org', name: 'Alice Example', source: 'forwarded_header' },
      forwarded_by: OWNER,
      received_at: new Date(T0).toISOString(),
      body: null,
      raw_truncated: false,
      parse_failed: false,
    });
    assert.ok(!f.calls[0].body.includes('report is attached'), 'body dropped by default');
    assertCleanLogs(d, 'quarterly');
  });

  it('picks the list from the plus address', async () => {
    const f = mockFetch([201]);
    await handleEmail(emailMessage({ to: 'todo+shopping@in.example.com' }), baseEnv(), deps(f));
    assert.equal(verify(f.calls[0]).list, 'shopping');
  });

  it('falls back to the From when there is no forwarded block', async () => {
    const f = mockFetch([201]);
    await handleEmail(emailMessage({ raw: mime({ subject: 'Buy milk', text: 'just a note' }) }), baseEnv(), deps(f));
    assert.deepEqual(verify(f.calls[0]).original_sender, { address: OWNER, name: 'Owner', source: 'from' });
  });

  it('#keep keeps text/plain and removes the marker from the title', async () => {
    const f = mockFetch([201]);
    const raw = mime({ subject: 'Fwd: #keep Invoice', text: 'plain body', html: '<p>html body</p>' });
    await handleEmail(emailMessage({ raw }), baseEnv(), deps(f));
    const p = verify(f.calls[0]);
    assert.equal(p.title, 'Invoice');
    assert.equal(p.subject, 'Fwd: #keep Invoice');
    assert.equal(p.keep, true);
    assert.deepEqual(p.body, { text: 'plain body', source: 'text', truncated: false });
  });

  it('#keep strips HTML when there is no text part, and finds the sender in it', async () => {
    const f = mockFetch([201]);
    const html = '<div>FYI</div><hr><b>From:</b> Dana Example &lt;dana@example.org&gt;<br><b>Sent:</b> Monday<p>Hi&nbsp;there</p>';
    await handleEmail(emailMessage({ raw: mime({ subject: 'FW: Plan #keep', html }) }), baseEnv(), deps(f));
    const p = verify(f.calls[0]);
    assert.equal(p.body.source, 'html');
    assert.ok(p.body.text.includes('Hi there'));
    assert.ok(!/<\/?[a-z]+[\s>]/i.test(p.body.text), 'no tags left');
    assert.deepEqual(p.original_sender, { address: 'dana@example.org', name: 'Dana Example', source: 'forwarded_header' });
  });

  it('decodes encoded subjects', async () => {
    const f = mockFetch([201]);
    await handleEmail(emailMessage({ raw: mime({ subject: '=?UTF-8?B?RndkOiBOw6FrdXA=?=' }) }), baseEnv(), deps(f));
    assert.equal(verify(f.calls[0]).title, 'Nákup');
  });

  it('synthesizes a stable Message-ID when the header is missing', async () => {
    const f = mockFetch([201]);
    const raw = mime({ messageId: null });
    await handleEmail(emailMessage({ raw }), baseEnv(), deps(f));
    await handleEmail(emailMessage({ raw }), baseEnv(), deps(f));
    const [a, b] = f.calls.map((c) => verify(c).message_id);
    assert.match(a, /^<sha256-[0-9a-f]{64}@email-ingest\.invalid>$/);
    assert.equal(a, b);
  });
});

describe('email handler: rejected mail', () => {
  const cases = [
    ['a sender not on the allowlist', { raw: mime({ from: 'Mallory <mallory@evil.example>' }), authResults: ['mx.cloudflare.net; dkim=pass header.d=evil.example'] }, 'not_allowed'],
    ['a spoofed From with another domain\'s valid SPF', { authResults: ['mx.cloudflare.net; dkim=none; spf=pass smtp.mailfrom=bounce@evil.example; dmarc=fail'] }, 'auth_unaligned'],
    ['missing authentication results', { authResults: null }, 'auth_missing'],
    ['results from another service only', { authResults: ['mx.evil.example; dkim=pass header.d=example.com'] }, 'auth_untrusted'],
    ['failing authentication', { authResults: ['mx.cloudflare.net; dkim=fail header.d=example.com; spf=fail smtp.mailfrom=example.com'] }, 'auth_failed'],
    ['two From addresses', { headers: { from: `${OWNER}, mallory@evil.example` } }, 'bad_from'],
    ['another recipient', { to: 'admin@in.example.com' }, 'wrong_recipient'],
    ['an invalid list name', { to: 'todo+a.b@in.example.com' }, 'bad_list'],
  ];
  for (const [name, opts, reason] of cases) {
    it(`rejects ${name}`, async () => {
      const f = mockFetch([201]);
      const d = deps(f);
      const env = baseEnv();
      const msg = emailMessage(opts);
      assert.equal(await handleEmail(msg, env, d), `rejected:${reason}`);
      assert.ok(msg.rejected);
      assert.ok(!msg.rejected.includes('@'));
      assert.equal(f.calls.length, 0, 'nothing posted');
      assert.equal(env.INGEST_QUEUE.sent.length, 0, 'nothing queued');
      assert.deepEqual(d.log.lines.map((l) => JSON.parse(l)), [{ evt: 'rejected', reason }]);
      assertCleanLogs(d, 'quarterly');
    });
  }

  it('rejects everything when the allowlist is empty', async () => {
    const msg = emailMessage();
    assert.equal(await handleEmail(msg, baseEnv({ ALLOWED_SENDERS: '' }), deps(mockFetch([201]))), 'rejected:allowlist_empty');
  });

  it('does not read the body of rejected mail', async () => {
    const msg = emailMessage({ authResults: null });
    await handleEmail(msg, baseEnv(), deps(mockFetch([201])));
    assert.equal(msg.raw.locked, false);
    const reader = msg.raw.getReader();
    assert.equal((await reader.read()).done, false, 'stream untouched');
  });

  it('bounces on a permanent server refusal, without retrying', async () => {
    const f = mockFetch([422]);
    const d = deps(f);
    const env = baseEnv();
    const msg = emailMessage();
    assert.equal(await handleEmail(msg, env, d), 'rejected:permanent');
    assert.match(msg.rejected, /HTTP 422/);
    assert.equal(env.INGEST_QUEUE.sent.length, 0);
    const line = JSON.parse(d.log.lines[0]);
    assert.equal(line.evt, 'permanent_failure');
    assert.equal(line.status, 422);
    assert.match(line.mid, /^[0-9a-f]{12}$/);
  });
});

describe('email handler: transient failures', () => {
  for (const failure of [new TypeError('network'), 503, 429]) {
    it(`queues the payload on ${failure instanceof Error ? 'a network error' : failure}`, async () => {
      const f = mockFetch([failure]);
      const d = deps(f);
      const env = baseEnv();
      const msg = emailMessage();
      assert.equal(await handleEmail(msg, env, d), 'queued');
      assert.equal(msg.rejected, null);
      const [{ body, opts }] = env.INGEST_QUEUE.sent;
      assert.equal(opts.delaySeconds, 30);
      assert.equal(body.v, 1);
      assert.equal(body.attempt, 1);
      assert.equal(body.accepted_at, T0);
      assert.deepEqual(body.payload, JSON.parse(f.calls[0].body), 'same metadata, unsigned');
      assert.ok(!JSON.stringify(body).includes('x-todo-signature'));
      assertCleanLogs(d, 'quarterly');
    });
  }

  it('queues when the Worker is misconfigured', async () => {
    const env = baseEnv({ INGEST_HMAC_SECRET: undefined });
    assert.equal(await handleEmail(emailMessage(), env, deps(mockFetch([201]))), 'queued');
    assert.equal(env.INGEST_QUEUE.sent.length, 1);
  });

  it('throws rather than accept mail it could neither deliver nor queue', async () => {
    const env = baseEnv({ INGEST_QUEUE: mockQueue({ fail: true }) });
    const d = deps(mockFetch([503]));
    await assert.rejects(handleEmail(emailMessage(), env, d));
    assert.equal(JSON.parse(d.log.lines.at(-1)).evt, 'queue_failed');
  });
});

describe('size caps', () => {
  it('reads at most MAX_RAW_BYTES of a large message and still creates the task', async () => {
    const f = mockFetch([201]);
    const big = mime({ text: `${'x'.repeat(76)}\r\n`.repeat(20_000) }); // ~1.5 MiB
    assert.ok(big.length > MAX_RAW_BYTES);
    await handleEmail(emailMessage({ raw: big }), baseEnv(), deps(f));
    const p = verify(f.calls[0]);
    assert.equal(p.raw_truncated, true);
    assert.equal(p.title, 'Quarterly report');
  });

  it('readCapped stops at the cap and cancels the stream', async () => {
    let cancelled = false;
    let pulled = 0;
    const s = new ReadableStream({
      pull(c) {
        pulled += 1;
        c.enqueue(new Uint8Array(1000));
      },
      cancel() {
        cancelled = true;
      },
    });
    const r = await readCapped(s, 2500);
    assert.equal(r.bytes.length, 2500);
    assert.equal(r.truncated, true);
    assert.ok(cancelled);
    assert.ok(pulled <= 4);
    const small = await readCapped(streamOf(new Uint8Array(10)), 2500);
    assert.deepEqual([small.bytes.length, small.truncated], [10, false]);
  });

  it('keeps the serialized payload under the queue bound', () => {
    const payload = { title: 't', body: { text: '"\\'.repeat(60_000), source: 'text', truncated: false } };
    const { json, payload: p } = fitPayload(payload);
    assert.ok(utf8Length(json) <= MAX_PAYLOAD_BYTES);
    assert.equal(p.body.truncated, true);
  });

  it('a #keep body is capped at 64 KiB and the request stays bounded', async () => {
    const f = mockFetch([201]);
    const raw = mime({ subject: 'Fwd: long #keep', text: 'ž'.repeat(200_000) });
    await handleEmail(emailMessage({ raw }), baseEnv(), deps(f));
    const p = verify(f.calls[0]);
    assert.equal(p.body.truncated, true);
    assert.ok(utf8Length(p.body.text) <= 64 * 1024);
    assert.ok(utf8Length(f.calls[0].body) <= MAX_PAYLOAD_BYTES);
  });

  it('rejects a message whose headers do not fit', async () => {
    const huge = mime({ extraHeaders: Array.from({ length: 5000 }, (_, i) => `X-Pad-${i}: ${'y'.repeat(200)}`) });
    const msg = emailMessage({ raw: huge });
    assert.equal(await handleEmail(msg, baseEnv(), deps(mockFetch([201]))), 'rejected:headers_too_large');
  });

  it('turns a message postal-mime refuses into a headers-only task', async () => {
    const deep = mime({
      extraHeaders: ['Content-Type: multipart/mixed; boundary="b0"'],
      text: Array.from({ length: 40 }, (_, i) => `--b${i}\r\nContent-Type: multipart/mixed; boundary="b${i + 1}"\r\n`).join('\r\n'),
    });
    const msg = emailMessage({ raw: deep.replace(/Content-Type: text\/plain[^\r]*\r\n/, '') });
    const f = mockFetch([201]);
    assert.equal(await handleEmail(msg, baseEnv(), deps(f)), 'delivered');
    const p = verify(f.calls[0]);
    assert.equal(p.parse_failed, true);
    assert.equal(p.title, 'Quarterly report');
    assert.equal(p.body, null);
    assert.equal(p.original_sender.source, 'from');
  });
});

describe('queue consumer', () => {
  const payload = { version: 1, message_id: '<fwd-1@mail.example.com>', title: 'Quarterly report', received_at: new Date(T0).toISOString() };
  const envelope = (attempt, acceptedAt = T0) => ({ v: 1, accepted_at: acceptedAt, attempt, mid: 'abcdef012345', payload });

  it('delivers and acks', async () => {
    const f = mockFetch([200]);
    const msg = queueMessage(envelope(1));
    await handleQueue({ messages: [msg] }, baseEnv(), deps(f, { now: T0 + 30_000 }));
    assert.ok(msg.acked);
    assert.deepEqual(verify(f.calls[0]), payload);
  });

  it('re-enqueues on a transient failure with the next delay, and re-signs each attempt', async () => {
    const f = mockFetch([503]);
    const env = baseEnv();
    const d = deps(f, { now: T0 + 30_000 });
    const msg = queueMessage(envelope(1));
    await handleQueue({ messages: [msg] }, env, d);
    assert.ok(msg.acked);
    assert.deepEqual(env.INGEST_QUEUE.sent[0], { body: envelope(2), opts: { delaySeconds: 60 } });

    d.clock.t += 61_000;
    const again = queueMessage(env.INGEST_QUEUE.sent[0].body);
    await handleQueue({ messages: [again] }, env, d);
    assert.equal(env.INGEST_QUEUE.sent[1].opts.delaySeconds, 120);
    const [a, b] = f.calls.map((c) => c.init.headers);
    assert.notEqual(a['x-todo-nonce'], b['x-todo-nonce']);
    assert.notEqual(a['x-todo-timestamp'], b['x-todo-timestamp']);
    assert.notEqual(a['x-todo-signature'], b['x-todo-signature']);
    assert.equal(f.calls[0].body, f.calls[1].body, 'same payload and Message-ID');
    assertCleanLogs(d, 'quarterly');
  });

  it('falls back to msg.retry when re-enqueueing fails', async () => {
    const env = baseEnv({ INGEST_QUEUE: mockQueue({ fail: true }) });
    const msg = queueMessage(envelope(3));
    await handleQueue({ messages: [msg] }, env, deps(mockFetch([502]), { now: T0 + 300_000 }));
    assert.equal(msg.acked, false);
    assert.deepEqual(msg.retried, { delaySeconds: 300 });

    // The redelivered message keeps envelope attempt 3; the platform's count moves the schedule on.
    const again = { ...queueMessage(envelope(3)), attempts: 2 };
    const d = deps(mockFetch([502]), { now: T0 + 1_000_000 });
    await handleQueue({ messages: [again] }, env, d);
    assert.deepEqual(again.retried, { delaySeconds: 600 });
    assert.equal(JSON.parse(d.log.lines.at(-1)).attempt, 5);
  });

  it('gives up at the deadline without posting, notifies the owner, then acks', async () => {
    const sent = [];
    const msg = queueMessage(envelope(19));
    const env = baseEnv({
      NOTIFY: { send: async (m) => sent.push({ ...m, ackedBefore: msg.acked }) },
      NOTIFY_ADDRESS: OWNER,
      NOTIFY_FROM: 'todo-notify@in.example.com',
    });
    const f = mockFetch([new TypeError('down')]);
    const d = deps(f, { now: T0 + RETRY_WINDOW_MS });
    await handleQueue({ messages: [msg] }, env, d);
    assert.equal(f.calls.length, 0, 'no POST after the deadline');
    assert.ok(msg.acked);
    assert.equal(sent[0].ackedBefore, false, 'notice sent before the ack');
    assert.equal(env.INGEST_QUEUE.sent.length, 0);
    assert.equal(sent.length, 1);
    assert.equal(sent[0].to, OWNER);
    assert.match(sent[0].text, /Quarterly report/);
    assert.match(sent[0].text, /abcdef012345/);
    assert.deepEqual(
      d.log.lines.map((l) => JSON.parse(l).evt),
      ['final_failure', 'notify_sent'],
    );
    assertCleanLogs(d, 'quarterly');
  });

  it('gives up when the last POST fails just before the deadline', async () => {
    const f = mockFetch([503]);
    const d = deps(f, { now: T0 + RETRY_WINDOW_MS - 500 });
    const msg = queueMessage(envelope(19));
    await handleQueue({ messages: [msg] }, baseEnv(), d);
    assert.equal(f.calls.length, 1);
    assert.ok(msg.acked);
    assert.deepEqual(d.log.lines.map((l) => JSON.parse(l).evt), ['final_failure', 'notify_unconfigured', 'dead_letter_lost']);
  });

  it('acks when neither a notice nor an archive is configured, and retries when the notice fails', async () => {
    const d = deps(mockFetch([500]), { now: T0 + RETRY_WINDOW_MS + 1 });
    const m1 = queueMessage(envelope(5));
    await handleQueue({ messages: [m1] }, baseEnv(), d);
    assert.deepEqual(d.log.lines.slice(-2).map((l) => JSON.parse(l)), [
      { evt: 'notify_unconfigured', mid: 'abcdef012345', reason: 'expired' },
      { evt: 'dead_letter_lost', mid: 'abcdef012345', reason: 'expired', via: 'unconfigured' },
    ]);
    assert.ok(m1.acked);

    const d2 = deps(mockFetch([500]), { now: T0 + RETRY_WINDOW_MS + 1 });
    const env = baseEnv({ NOTIFY: { send: async () => { throw new Error('no'); } }, NOTIFY_ADDRESS: OWNER, NOTIFY_FROM: 'n@in.example.com' });
    const m2 = queueMessage(envelope(5));
    await handleQueue({ messages: [m2] }, env, d2);
    assert.equal(JSON.parse(d2.log.lines.at(-1)).evt, 'notify_failed');
    assert.equal(m2.acked, false, 'not acked while the notice is undelivered');
    assert.deepEqual(m2.retried, { delaySeconds: 300 });
  });

  it('a permanent failure is not retried; status and hash are logged and the owner notified', async () => {
    const sent = [];
    const env = baseEnv({ NOTIFY: { send: async (m) => sent.push(m) }, NOTIFY_ADDRESS: OWNER, NOTIFY_FROM: 'n@in.example.com' });
    const d = deps(mockFetch([401]), { now: T0 + 60_000 });
    const msg = queueMessage(envelope(2));
    await handleQueue({ messages: [msg] }, env, d);
    assert.ok(msg.acked);
    assert.equal(env.INGEST_QUEUE.sent.length, 0);
    assert.deepEqual(JSON.parse(d.log.lines[0]), { evt: 'permanent_failure', mid: 'abcdef012345', status: 401, attempt: 2 });
    assert.equal(sent.length, 1);
    assert.match(sent[0].text, /HTTP 401/);
  });

  it('drops malformed queue messages', async () => {
    const f = mockFetch([200]);
    const msg = queueMessage({ nope: true });
    const d = deps(f);
    await handleQueue({ messages: [msg] }, baseEnv(), d);
    assert.ok(msg.acked);
    assert.equal(f.calls.length, 0);
    assert.equal(JSON.parse(d.log.lines[0]).evt, 'queue_invalid');
  });

  it('handles each message of a batch independently', async () => {
    const f = mockFetch([200, 503, 400]);
    const env = baseEnv();
    const msgs = [queueMessage(envelope(1)), queueMessage(envelope(1)), queueMessage(envelope(1))];
    await handleQueue({ messages: msgs }, env, deps(f, { now: T0 + 30_000 }));
    assert.ok(msgs.every((m) => m.acked));
    assert.equal(env.INGEST_QUEUE.sent.length, 1);
  });
});
