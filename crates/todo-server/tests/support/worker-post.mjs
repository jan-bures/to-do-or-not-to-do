// Drives the Email Worker's own delivery code against a running todo-server (see tests/ingest.rs,
// `the_workers_signing_code_is_accepted`): `postPayload` from workers/email-ingest/src/deliver.js
// signs and POSTs each payload exactly as the Worker does.
//
// Usage: node worker-post.mjs <path to workers/email-ingest/src>
// Environment: INGEST_URL, INGEST_HMAC_SECRET, PAYLOADS (a JSON array of payloads).
// Prints a JSON array of { outcome, status }, one per payload.

import path from 'node:path';
import { pathToFileURL } from 'node:url';

const src = process.argv[2];
const load = (file) => import(pathToFileURL(path.join(src, file)).href);
const { postPayload } = await load('deliver.js');
const { randomNonce } = await load('sign.js');

const env = { INGEST_URL: process.env.INGEST_URL, INGEST_HMAC_SECRET: process.env.INGEST_HMAC_SECRET };
// The Worker allows plain http only for `localhost`; connect to the loopback address the test server
// is bound to, whatever `localhost` resolves to first.
const loopback = (url, init) => fetch(String(url).replace('//localhost:', '//127.0.0.1:'), init);

const out = [];
for (const p of JSON.parse(process.env.PAYLOADS)) {
  const r = await postPayload(env, JSON.stringify(p), { fetch: loopback, now: Date.now, nonce: randomNonce });
  out.push({ outcome: r.outcome, status: r.status });
}
console.log(JSON.stringify(out));
