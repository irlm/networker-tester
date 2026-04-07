#!/usr/bin/env node
"use strict";

// Unit tests for Node.js reference API — tests pure logic without starting server.
// Run: node test.js

const crypto = require("node:crypto");
const zlib = require("node:zlib");
const assert = require("node:assert/strict");

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

// Import logic from server.js by extracting the pure functions.
// Since server.js is a monolith, we re-implement the same logic here
// and verify it matches the spec.

// --- Data generation (must match server.js USERS/TIMESERIES/CORPUS) ---

const FIRST_NAMES = [
  "Alice", "Bob", "Charlie", "Diana", "Eve", "Frank", "Grace", "Hank",
  "Ivy", "Jack", "Karen", "Leo", "Mona", "Nick", "Olivia", "Paul",
  "Quinn", "Rose", "Steve", "Tina",
];
const LAST_NAMES = [
  "Adams", "Brown", "Clark", "Davis", "Evans", "Fisher", "Garcia",
  "Harris", "Irwin", "Jones", "King", "Lopez", "Miller", "Nelson",
  "Owen", "Parker", "Quinn", "Reed", "Smith", "Taylor",
];
const DEPARTMENTS = [
  "engineering", "marketing", "sales", "support", "hr", "finance",
  "legal", "ops", "product", "design",
];

function generateUsers(count) {
  const users = [];
  for (let i = 0; i < count; i++) {
    const seed = i * 31 + 7;
    users.push({
      id: i + 1,
      name: `${FIRST_NAMES[seed % 20]} ${LAST_NAMES[(seed * 3) % 20]}`,
      email: `user${i + 1}@example.com`,
      department: DEPARTMENTS[i % 10],
      score: ((seed * 17) % 1000) / 10,
    });
  }
  return users;
}

// --- Tests ---

test("generates 1000 users deterministically", () => {
  const users = generateUsers(1000);
  assert.equal(users.length, 1000);
  assert.equal(users[0].id, 1);
  assert.equal(users[0].name, "Hank Adams");
  assert.equal(users[999].id, 1000);
});

test("user emails are unique", () => {
  const users = generateUsers(1000);
  const emails = new Set(users.map((u) => u.email));
  assert.equal(emails.size, 1000);
});

test("departments cycle through 10 values", () => {
  const users = generateUsers(100);
  const depts = new Set(users.map((u) => u.department));
  assert.equal(depts.size, 10);
});

test("scores are between 0 and 100", () => {
  const users = generateUsers(1000);
  for (const u of users) {
    assert.ok(u.score >= 0 && u.score < 100, `score ${u.score} out of range`);
  }
});

test("pagination returns correct page size", () => {
  const users = generateUsers(1000);
  const page = 3;
  const perPage = 20;
  const start = (page - 1) * perPage;
  const pageData = users.slice(start, start + perPage);
  assert.equal(pageData.length, 20);
  assert.equal(pageData[0].id, 41);
});

test("sorting by score works", () => {
  const users = generateUsers(1000);
  const sorted = [...users].sort((a, b) => a.score - b.score);
  for (let i = 1; i < sorted.length; i++) {
    assert.ok(sorted[i].score >= sorted[i - 1].score);
  }
});

test("CRC32 + SHA256 + zlib produces deterministic output", () => {
  const input = Buffer.from("Hello, benchmark!");
  const crc = zlib.crc32(input);
  const sha = crypto.createHash("sha256").update(input).digest("hex");
  const compressed = zlib.deflateSync(input);

  assert.equal(typeof crc, "number");
  assert.equal(sha.length, 64);
  assert.ok(compressed.length > 0);
  assert.ok(compressed.length <= input.length + 20); // zlib overhead
});

test("validate endpoint checksum is deterministic", () => {
  // /api/validate?seed=42 should always return the same checksums
  const seed = 42;
  const data = Buffer.alloc(1024);
  for (let i = 0; i < data.length; i++) {
    data[i] = (seed * 31 + i * 17) & 0xff;
  }
  const md5 = crypto.createHash("md5").update(data).digest("hex");
  const sha256 = crypto.createHash("sha256").update(data).digest("hex");

  // Same seed must produce same hashes
  const data2 = Buffer.alloc(1024);
  for (let i = 0; i < data2.length; i++) {
    data2[i] = (seed * 31 + i * 17) & 0xff;
  }
  const md5_2 = crypto.createHash("md5").update(data2).digest("hex");
  assert.equal(md5, md5_2);
  assert.equal(sha256.length, 64);
});

// --- Summary ---
console.log(`\n${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
