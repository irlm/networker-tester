#!/usr/bin/env python3
"""Unit tests for Python reference API — tests pure logic without starting server."""

import hashlib
import json
import sys
import zlib

passed = 0
failed = 0


def test(name, fn):
    global passed, failed
    try:
        fn()
        passed += 1
    except AssertionError as e:
        failed += 1
        print(f"FAIL: {name}")
        print(f"  {e}")


# --- Data generation (must match app.py) ---

FIRST_NAMES = [
    "Alice", "Bob", "Charlie", "Diana", "Eve", "Frank", "Grace", "Hank",
    "Ivy", "Jack", "Karen", "Leo", "Mona", "Nick", "Olivia", "Paul",
    "Quinn", "Rose", "Steve", "Tina",
]
LAST_NAMES = [
    "Adams", "Brown", "Clark", "Davis", "Evans", "Fisher", "Garcia",
    "Harris", "Irwin", "Jones", "King", "Lopez", "Miller", "Nelson",
    "Owen", "Parker", "Quinn", "Reed", "Smith", "Taylor",
]
DEPARTMENTS = [
    "engineering", "marketing", "sales", "support", "hr", "finance",
    "legal", "ops", "product", "design",
]


def generate_users(count):
    users = []
    for i in range(count):
        seed = i * 31 + 7
        users.append({
            "id": i + 1,
            "name": f"{FIRST_NAMES[seed % 20]} {LAST_NAMES[(seed * 3) % 20]}",
            "email": f"user{i + 1}@example.com",
            "department": DEPARTMENTS[i % 10],
            "score": ((seed * 17) % 1000) / 10,
        })
    return users


# --- Tests ---

def test_generates_1000_users():
    users = generate_users(1000)
    assert len(users) == 1000
    assert users[0]["id"] == 1
    assert users[0]["name"] == "Hank Adams"
    assert users[999]["id"] == 1000

test("generates 1000 users deterministically", test_generates_1000_users)


def test_unique_emails():
    users = generate_users(1000)
    emails = {u["email"] for u in users}
    assert len(emails) == 1000

test("user emails are unique", test_unique_emails)


def test_departments_cycle():
    users = generate_users(100)
    depts = {u["department"] for u in users}
    assert len(depts) == 10

test("departments cycle through 10 values", test_departments_cycle)


def test_scores_in_range():
    users = generate_users(1000)
    for u in users:
        assert 0 <= u["score"] < 100, f"score {u['score']} out of range"

test("scores are between 0 and 100", test_scores_in_range)


def test_pagination():
    users = generate_users(1000)
    page, per_page = 3, 20
    start = (page - 1) * per_page
    page_data = users[start:start + per_page]
    assert len(page_data) == 20
    assert page_data[0]["id"] == 41

test("pagination returns correct page", test_pagination)


def test_sorting():
    users = generate_users(1000)
    sorted_users = sorted(users, key=lambda u: u["score"])
    for i in range(1, len(sorted_users)):
        assert sorted_users[i]["score"] >= sorted_users[i - 1]["score"]

test("sorting by score works", test_sorting)


def test_upload_processing():
    data = b"Hello, benchmark!"
    crc = zlib.crc32(data) & 0xFFFFFFFF
    sha = hashlib.sha256(data).hexdigest()
    compressed = zlib.compress(data)
    assert isinstance(crc, int)
    assert len(sha) == 64
    assert len(compressed) > 0

test("CRC32 + SHA256 + zlib produces output", test_upload_processing)


def test_validate_deterministic():
    seed = 42
    data = bytes([(seed * 31 + i * 17) & 0xFF for i in range(1024)])
    md5_1 = hashlib.md5(data).hexdigest()
    data2 = bytes([(seed * 31 + i * 17) & 0xFF for i in range(1024)])
    md5_2 = hashlib.md5(data2).hexdigest()
    assert md5_1 == md5_2
    assert len(hashlib.sha256(data).hexdigest()) == 64

test("validate checksum is deterministic", test_validate_deterministic)


# --- Summary ---
print(f"\n{passed} passed, {failed} failed")
if failed > 0:
    sys.exit(1)
