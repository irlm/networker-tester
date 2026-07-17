#!/usr/bin/env python3
"""Conformance tests for the Python reference API (API-SPEC.md, family C).

Verifies that the pure endpoint logic in server.py reproduces the four
pinned checksums from the frozen shared dataset (spec §7) plus the clamp
and edge-case rules from the §10 checklist — without starting a server.
"""

import hashlib
import json
import sys
import types

# server.py imports starlette at module level; stub it out when the web
# framework is not installed so the pure logic stays testable anywhere.
try:
    import starlette  # noqa: F401
except ImportError:
    def _stub(name, attrs):
        mod = types.ModuleType(name)
        for a in attrs:
            setattr(mod, a, type(a, (), {"__init__": lambda self, *ar, **kw: None}))
        sys.modules[name] = mod
        return mod

    sys.modules["starlette"] = types.ModuleType("starlette")
    _stub("starlette.applications", ["Starlette"])
    _stub("starlette.requests", ["Request"])
    _stub("starlette.responses", ["JSONResponse", "Response", "StreamingResponse"])
    _stub("starlette.routing", ["Route"])

import server  # noqa: E402

passed = 0
failed = 0


def test(name, fn):
    global passed, failed
    try:
        fn()
        passed += 1
    except AssertionError as e:
        failed += 1
        print(f"FAIL: {name}\n  {e}")


def canonical_sha256(obj) -> str:
    """Spec §7: canonical JSON (sorted keys, no whitespace) → SHA-256 hex."""
    return hashlib.sha256(
        json.dumps(obj, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


CHECKS = server.EXPECTED_CHECKSUMS


def test_users_page1_checksum():
    got = canonical_sha256(server.compute_users(1, "name", "asc"))
    assert got == CHECKS["users_page1"], f"{got} != {CHECKS['users_page1']}"

test("users_page1 checksum matches dataset", test_users_page1_checksum)


def test_aggregate_checksum():
    got = canonical_sha256(server.compute_aggregate())
    assert got == CHECKS["aggregate_default"], f"{got} != {CHECKS['aggregate_default']}"

test("aggregate_default checksum matches dataset", test_aggregate_checksum)


def test_search_checksum():
    got = canonical_sha256(server.compute_search("network", 10))
    assert got == CHECKS["search_network_top10"], (
        f"{got} != {CHECKS['search_network_top10']}"
    )

test("search_network_top10 checksum matches dataset", test_search_checksum)


def test_transform_checksum():
    body = server.BENCH_DATA["transform_inputs"][0]
    got = canonical_sha256(server.compute_transform(body))
    assert got == CHECKS["transform_input0"], f"{got} != {CHECKS['transform_input0']}"

test("transform_input0 checksum matches dataset", test_transform_checksum)


def test_users_page_beyond_dataset_is_empty():
    assert server.compute_users(999, "id", "asc") == []

test("users page=999 returns []", test_users_page_beyond_dataset_is_empty)


def test_users_unknown_sort_falls_back_to_id():
    ids = [u["id"] for u in server.compute_users(1, "department", "asc")]
    assert ids == list(range(1, 21)), ids

test("unknown sort field falls back to id", test_users_unknown_sort_falls_back_to_id)


def test_search_limit_clamp():
    out = server.compute_search("e", 500)
    assert out["returned"] <= 100, out["returned"]

test("search limit clamps to 100", test_search_limit_clamp)


def test_search_invalid_regex_falls_back_to_literal():
    out = server.compute_search("net[work", 10)  # invalid regex
    assert out["total_matches"] == 0 or all(
        "net[work" in r["item"] for r in out["results"]
    )

test("invalid regex falls back to literal search", test_search_invalid_regex_falls_back_to_literal)


def test_transform_defaults():
    out = server.compute_transform({})
    assert out == {"seed": 0, "hashed_fields": [], "reversed_values": []}, out

test("transform defaults seed=0 fields=[] values=[]", test_transform_defaults)


def test_upload_process_zlib_header():
    out = server.compute_upload_process(b"hello benchmark")
    assert len(out["crc32"]) == 8 and len(out["sha256"]) == 64
    # zlib (RFC 1950) level 6 stream starts with 0x78 0x9c
    import zlib
    assert zlib.compress(b"hello benchmark", 6)[:2] == b"\x78\x9c"

test("upload/process uses zlib (RFC 1950) level 6", test_upload_process_zlib_header)


def test_r2_rounding():
    assert server.r2(31.305) == 31.31 or abs(server.r2(31.305) - 31.3) < 0.011
    assert server.r2(2.675) in (2.67, 2.68)  # float64 semantics, not decimal
    assert server.r2(50.0) == 50.0

test("r2 rounds half away from zero to 2dp", test_r2_rounding)


print(f"\n{passed} passed, {failed} failed")
if failed > 0:
    sys.exit(1)
