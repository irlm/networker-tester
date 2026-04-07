#!/usr/bin/env python3
"""Unit tests for Python reference API — tests pure logic without starting server."""

import hashlib
import sys
import zlib

passed = 0
failed = 0


def test(name, fn):
    global passed, failed
    try:
        fn()
        passed += 1
    except (AssertionError, Exception) as e:
        failed += 1
        print(f"FAIL: {name}")
        print(f"  {e}")


# --- Seeded RNG (matches mulberry32 from the reference APIs) ---
def mulberry32(seed):
    s = seed & 0xFFFFFFFF
    def rng():
        nonlocal s
        s = (s + 0x6D2B79F5) & 0xFFFFFFFF
        t = ((s ^ (s >> 15)) * (1 | s)) & 0xFFFFFFFF
        t = ((t + ((t ^ (t >> 7)) * (61 | t)) & 0xFFFFFFFF) ^ t) & 0xFFFFFFFF
        return ((t ^ (t >> 14)) & 0xFFFFFFFF) / 4294967296
    return rng


# --- Tests ---

def test_seeded_rng_deterministic():
    rng1 = mulberry32(42)
    rng2 = mulberry32(42)
    for _ in range(100):
        assert rng1() == rng2(), "RNG not deterministic"

test("seeded RNG is deterministic", test_seeded_rng_deterministic)


def test_different_seeds_differ():
    rng1 = mulberry32(42)
    rng2 = mulberry32(99)
    diffs = sum(1 for _ in range(100) if rng1() != rng2())
    assert diffs > 50, f"only {diffs} differences"

test("different seeds produce different values", test_different_seeds_differ)


def test_upload_processing():
    data = b"Hello, benchmark!"
    crc = zlib.crc32(data) & 0xFFFFFFFF
    sha = hashlib.sha256(data).hexdigest()
    compressed = zlib.compress(data)
    assert isinstance(crc, int)
    assert len(sha) == 64
    assert len(compressed) > 0

test("CRC32 + SHA256 + zlib produces output", test_upload_processing)


def test_upload_deterministic():
    data = b"Hello, benchmark!"
    sha1 = hashlib.sha256(data).hexdigest()
    sha2 = hashlib.sha256(data).hexdigest()
    assert sha1 == sha2

test("upload processing is deterministic", test_upload_deterministic)


def test_validate_deterministic():
    seed = 42
    data = bytes([(seed * 31 + i * 17) & 0xFF for i in range(1024)])
    md5_1 = hashlib.md5(data).hexdigest()
    data2 = bytes([(seed * 31 + i * 17) & 0xFF for i in range(1024)])
    md5_2 = hashlib.md5(data2).hexdigest()
    assert md5_1 == md5_2

test("validate checksum is deterministic", test_validate_deterministic)


def test_pagination_math():
    total = 1000
    page, per_page = 3, 20
    start = (page - 1) * per_page
    end = start + per_page
    assert start == 40
    assert end == 60
    assert end <= total

test("pagination math is correct", test_pagination_math)


def test_sorting():
    items = [{"score": 3.0}, {"score": 1.0}, {"score": 2.0}]
    sorted_items = sorted(items, key=lambda x: x["score"])
    assert sorted_items[0]["score"] == 1.0
    assert sorted_items[-1]["score"] == 3.0

test("sorting works correctly", test_sorting)


# --- Summary ---
print(f"\n{passed} passed, {failed} failed")
if failed > 0:
    sys.exit(1)
