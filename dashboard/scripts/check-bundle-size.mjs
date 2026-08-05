#!/usr/bin/env node
// Bundle-size gate (audit P1-11).
//
// vite's `chunkSizeWarningLimit` is a WARNING — it has never failed a build,
// so bundle growth has been invisible. This asserts a real budget over the
// built output and fails CI when it is exceeded.
//
// Budgets are deliberately set with headroom over the measured baseline, so
// this catches a step-change (a heavy dependency landing, a vendor chunk
// splitting badly) rather than nagging about normal drift. Raise them
// consciously in the same PR that grows the bundle.
import { readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const assetsDir = join(here, '..', 'dist', 'assets');

// Measured 2026-08-05: total 1.38 MB across 75 chunks, largest
// charts-vendor at 355 KB.
const TOTAL_BUDGET_BYTES = 1_800_000;   // ~30% headroom over 1.38 MB
const LARGEST_CHUNK_BUDGET_BYTES = 450_000; // ~27% headroom over 355 KB

let files;
try {
  files = readdirSync(assetsDir).filter((f) => f.endsWith('.js'));
} catch (err) {
  console.error(`bundle-size: cannot read ${assetsDir} — run \`npm run build\` first`);
  console.error(String(err));
  process.exit(2);
}

if (files.length === 0) {
  // Guard against the vacuous pass: no files must never look like "0 bytes, OK".
  console.error('bundle-size: no .js chunks found in dist/assets — build produced nothing?');
  process.exit(2);
}

const sized = files
  .map((f) => ({ name: f, bytes: statSync(join(assetsDir, f)).size }))
  .sort((a, b) => b.bytes - a.bytes);

const total = sized.reduce((sum, f) => sum + f.bytes, 0);
const largest = sized[0];
const mb = (n) => (n / 1_000_000).toFixed(2);
const kb = (n) => (n / 1_000).toFixed(1);

console.log(`bundle-size: ${files.length} chunks, total ${mb(total)} MB (budget ${mb(TOTAL_BUDGET_BYTES)} MB)`);
console.log('  largest chunks:');
for (const f of sized.slice(0, 5)) {
  console.log(`    ${kb(f.bytes).padStart(8)} kB  ${f.name}`);
}

let failed = false;
if (total > TOTAL_BUDGET_BYTES) {
  console.error(`\n::error::bundle total ${mb(total)} MB exceeds the ${mb(TOTAL_BUDGET_BYTES)} MB budget`);
  failed = true;
}
if (largest.bytes > LARGEST_CHUNK_BUDGET_BYTES) {
  console.error(`\n::error::largest chunk ${largest.name} is ${kb(largest.bytes)} kB, over the ${kb(LARGEST_CHUNK_BUDGET_BYTES)} kB per-chunk budget`);
  failed = true;
}

if (failed) {
  console.error('\nIf the growth is intentional, raise the budget in this script in the same PR.');
  process.exit(1);
}
console.log('\nbundle-size: within budget.');
