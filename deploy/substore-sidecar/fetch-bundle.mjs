// fetch-bundle.mjs — fetch (or verify) the pinned sub-store-convert core bundle.
//
// Supply-chain rule from docs/THIRD_PARTY.md: NO floating versions. This pins both
// the upstream commit and the expected SHA-256 of the artifact. The bundle is a
// build output that is checked into the upstream repo, so we can consume it directly
// without cloning the pnpm workspace or its vendored Sub-Store submodule.
//
//   node fetch-bundle.mjs           download + verify + write ./index.mjs
//   node fetch-bundle.mjs --check   only verify an existing ./index.mjs
//
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, 'index.mjs');

const UPSTREAM_COMMIT = 'cddcab9c536da53c0f867cd6b5738791bc4e8121';
const EXPECTED_SHA256 = 'ee99f9a7fe2bb3a82321a5b94983ae3e223bfe02801fa55ef7c29478152027ef';
const EXPECTED_BYTES = 503527;
const URL = `https://cdn.jsdelivr.net/gh/tbxark/sub-store-convert@${UPSTREAM_COMMIT}/packages/core/build/index.js`;

function sha256(buf) {
  return createHash('sha256').update(buf).digest('hex');
}

function verify(buf) {
  const hash = sha256(buf);
  const okBytes = buf.length === EXPECTED_BYTES;
  const okHash = hash === EXPECTED_SHA256;
  console.log(`  bytes   = ${buf.length} (expected ${EXPECTED_BYTES}) ${okBytes ? 'OK' : 'MISMATCH'}`);
  console.log(`  sha256  = ${hash}`);
  console.log(`            (expected ${EXPECTED_SHA256}) ${okHash ? 'OK' : 'MISMATCH'}`);
  return okBytes && okHash;
}

const checkOnly = process.argv.includes('--check');

if (checkOnly) {
  if (!existsSync(OUT)) {
    console.error(`[fetch-bundle] ${OUT} missing; run without --check first`);
    process.exit(1);
  }
  console.log(`[fetch-bundle] verifying existing ${OUT}`);
  process.exit(verify(readFileSync(OUT)) ? 0 : 1);
}

console.log(`[fetch-bundle] upstream commit = ${UPSTREAM_COMMIT}`);
console.log(`[fetch-bundle] fetching ${URL}`);
const res = await fetch(URL, { headers: { 'user-agent': 'phlogo-gnosis-substore-sidecar' } });
if (!res.ok) {
  console.error(`[fetch-bundle] HTTP ${res.status} ${res.statusText}`);
  process.exit(1);
}
const buf = Buffer.from(await res.arrayBuffer());
if (!verify(buf)) {
  console.error('[fetch-bundle] REFUSING to write: artifact does not match the pin.');
  console.error('[fetch-bundle] Review the upstream change, then update the pin deliberately.');
  process.exit(2);
}
writeFileSync(OUT, buf);
console.log(`[fetch-bundle] wrote ${OUT}`);
