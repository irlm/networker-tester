#!/usr/bin/env node
"use strict";

// Unit tests for the Node.js reference API — exercises the exported pure
// logic against the frozen contract (benchmarks/shared/API-SPEC.md) and the
// shared dataset. Run from this directory: node test.js
//
// Requiring server.js loads bench-data.json via the ../shared fallback and
// does NOT start the server (require.main guard).

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const zlib = require("node:zlib");

const server = require("./server.js");

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed++;
  } catch (e) {
    failed++;
    console.error(`FAIL: ${name}`);
    console.error(`  ${e.message}`);
  }
}

// --- Dataset (§2) ---

test("dataset loaded with spec counts", () => {
  assert.equal(server.BENCH_DATA._version, 2);
  assert.equal(server.BENCH_DATA.users.length, 100);
  assert.equal(server.BENCH_DATA.search_corpus.length, 1000);
  assert.equal(server.BENCH_DATA.timeseries.length, 10000);
  assert.equal(Object.keys(server.BENCH_DATA.expected_checksums).length, 4);
});

// --- /health (§5.1) ---

test("health body is constant and spec-shaped", () => {
  const h = JSON.parse(server.HEALTH_BODY);
  assert.equal(h.status, "ok");
  assert.equal(h.runtime, "nodejs");
  assert.ok(typeof h.version === "string" && h.version.length > 0);
  assert.equal(server.HEALTH_BODY, server.HEALTH_BODY); // byte-constant
});

// --- /download (§5.2) ---

test("download chunk is 8 KiB of 0x42", () => {
  assert.equal(server.CHUNK.length, 8192);
  assert.ok(server.CHUNK.every((b) => b === 0x42));
});

// --- Number formatting (§7 canonical JSON) ---

test("jnum keeps trailing .0 on integral floats", () => {
  assert.equal(server.jnum(39.0), "39.0");
  assert.equal(server.jnum(50.11), "50.11");
  assert.equal(server.jnum(0), "0.0");
});

test("r2 rounds half away from zero to 2 decimals", () => {
  assert.equal(server.r2(1.005), 1.0); // fp: 1.005*100 = 100.49999…
  assert.equal(server.r2(1.006), 1.01);
  assert.equal(server.r2(39.0000001), 39.0);
});

// --- /api/aggregate (§5.6) ---

test("aggregate matches the pinned canonical checksum", () => {
  const body = server.serializeAggregate(server.computeAggregate());
  const parsed = JSON.parse(body);
  assert.equal(parsed.total_points, 10000);
  assert.equal(parsed.categories.length, 5);
  assert.equal(parsed.categories[0].category, "q1");
  assert.equal(parsed.categories[0].count, 2000);
  // Integral floats must keep their ".0" in the serialized body — the
  // frozen dataset has q2.mean = 39.0.
  assert.match(body, /"mean":39\.0[,}]/);
});

// --- /api/search (§5.7) ---

test("search sorts by (position, item) and counts before truncation", () => {
  const r = server.computeSearch("network", 10);
  assert.equal(r.query, "network");
  assert.equal(r.returned, Math.min(10, r.total_matches));
  assert.ok(r.total_matches >= r.returned);
  assert.equal(r.results[0].rank, 1);
  for (let i = 1; i < r.results.length; i++) {
    const prev = r.results[i - 1];
    const cur = r.results[i];
    assert.ok(
      prev.match_position < cur.match_position ||
      (prev.match_position === cur.match_position && prev.item <= cur.item),
      "results not ordered by (position, item)"
    );
  }
});

test("search falls back to literal matching on invalid regex", () => {
  const r = server.computeSearch("([", 10);
  assert.equal(r.total_matches, 0); // "([" never appears in the corpus
});

test("search is case-sensitive", () => {
  const lower = server.computeSearch("network", 100);
  const upper = server.computeSearch("NETWORK", 100);
  assert.ok(lower.total_matches > 0);
  assert.equal(upper.total_matches, 0);
});

// --- cmpStr is ordinal, not locale ---

test("cmpStr compares bytewise", () => {
  assert.equal(server.cmpStr("a", "b"), -1);
  assert.equal(server.cmpStr("B", "a"), -1); // ordinal: uppercase first
  assert.equal(server.cmpStr("x", "x"), 0);
});

// --- /api/upload/process primitives (§5.8) ---

test("CRC32 + SHA256 + zlib level 6 produce deterministic output", () => {
  const input = Buffer.from("Hello, benchmark!");
  const crc = zlib.crc32(input) >>> 0;
  const sha = crypto.createHash("sha256").update(input).digest("hex");
  const compressed = zlib.deflateSync(input, { level: 6 });
  assert.equal(typeof crc, "number");
  assert.equal(sha.length, 64);
  // zlib (RFC 1950) magic: 0x78, level-6 flag byte 0x9c.
  assert.equal(compressed[0], 0x78);
  assert.equal(compressed[1], 0x9c);
});

// --- Summary ---
console.log(`\n${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
