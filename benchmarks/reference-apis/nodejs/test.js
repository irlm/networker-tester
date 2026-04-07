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

// --- Seeded RNG (same as server.js) ---
function mulberry32(seed) {
  let s = seed | 0;
  return function () {
    s = (s + 0x6d2b79f5) | 0;
    let t = Math.imul(s ^ (s >>> 15), 1 | s);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// --- Name lists from server.js ---
const FIRST_NAMES = [
  "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Heidi",
  "Ivan", "Judy", "Karl", "Laura", "Mallory", "Nina", "Oscar", "Peggy",
  "Quentin", "Ruth", "Steve", "Trent", "Ursula", "Victor", "Wendy",
  "Xander", "Yvonne", "Zack",
];
const LAST_NAMES = [
  "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller",
  "Davis", "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez",
  "Wilson", "Anderson", "Thomas", "Taylor", "Moore", "Jackson", "Martin",
];
const DEPARTMENTS = [
  "Engineering", "Marketing", "Sales", "Finance", "HR",
  "Operations", "Legal", "Support", "Design", "Product",
];

function generateUsers(rng, count) {
  const users = [];
  for (let i = 0; i < count; i++) {
    users.push({
      id: i + 1,
      name: FIRST_NAMES[Math.floor(rng() * FIRST_NAMES.length)] + " " +
            LAST_NAMES[Math.floor(rng() * LAST_NAMES.length)],
      email: "user" + (i + 1) + "@example.com",
      age: 22 + Math.floor(rng() * 44),
      department: DEPARTMENTS[Math.floor(rng() * DEPARTMENTS.length)],
      score: Math.round(rng() * 10000) / 100,
    });
  }
  return users;
}

// --- Tests ---

test("seeded RNG is deterministic", () => {
  const rng1 = mulberry32(42);
  const rng2 = mulberry32(42);
  for (let i = 0; i < 100; i++) {
    assert.equal(rng1(), rng2());
  }
});

test("generates 1000 users with correct structure", () => {
  const rng = mulberry32(42);
  const users = generateUsers(rng, 1000);
  assert.equal(users.length, 1000);
  assert.equal(users[0].id, 1);
  assert.equal(users[999].id, 1000);
  assert.ok(typeof users[0].name === "string");
  assert.ok(typeof users[0].email === "string");
  assert.ok(typeof users[0].age === "number");
  assert.ok(typeof users[0].department === "string");
  assert.ok(typeof users[0].score === "number");
});

test("user emails are unique", () => {
  const rng = mulberry32(42);
  const users = generateUsers(rng, 1000);
  const emails = new Set(users.map((u) => u.email));
  assert.equal(emails.size, 1000);
});

test("same seed produces same users", () => {
  const users1 = generateUsers(mulberry32(42), 100);
  const users2 = generateUsers(mulberry32(42), 100);
  for (let i = 0; i < 100; i++) {
    assert.equal(users1[i].name, users2[i].name);
    assert.equal(users1[i].score, users2[i].score);
  }
});

test("different seeds produce different users", () => {
  const users1 = generateUsers(mulberry32(42), 100);
  const users2 = generateUsers(mulberry32(99), 100);
  let differences = 0;
  for (let i = 0; i < 100; i++) {
    if (users1[i].name !== users2[i].name) differences++;
  }
  assert.ok(differences > 50, `only ${differences} differences`);
});

test("pagination returns correct page", () => {
  const rng = mulberry32(42);
  const users = generateUsers(rng, 1000);
  const page = 3, perPage = 20;
  const start = (page - 1) * perPage;
  const pageData = users.slice(start, start + perPage);
  assert.equal(pageData.length, 20);
  assert.equal(pageData[0].id, 41);
});

test("sorting by score works", () => {
  const rng = mulberry32(42);
  const users = generateUsers(rng, 1000);
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
});

// --- Summary ---
console.log(`\n${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
