// The re-ingest script for dead letters archived in R2.

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, describe, it } from 'node:test';
import { fileURLToPath } from 'node:url';
import { payloadOf, reingest } from '../scripts/reingest.js';
import { sign } from '../src/sign.js';
import { mockFetch, SECRET, URL_ } from './helpers.js';

const T0 = Date.UTC(2025, 10, 21, 8, 0, 0);
const payload = { version: 1, message_id: '<a@mail.example.com>', title: 'Quarterly report', received_at: new Date(T0).toISOString() };
const record = { version: 1, archived_at: new Date(T0).toISOString(), reason: 'dead_letter', envelope: { v: 1, accepted_at: T0, attempt: 19, mid: 'abcdef012345', payload } };
const env = { INGEST_URL: URL_, INGEST_HMAC_SECRET: SECRET };

let dir = '';
before(async () => {
  dir = await mkdtemp(join(tmpdir(), 'reingest-'));
});
after(async () => {
  await rm(dir, { recursive: true, force: true });
});

const fileWith = async (name, content) => {
  const p = join(dir, name);
  await writeFile(p, typeof content === 'string' ? content : JSON.stringify(content));
  return p;
};

describe('reingest', () => {
  it('extracts the payload from an archived record only', () => {
    assert.deepEqual(payloadOf(record), payload);
    for (const bad of [null, 'x', {}, { ...record, version: 2 }, { version: 1, envelope: { junk: true } }]) assert.equal(payloadOf(bad), null);
  });

  it('posts each payload signed like the Worker does, and reports outcomes without content', async () => {
    const ok = await fileWith('ok.json', record);
    const down = await fileWith('down.json', record);
    const junk = await fileWith('junk.json', 'not json');
    const f = mockFetch([201, 503]);
    let n = 0;
    const results = await reingest([ok, down, junk], env, { fetch: f, now: () => T0, nonce: () => (n += 1).toString(16).padStart(32, '0') });
    assert.deepEqual(results, [
      { file: ok, outcome: 'ok', status: 201 },
      { file: down, outcome: 'transient', status: 503 },
      { file: junk, outcome: 'invalid', status: 0 },
    ]);
    assert.equal(f.calls.length, 2);
    const call = f.calls[0];
    assert.equal(call.url, URL_);
    assert.deepEqual(JSON.parse(call.body), payload);
    const h = call.init.headers;
    assert.equal(h['x-todo-timestamp'], String(T0 / 1000));
    const expected = await sign(SECRET, h['x-todo-timestamp'], h['x-todo-nonce'], new TextEncoder().encode(call.body));
    assert.equal(h['x-todo-signature'], expected);
  });

  it('prints usage and exits 2 without files', () => {
    const script = fileURLToPath(new URL('../scripts/reingest.js', import.meta.url));
    const r = spawnSync(process.execPath, [script], { encoding: 'utf8' });
    assert.equal(r.status, 2);
    assert.match(r.stderr, /usage/);
  });
});
