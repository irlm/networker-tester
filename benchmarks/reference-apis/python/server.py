"""AletheBench Python reference API — hypercorn + starlette (HTTP/3 QUIC)."""

import asyncio
import hashlib
import json
import logging
import os
import re
import random
import sys
import time
import zlib

logging.basicConfig(
    level=getattr(logging, os.environ.get("LOG_LEVEL", "INFO").upper(), logging.INFO),
    format="%(asctime)s %(levelname)s %(name)s %(message)s",
    stream=sys.stderr,
)
logger = logging.getLogger("bench-api")

from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, StreamingResponse
from starlette.routing import Route

CHUNK_SIZE = 8192
CHUNK = bytes([0x42]) * CHUNK_SIZE


# ---------------------------------------------------------------------------
# Shared benchmark dataset
# ---------------------------------------------------------------------------

def _load_bench_data() -> dict | None:
    """Load bench-data.json from BENCH_DATA_PATH, /opt/bench, or ../shared."""
    candidates = []
    env_path = os.environ.get("BENCH_DATA_PATH")
    if env_path:
        candidates.append(env_path)
    candidates.append("/opt/bench/bench-data.json")
    candidates.append(os.path.join(os.path.dirname(__file__), "..", "shared", "bench-data.json"))

    for p in candidates:
        try:
            with open(p) as f:
                data = json.load(f)
            logger.info("Loaded bench-data.json from %s (version %s, %d users, %d corpus, %d timeseries)",
                        p, data.get('_version'),
                        len(data.get('users', [])),
                        len(data.get('search_corpus', [])),
                        len(data.get('timeseries', [])))
            return data
        except (OSError, json.JSONDecodeError):
            continue

    logger.warning("bench-data.json not found, falling back to per-language PRNG")
    return None


BENCH_DATA: dict | None = _load_bench_data()

BENCH_API_TOKEN: str = os.environ.get("BENCH_API_TOKEN", "")


async def health(request: Request) -> JSONResponse:
    """GET /health -- runtime identity and version."""
    return JSONResponse(
        {"status": "ok", "runtime": "python", "version": sys.version}
    )


async def download(request: Request) -> StreamingResponse | JSONResponse:
    """GET /download/{size} -- stream `size` bytes of 0x42 in 8 KiB chunks."""
    try:
        size = int(request.path_params["size"])
    except (KeyError, ValueError):
        return JSONResponse({"error": "invalid size"}, status_code=400)

    if size <= 0:
        return JSONResponse({"error": "invalid size"}, status_code=400)

    async def generate():
        remaining = size
        while remaining > 0:
            to_send = min(remaining, CHUNK_SIZE)
            yield CHUNK[:to_send]
            remaining -= to_send

    return StreamingResponse(
        generate(),
        media_type="application/octet-stream",
        headers={"content-length": str(size)},
    )


async def upload(request: Request) -> JSONResponse:
    """POST /upload -- consume full request body, return byte count."""
    total = 0
    async for chunk in request.stream():
        total += len(chunk)
    return JSONResponse({"bytes_received": total})


def api_headers(duration_ms: float) -> dict:
    """Return required benchmark headers for JSON API endpoints."""
    return {
        "Server-Timing": f"app;dur={duration_ms:.1f}",
        "Cache-Control": "no-store, no-cache, must-revalidate",
        "Timing-Allow-Origin": "*",
        "Access-Control-Allow-Origin": "*",
    }


def api_json(body: dict, duration_ms: float, status_code: int = 200) -> JSONResponse:
    """Return a JSONResponse with required benchmark headers."""
    return JSONResponse(body, status_code=status_code, headers=api_headers(duration_ms))


# ---------------------------------------------------------------------------
# /api/users — deterministic user list with sorting & pagination
# ---------------------------------------------------------------------------

FIRST_NAMES = [
    "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Heidi",
    "Ivan", "Judy", "Karl", "Laura", "Mallory", "Nina", "Oscar", "Peggy",
    "Quentin", "Ruth", "Steve", "Trent", "Ursula", "Victor", "Wendy",
    "Xander", "Yvonne", "Zack",
]
LAST_NAMES = [
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller",
    "Davis", "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez",
    "Wilson", "Anderson", "Thomas", "Taylor", "Moore", "Jackson", "Martin",
]
DEPARTMENTS = [
    "Engineering", "Marketing", "Sales", "Finance", "HR",
    "Operations", "Legal", "Support", "Design", "Product",
]


def _generate_users(rng: random.Random, count: int = 100) -> list[dict]:
    users = []
    for i in range(count):
        users.append({
            "id": i + 1,
            "name": f"{rng.choice(FIRST_NAMES)} {rng.choice(LAST_NAMES)}",
            "email": f"user{i + 1}@example.com",
            "age": rng.randint(22, 65),
            "department": rng.choice(DEPARTMENTS),
            "score": round(rng.uniform(0, 100), 2),
        })
    return users


async def api_users(request: Request) -> JSONResponse:
    """GET /api/users?page=N&sort=field&order=asc|desc"""
    t0 = time.perf_counter()
    page = int(request.query_params.get("page", "1"))
    sort_field = request.query_params.get("sort", "id")
    order = request.query_params.get("order", "asc")

    if BENCH_DATA is not None and BENCH_DATA.get("users"):
        users = list(BENCH_DATA["users"])  # shallow copy for sorting
    else:
        rng = random.Random(page)
        users = _generate_users(rng)

    if sort_field in ("id", "name", "email", "age", "department", "score"):
        users.sort(key=lambda u: u[sort_field], reverse=(order == "desc"))

    page_size = 20
    start = (page - 1) * page_size
    page_users = users[start : start + page_size]

    duration_ms = (time.perf_counter() - t0) * 1000
    return api_json({
        "page": page,
        "page_size": page_size,
        "total": len(users),
        "sort": sort_field,
        "order": order,
        "users": page_users,
    }, duration_ms)


# ---------------------------------------------------------------------------
# /api/transform — hash + reverse string fields in JSON body
# ---------------------------------------------------------------------------

async def api_transform(request: Request) -> JSONResponse:
    """POST /api/transform — SHA-256 hash string fields, reverse values."""
    t0 = time.perf_counter()
    try:
        body = await request.json()
    except Exception:
        duration_ms = (time.perf_counter() - t0) * 1000
        return api_json({"error": "invalid JSON"}, duration_ms, status_code=400)

    transformed = {}
    for key, value in body.items():
        if isinstance(value, str):
            hashed = hashlib.sha256(value.encode()).hexdigest()
            transformed[key] = {"original_reversed": value[::-1], "sha256": hashed}
        else:
            transformed[key] = value

    duration_ms = (time.perf_counter() - t0) * 1000
    return api_json({
        "original_fields": len(body),
        "transformed": transformed,
    }, duration_ms)


# ---------------------------------------------------------------------------
# /api/aggregate — statistical aggregation over generated data
# ---------------------------------------------------------------------------

async def api_aggregate(request: Request) -> JSONResponse:
    """GET /api/aggregate?range=start,end"""
    t0 = time.perf_counter()
    range_param = request.query_params.get("range", "0,1000")
    parts = range_param.split(",")
    start = int(parts[0]) if len(parts) >= 1 else 0
    end = int(parts[1]) if len(parts) >= 2 else 1000

    if BENCH_DATA is not None and BENCH_DATA.get("timeseries"):
        values = list(BENCH_DATA["timeseries"])
    else:
        rng = random.Random(start)
        values = [rng.gauss(50, 15) for _ in range(10000)]

    count = len(values)
    categories = ["alpha", "beta", "gamma", "delta", "epsilon"]
    cat_rng = random.Random(start + 1)
    assignments = [cat_rng.choice(categories) for _ in range(count)]

    sorted_vals = sorted(values)
    mean = sum(values) / count
    p50 = sorted_vals[count // 2]
    p95 = sorted_vals[int(count * 0.95)]
    max_val = sorted_vals[-1]

    groups = {}
    for cat in categories:
        cat_vals = [v for v, c in zip(values, assignments) if c == cat]
        if cat_vals:
            cat_sorted = sorted(cat_vals)
            groups[cat] = {
                "count": len(cat_vals),
                "mean": round(sum(cat_vals) / len(cat_vals), 4),
                "p50": round(cat_sorted[len(cat_sorted) // 2], 4),
                "max": round(cat_sorted[-1], 4),
            }

    duration_ms = (time.perf_counter() - t0) * 1000
    return api_json({
        "range": {"start": start, "end": end},
        "total_points": count,
        "stats": {
            "mean": round(mean, 4),
            "p50": round(p50, 4),
            "p95": round(p95, 4),
            "max": round(max_val, 4),
        },
        "groups": groups,
    }, duration_ms)


# ---------------------------------------------------------------------------
# /api/search — regex search over generated strings
# ---------------------------------------------------------------------------

async def api_search(request: Request) -> JSONResponse:
    """GET /api/search?q=term&limit=N"""
    t0 = time.perf_counter()
    query = request.query_params.get("q", "test")
    limit = int(request.query_params.get("limit", "10"))

    if BENCH_DATA is not None and BENCH_DATA.get("search_corpus"):
        corpus = [{"id": i + 1, "text": t} for i, t in enumerate(BENCH_DATA["search_corpus"])]
    else:
        rng = random.Random(42)
        words = ["network", "latency", "throughput", "bandwidth", "packet",
                 "routing", "firewall", "proxy", "endpoint", "server",
                 "client", "protocol", "socket", "buffer", "stream",
                 "timeout", "retry", "cache", "queue", "load"]
        corpus = []
        for i in range(1000):
            phrase = " ".join(rng.choices(words, k=rng.randint(3, 8)))
            corpus.append({"id": i + 1, "text": phrase})

    pattern = re.compile(re.escape(query), re.IGNORECASE)

    results = []
    for item in corpus:
        match = pattern.search(item["text"])
        if match:
            score = 1.0 / (1 + match.start())
            results.append({**item, "score": round(score, 4)})

    results.sort(key=lambda r: r["score"], reverse=True)
    results = results[:limit]

    duration_ms = (time.perf_counter() - t0) * 1000
    return api_json({
        "query": query,
        "total_matches": len(results),
        "limit": limit,
        "results": results,
    }, duration_ms)


# ---------------------------------------------------------------------------
# /api/upload/process — hash + compress uploaded body
# ---------------------------------------------------------------------------

async def api_upload_process(request: Request) -> JSONResponse:
    """POST /api/upload/process — CRC32 + SHA-256 + zlib compress body."""
    t0 = time.perf_counter()
    body = b""
    async for chunk in request.stream():
        body += chunk

    crc = zlib.crc32(body) & 0xFFFFFFFF
    sha = hashlib.sha256(body).hexdigest()
    compressed = zlib.compress(body)

    duration_ms = (time.perf_counter() - t0) * 1000
    return api_json({
        "original_size": len(body),
        "compressed_size": len(compressed),
        "compression_ratio": round(len(compressed) / max(len(body), 1), 4),
        "crc32": format(crc, "08x"),
        "sha256": sha,
    }, duration_ms)


# ---------------------------------------------------------------------------
# /api/delayed — async delay with optional light work
# ---------------------------------------------------------------------------

async def api_delayed(request: Request) -> JSONResponse:
    """GET /api/delayed?ms=N&work=light"""
    t0 = time.perf_counter()
    ms = int(request.query_params.get("ms", "100"))
    work = request.query_params.get("work", "none")

    await asyncio.sleep(ms / 1000)

    if work == "light":
        _ = hashlib.sha256(b"benchmark" * 100).hexdigest()

    actual_ms = (time.perf_counter() - t0) * 1000
    duration_ms = actual_ms
    return api_json({
        "requested_ms": ms,
        "actual_ms": round(actual_ms, 2),
        "work": work,
    }, duration_ms)


# ---------------------------------------------------------------------------
# /api/validate — checksums for deterministic verification
# ---------------------------------------------------------------------------

async def api_validate(request: Request) -> JSONResponse:
    """GET /api/validate?seed=42 — return checksums for all endpoints."""
    t0 = time.perf_counter()
    seed = int(request.query_params.get("seed", "42"))

    # If shared data is loaded, return pre-computed checksums.
    if BENCH_DATA is not None and BENCH_DATA.get("expected_checksums"):
        checksums = dict(BENCH_DATA["expected_checksums"])
        duration_ms = (time.perf_counter() - t0) * 1000
        return api_json({
            "seed": seed,
            "checksums": checksums,
        }, duration_ms)

    # PRNG fallback.
    # Users checksum
    rng = random.Random(1)  # page=1
    users = _generate_users(rng)
    users_hash = hashlib.sha256(json.dumps(users, sort_keys=True).encode()).hexdigest()

    # Aggregate checksum
    rng = random.Random(0)  # start=0
    values = [rng.gauss(50, 15) for _ in range(10000)]
    agg_hash = hashlib.sha256(json.dumps([round(v, 4) for v in sorted(values)]).encode()).hexdigest()

    # Search checksum
    rng = random.Random(42)
    words = ["network", "latency", "throughput", "bandwidth", "packet",
             "routing", "firewall", "proxy", "endpoint", "server",
             "client", "protocol", "socket", "buffer", "stream",
             "timeout", "retry", "cache", "queue", "load"]
    corpus = [" ".join(rng.choices(words, k=rng.randint(3, 8))) for _ in range(1000)]
    search_hash = hashlib.sha256(json.dumps(corpus).encode()).hexdigest()

    duration_ms = (time.perf_counter() - t0) * 1000
    return api_json({
        "seed": seed,
        "checksums": {
            "users_page1": users_hash[:16],
            "aggregate_start0": agg_hash[:16],
            "search_corpus": search_hash[:16],
        },
    }, duration_ms)


class AuthMiddleware:
    """Validate BENCH_API_TOKEN on all routes except /health."""

    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] == "http" and BENCH_API_TOKEN:
            path = scope.get("path", "")
            if path != "/health":
                headers = dict(scope.get("headers", []))
                auth = headers.get(b"authorization", b"").decode()
                if not auth.startswith("Bearer ") or auth[7:] != BENCH_API_TOKEN:
                    t0 = time.perf_counter()
                    auth_dur = (time.perf_counter() - t0) * 1000
                    body = b'{"error":"unauthorized"}'
                    await send({
                        "type": "http.response.start",
                        "status": 401,
                        "headers": [
                            (b"content-type", b"application/json"),
                            (b"content-length", str(len(body)).encode()),
                            (b"server-timing", f"auth;dur={auth_dur:.1f}".encode()),
                        ],
                    })
                    await send({
                        "type": "http.response.body",
                        "body": body,
                    })
                    return
        await self.app(scope, receive, send)


class AltSvcMiddleware:
    """Advertise HTTP/3 via Alt-Svc header on every HTTP response."""

    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] == "http":
            port = os.environ.get("BENCH_PORT", "8443")

            async def send_with_alt_svc(message):
                if message["type"] == "http.response.start":
                    headers = list(message.get("headers", []))
                    headers.append(
                        (b"alt-svc", f'h3=":{port}"; ma=86400'.encode())
                    )
                    message = {**message, "headers": headers}
                await send(message)

            await self.app(scope, receive, send_with_alt_svc)
        else:
            await self.app(scope, receive, send)


app = AuthMiddleware(AltSvcMiddleware(
    Starlette(
        routes=[
            Route("/health", health, methods=["GET"]),
            Route("/download/{size:int}", download, methods=["GET"]),
            Route("/upload", upload, methods=["POST"]),
            Route("/api/users", api_users, methods=["GET"]),
            Route("/api/transform", api_transform, methods=["POST"]),
            Route("/api/aggregate", api_aggregate, methods=["GET"]),
            Route("/api/search", api_search, methods=["GET"]),
            Route("/api/upload/process", api_upload_process, methods=["POST"]),
            Route("/api/delayed", api_delayed, methods=["GET"]),
            Route("/api/validate", api_validate, methods=["GET"]),
        ],
    )
))
