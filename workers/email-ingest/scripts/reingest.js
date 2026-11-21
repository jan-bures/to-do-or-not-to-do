#!/usr/bin/env node
// Re-ingests dead letters archived in R2 (dead-letters/<date>/<mid>.json): POSTs each one's payload to
// the server, signed exactly as the Worker signs it. The server dedupes on message_id, so running it
// twice, or on a letter that did arrive after all, creates nothing new.
//
// Usage (see README.md, "Re-ingest dead letters"):
//   INGEST_URL=https://todo.example.com/api/v1/ingest/email INGEST_HMAC_SECRET=… \
//     node scripts/reingest.js dead-letters/2025-11-21/abcdef012345.json …
//
// It prints one line per file: the file name, the outcome and the HTTP status. Never the content.

import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { parseEnvelope, postPayload } from '../src/deliver.js';
import { randomNonce } from '../src/sign.js';

/**
 * The payload of an archived dead letter, or null if the file isn't one.
 * @param {unknown} record
 * @returns {Record<string, unknown> | null}
 */
export function payloadOf(record) {
  if (!record || typeof record !== 'object') return null;
  const r = /** @type {Record<string, any>} */ (record);
  if (r.version !== 1) return null;
  return parseEnvelope(r.envelope)?.payload ?? null;
}

/**
 * @typedef {{ file: string, outcome: 'ok' | 'transient' | 'permanent' | 'invalid', status: number }} ReingestResult
 */

/**
 * @param {readonly string[]} files
 * @param {Record<string, any>} env INGEST_URL and INGEST_HMAC_SECRET.
 * @param {{ fetch: typeof fetch, now: () => number, nonce: () => string }} deps
 * @returns {Promise<ReingestResult[]>}
 */
export async function reingest(files, env, deps) {
  /** @type {ReingestResult[]} */
  const results = [];
  for (const file of files) {
    let payload = null;
    try {
      payload = payloadOf(JSON.parse(await readFile(file, 'utf8')));
    } catch {
      // Unreadable or not JSON: reported as invalid below.
    }
    if (!payload) {
      results.push({ file, outcome: 'invalid', status: 0 });
      continue;
    }
    const res = await postPayload(env, JSON.stringify(payload), deps);
    results.push({ file, outcome: res.outcome, status: res.status });
  }
  return results;
}

async function main() {
  const files = process.argv.slice(2);
  if (files.length === 0) {
    console.error('usage: INGEST_URL=… INGEST_HMAC_SECRET=… node scripts/reingest.js <dead-letter.json>…');
    process.exit(2);
  }
  const env = { INGEST_URL: process.env.INGEST_URL, INGEST_HMAC_SECRET: process.env.INGEST_HMAC_SECRET };
  const results = await reingest(files, env, { fetch, now: () => Date.now(), nonce: randomNonce });
  for (const r of results) console.log(`${r.file}: ${r.outcome}${r.status ? ` (HTTP ${r.status})` : ''}`);
  process.exit(results.every((r) => r.outcome === 'ok') ? 0 : 1);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
