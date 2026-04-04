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

# ── Expected checksums (computed from this exact dataset) ────────────────────
# These are the SHA-256 hashes that /api/validate should return.
# Every language should produce these exact values.

def sha256_json(obj):
    """SHA-256 of canonical JSON (sorted keys, no whitespace)."""
    return hashlib.sha256(json.dumps(obj, sort_keys=True, separators=(',', ':')).encode()).hexdigest()

# Users sorted by name, first page of 20
users_sorted = sorted(users, key=lambda u: u["name"])
users_page1 = users_sorted[:20]

# Aggregate stats
values = [p["value"] for p in timeseries]
values_sorted = sorted(values)
n = len(values_sorted)
agg_summary = {
    "count": n,
    "mean": round(sum(values) / n, 4),
    "p50": values_sorted[n // 2],
    "p95": values_sorted[int(n * 0.95)],
    "max": values_sorted[-1],
}

# Search for "network" (top 10)
import re
search_results = []
pattern = re.compile("network")
for item in search_corpus:
    m = pattern.search(item)
    if m:
        search_results.append({"item": item, "score": 1000 - m.start()})
search_results.sort(key=lambda x: -x["score"])
search_top10 = search_results[:10]

# Transform first input
t0 = transform_inputs[0]
t0_hashed_fields = [hashlib.sha256(f.encode()).hexdigest() for f in t0["fields"]]
t0_reversed_values = list(reversed(t0["values"]))

expected_checksums = {
    "users_page1": sha256_json(users_page1),
    "aggregate_summary": sha256_json(agg_summary),
    "search_network_top10": sha256_json(search_top10),
    "transform_input0": sha256_json({
        "hashed_fields": t0_hashed_fields,
        "reversed_values": t0_reversed_values,
    }),
}

# ── Assemble dataset ─────────────────────────────────────────────────────────
dataset = {
    "_version": 1,
    "_description": "Shared benchmark dataset for Application mode JSON API endpoints. DO NOT EDIT — regenerate with generate-bench-data.py.",
    "users": users,
    "search_corpus": search_corpus,
    "timeseries": timeseries,
    "transform_inputs": transform_inputs,
    "expected_checksums": expected_checksums,
}

print(json.dumps(dataset, indent=2))
