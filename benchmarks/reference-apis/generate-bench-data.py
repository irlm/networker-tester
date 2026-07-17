#!/usr/bin/env python3
"""
Generate the shared benchmark dataset (bench-data.json).

This file is read by ALL 18 language reference APIs so they operate on
identical input data. Run once; commit the output to the repo.

Usage:
    python3 generate-bench-data.py > shared/bench-data.json
"""
import json
import random
import math
import hashlib

# Fixed seed so the dataset is reproducible
rng = random.Random(20260403)

# ── Users (100) ──────────────────────────────────────────────────────────────
FIRST_NAMES = [
    "Alice", "Bob", "Charlie", "Diana", "Eve", "Frank", "Grace", "Hector",
    "Iris", "Jack", "Karen", "Leo", "Mona", "Nick", "Olivia", "Paul",
    "Quinn", "Rosa", "Steve", "Tina", "Uma", "Victor", "Wendy", "Xander",
    "Yuki", "Zane",
]
LAST_NAMES = [
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller",
    "Davis", "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez",
    "Wilson", "Anderson", "Thomas", "Taylor", "Moore", "Jackson", "Martin",
    "Lee", "Perez", "Thompson", "White", "Harris", "Sanchez",
]
DOMAINS = ["example.com", "test.org", "bench.dev", "sample.net", "demo.io"]

users = []
for i in range(100):
    first = rng.choice(FIRST_NAMES)
    last = rng.choice(LAST_NAMES)
    users.append({
        "id": i + 1,
        "name": f"{first} {last}",
        "email": f"{first.lower()}.{last.lower()}{rng.randint(1,999)}@{rng.choice(DOMAINS)}",
        "score": round(rng.uniform(0, 100), 2),
        "created_at": f"2026-{rng.randint(1,12):02d}-{rng.randint(1,28):02d}T{rng.randint(0,23):02d}:{rng.randint(0,59):02d}:00Z",
    })

# ── Search corpus (1000 strings) ────────────────────────────────────────────
WORDS = [
    "network", "latency", "throughput", "bandwidth", "packet", "protocol",
    "server", "client", "proxy", "benchmark", "performance", "metric",
    "endpoint", "request", "response", "timeout", "connection", "stream",
    "buffer", "cache", "firewall", "router", "switch", "gateway", "dns",
    "tls", "quic", "http", "tcp", "udp", "socket", "port", "frame",
    "header", "payload", "checksum", "compress", "encrypt", "decrypt",
    "handshake", "certificate", "session", "token", "auth", "load",
    "balance", "replicate", "shard", "queue", "retry",
]

search_corpus = []
for _ in range(1000):
    w1 = rng.choice(WORDS)
    w2 = rng.choice(WORDS)
    n = rng.randint(1, 999)
    search_corpus.append(f"{w1}-{w2}-{n}")

# ── Time series (10000 points) ──────────────────────────────────────────────
CATEGORIES = ["alpha", "beta", "gamma", "delta", "epsilon"]

timeseries = []
for i in range(10000):
    # Gaussian-ish values with some variation
    value = round(50 + 20 * math.sin(i * 0.01) + rng.gauss(0, 5), 4)
    timeseries.append({
        "ts": i,
        "value": value,
        "category": CATEGORIES[i % 5],
    })

# ── Transform inputs (sample payloads for /api/transform) ───────────────────
transform_inputs = []
for i in range(10):
    fields = [rng.choice(WORDS) for _ in range(rng.randint(2, 5))]
    values = [rng.randint(1, 10000) for _ in range(rng.randint(2, 6))]
    transform_inputs.append({
        "seed": i + 1,
        "fields": fields,
        "values": values,
    })

# ── Expected checksums (canonical responses per benchmarks/shared/API-SPEC.md §7)
# Each checksum is the SHA-256 of the CANONICAL JSON (sorted keys, no
# whitespace, shortest-round-trip floats) of the canonical (family C) response
# to one pinned request against this exact dataset. Validators fetch the
# request, re-serialize the parsed body canonically, hash, and compare.

def sha256_json(obj):
    """SHA-256 of canonical JSON (sorted keys, no whitespace)."""
    return hashlib.sha256(json.dumps(obj, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def r2(x):
    """Spec §5.6 rounding: round half away from zero to 2 decimals."""
    return math.floor(x * 100 + 0.5) / 100


# 1. users_page1 — GET /api/users?page=1&sort=name&order=asc
#    Page-1 window (all 100 users), stable-sorted by name, first 20.
users_page1_response = sorted(users, key=lambda u: u["name"])[:20]

# 2. aggregate_default — GET /api/aggregate (range ignored; full series).
#    Mirrors the normative algorithm: sort ascending, sequential float64 sum
#    over the SORTED values, truncated-index percentiles, quintile categories.
agg_values = sorted(p["value"] for p in timeseries)
agg_n = len(agg_values)
agg_sum = 0.0
for v in agg_values:
    agg_sum += v
chunk = agg_n // 5
categories = []
for i in range(5):
    part = agg_values[i * chunk:(i + 1) * chunk]
    part_sum = 0.0
    for v in part:
        part_sum += v
    categories.append({
        "category": f"q{i + 1}",
        "count": chunk,
        "mean": r2(part_sum / chunk),
        "min": r2(part[0]),
        "max": r2(part[-1]),
    })
aggregate_response = {
    "total_points": agg_n,
    "mean": r2(agg_sum / agg_n),
    "p50": r2(agg_values[int(agg_n * 0.50)]),
    "p95": r2(agg_values[int(agg_n * 0.95)]),
    "max": r2(agg_values[-1]),
    "categories": categories,
}

# 3. search_network_top10 — GET /api/search?q=network&limit=10
import re
pattern = re.compile("network")
scored = []
for item in search_corpus:
    m = pattern.search(item)
    if m:
        scored.append((m.start(), item))
scored.sort(key=lambda t: (t[0], t[1]))
search_response = {
    "query": "network",
    "total_matches": len(scored),
    "returned": min(10, len(scored)),
    "results": [
        {"rank": i + 1, "item": item, "match_position": pos}
        for i, (pos, item) in enumerate(scored[:10])
    ],
}

# 4. transform_input0 — POST /api/transform with transform_inputs[0]
t0 = transform_inputs[0]
transform_response = {
    "seed": t0["seed"],
    "hashed_fields": [hashlib.sha256(f.encode()).hexdigest() for f in t0["fields"]],
    "reversed_values": list(reversed(t0["values"])),
}

expected_checksums = {
    "users_page1": sha256_json(users_page1_response),
    "aggregate_default": sha256_json(aggregate_response),
    "search_network_top10": sha256_json(search_response),
    "transform_input0": sha256_json(transform_response),
}

# ── Assemble dataset ─────────────────────────────────────────────────────────
dataset = {
    "_version": 2,
    "_description": "Shared benchmark dataset for Application mode JSON API endpoints. Contract: benchmarks/shared/API-SPEC.md. DO NOT EDIT — regenerate with generate-bench-data.py.",
    "users": users,
    "search_corpus": search_corpus,
    "timeseries": timeseries,
    "transform_inputs": transform_inputs,
    "expected_checksums": expected_checksums,
}

print(json.dumps(dataset, indent=2))
