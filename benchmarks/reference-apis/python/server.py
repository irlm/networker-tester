"""Networker Bench Python reference API — starlette on uvicorn.

Conforms to benchmarks/shared/API-SPEC.md (frozen contract v1, family C).

Runtime identity (spec §3 / audit F12): **uvicorn** — matches the Dockerfile,
deploy.sh, and ci/run-language.sh. uvicorn has no HTTP/3, so this server does
NOT advertise Alt-Svc h3 (audit P1#10).
"""

import asyncio
import hashlib
import json
import logging
import math
import os
import platform
import re
import sys
import time
import zlib

from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, Response, StreamingResponse
from starlette.routing import Route

logging.basicConfig(
    level=getattr(logging, os.environ.get("LOG_LEVEL", "INFO").upper(), logging.INFO),
    format="%(asctime)s %(levelname)s %(name)s %(message)s",
    stream=sys.stderr,
)
logger = logging.getLogger("bench-api")

CHUNK_SIZE = 8192  # spec §5.2: pinned chunk size
CHUNK = b"\x42" * CHUNK_SIZE  # spec §5.2: pinned fill byte 0x42 ('B')
DOWNLOAD_CAP = 2_147_483_648  # spec §5.2: 2 GiB cap

BENCH_API_TOKEN: str = os.environ.get("BENCH_API_TOKEN", "")


# ---------------------------------------------------------------------------
# Shared benchmark dataset (spec §2) — load failure is FATAL, no PRNG fallback
# ---------------------------------------------------------------------------

def _fatal(msg: str) -> None:
    print(f"FATAL: {msg}", file=sys.stderr)
    sys.exit(1)


def _load_bench_data() -> dict:
    env_path = os.environ.get("BENCH_DATA_PATH")
    if env_path:
        # An explicitly configured path must exist and parse (spec §2).
        try:
            with open(env_path, "rb") as f:
                data = json.load(f)
        except (OSError, json.JSONDecodeError) as e:
            _fatal(f"BENCH_DATA_PATH={env_path} could not be loaded: {e}")
        source = env_path
    else:
        candidates = [
            "/opt/bench/bench-data.json",
            os.path.join(os.path.dirname(os.path.abspath(__file__)),
                         "..", "shared", "bench-data.json"),
        ]
        data = None
        source = ""
        for p in candidates:
            if not os.path.exists(p):
                continue
            try:
                with open(p, "rb") as f:
                    data = json.load(f)
            except (OSError, json.JSONDecodeError) as e:
                _fatal(f"bench-data.json exists at {p} but could not be loaded: {e}")
            source = p
            break
        if data is None:
            _fatal(
                "bench-data.json not found (set BENCH_DATA_PATH or deploy "
                "/opt/bench/bench-data.json); reference implementations have "
                "no PRNG fallback (spec §2)"
            )

    # Verify the §2 schema counts — a truncated/foreign dataset must not be
    # silently benchmarked.
    checks = [
        ("_version", data.get("_version") == 2, "== 2"),
        ("users", len(data.get("users", [])) == 100, "100 entries"),
        ("search_corpus", len(data.get("search_corpus", [])) == 1000, "1000 entries"),
        ("timeseries", len(data.get("timeseries", [])) == 10000, "10000 entries"),
        ("transform_inputs", len(data.get("transform_inputs", [])) == 10, "10 entries"),
        ("expected_checksums", len(data.get("expected_checksums", {})) == 4, "4 keys"),
    ]
    for field, ok, want in checks:
        if not ok:
            _fatal(f"bench-data.json at {source}: field {field!r} failed check ({want})")

    logger.info("Loaded bench-data.json from %s (_version 2, 100 users, "
                "1000 corpus, 10000 timeseries)", source)
    return data


BENCH_DATA: dict = _load_bench_data()
USERS: list = BENCH_DATA["users"]
SEARCH_CORPUS: list = BENCH_DATA["search_corpus"]
# Aggregate reads only the value field, in dataset order (spec §5.6).
TS_VALUES: list = [p["value"] for p in BENCH_DATA["timeseries"]]
EXPECTED_CHECKSUMS: dict = BENCH_DATA["expected_checksums"]

# /health body is a byte-constant precomputed at startup (spec §5.1).
HEALTH_BODY: bytes = json.dumps(
    {"status": "ok", "runtime": "python", "version": platform.python_version()},
    separators=(",", ":"),
).encode()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def r2(x: float) -> float:
    """Spec §5.6: round half away from zero to 2 decimals."""
    return math.floor(x * 100 + 0.5) / 100


def _int_param(request: Request, key: str, default: int) -> int:
    raw = request.query_params.get(key)
    if raw is None:
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def api_headers(duration_ms: float) -> dict:
    """The four benchmark headers required on every /api/* response (§1)."""
    return {
        "Server-Timing": f"app;dur={duration_ms:.1f}",
        "Cache-Control": "no-store, no-cache, must-revalidate",
        "Timing-Allow-Origin": "*",
        "Access-Control-Allow-Origin": "*",
    }


def api_json(body, duration_ms: float, status_code: int = 200) -> JSONResponse:
    return JSONResponse(body, status_code=status_code, headers=api_headers(duration_ms))


# ---------------------------------------------------------------------------
# Pure endpoint logic (importable by test_app.py for checksum verification)
# ---------------------------------------------------------------------------

USER_SORT_FIELDS = ("id", "name", "email", "score", "created_at")


def compute_users(page: int, sort: str, order: str) -> list:
    """Spec §5.4: 100-user window, stable sort, first 20, bare array."""
    page = max(1, page)
    start = (page - 1) * 100
    window = USERS[start:start + 100]
    field = sort if sort in USER_SORT_FIELDS else "id"
    # Python's sort is stable; reverse=True keeps ties in dataset order,
    # matching the family C reference (comparator reversal, not list reversal).
    return sorted(window, key=lambda u: u[field], reverse=(order == "desc"))[:20]


def compute_transform(body: dict) -> dict:
    """Spec §5.5: SHA-256 each field string, reverse values."""
    seed = body.get("seed", 0)
    if seed is None:
        seed = 0
    fields = body.get("fields") or []
    values = body.get("values") or []
    hashed = [hashlib.sha256(str(f).encode()).hexdigest() for f in fields]
    return {"seed": seed, "hashed_fields": hashed, "reversed_values": list(reversed(values))}


def compute_aggregate() -> dict:
    """Spec §5.6: normative float64 algorithm, range always ignored."""
    values = sorted(TS_VALUES)
    n = len(values)
    total = 0.0
    for v in values:  # sequential sum over the SORTED values (normative)
        total += v
    chunk = n // 5
    categories = []
    for i in range(5):
        part = values[i * chunk:(i + 1) * chunk]
        s = 0.0
        for v in part:
            s += v
        categories.append({
            "category": f"q{i + 1}",
            "count": chunk,
            "mean": r2(s / chunk),
            "min": r2(part[0]),
            "max": r2(part[-1]),
        })
    return {
        "total_points": n,
        "mean": r2(total / n),
        "p50": r2(values[int(n * 0.50)]),
        "p95": r2(values[int(n * 0.95)]),
        "max": r2(values[-1]),
        "categories": categories,
    }


def compute_search(query: str, limit: int) -> dict:
    """Spec §5.7: case-sensitive regex, literal fallback, positional ranking."""
    limit = min(limit, 100)
    try:
        pattern = re.compile(query)

        def find(item):
            m = pattern.search(item)
            return m.start() if m else None
    except re.error:
        def find(item):
            pos = item.find(query)
            return pos if pos >= 0 else None

    matches = []
    for item in SEARCH_CORPUS:
        pos = find(item)
        if pos is not None:
            matches.append((pos, item))
    matches.sort(key=lambda t: (t[0], t[1]))

    results = [
        {"rank": i + 1, "item": item, "match_position": pos}
        for i, (pos, item) in enumerate(matches[:max(limit, 0)])
    ]
    return {
        "query": query,
        "total_matches": len(matches),  # counted BEFORE truncation
        "returned": len(results),
        "results": results,
    }


def compute_upload_process(body: bytes) -> dict:
    """Spec §5.8: CRC-32 + SHA-256 + zlib (RFC 1950) level 6."""
    return {
        "original_size": len(body),
        "compressed_size": len(zlib.compress(body, 6)),
        "crc32": format(zlib.crc32(body) & 0xFFFFFFFF, "08x"),
        "sha256": hashlib.sha256(body).hexdigest(),
    }


# ---------------------------------------------------------------------------
# HTTP handlers
# ---------------------------------------------------------------------------

async def health(request: Request) -> Response:
    """GET /health — constant-work byte-constant body (spec §5.1)."""
    return Response(content=HEALTH_BODY, media_type="application/json")


async def download(request: Request) -> Response:
    """GET /download/{size} — 0x42 in 8 KiB chunks (spec §5.2)."""
    t0 = time.perf_counter()
    size_str = request.path_params.get("size", "")
    if not size_str.isdigit():  # non-integer (incl. negative/empty) → 400
        return JSONResponse({"error": "invalid size"}, status_code=400)
    size = min(int(size_str), DOWNLOAD_CAP)  # clamp above cap; 0 is valid

    async def generate():
        remaining = size
        while remaining > 0:
            to_send = min(remaining, CHUNK_SIZE)
            yield CHUNK[:to_send]
            remaining -= to_send

    proc_ms = (time.perf_counter() - t0) * 1000
    return StreamingResponse(
        generate(),
        media_type="application/octet-stream",
        headers={
            "Content-Length": str(size),
            "X-Download-Bytes": str(size),
            "Server-Timing": f"proc;dur={proc_ms:.1f}",
        },
    )


async def upload(request: Request) -> JSONResponse:
    """POST /upload — drain body without buffering wholesale (spec §5.3)."""
    t0 = time.perf_counter()
    total = 0
    async for chunk in request.stream():
        total += len(chunk)
    recv_ms = (time.perf_counter() - t0) * 1000
    headers = {
        "X-Networker-Received-Bytes": str(total),
        "Server-Timing": f"recv;dur={recv_ms:.1f}",
    }
    request_id = request.headers.get("x-networker-request-id")
    if request_id is not None:
        headers["X-Networker-Request-Id"] = request_id
    return JSONResponse({"received_bytes": total}, headers=headers)


async def api_users(request: Request) -> JSONResponse:
    """GET /api/users?page=N&sort=<field>&order=<asc|desc> (spec §5.4)."""
    t0 = time.perf_counter()
    page = _int_param(request, "page", 1)
    sort_field = request.query_params.get("sort", "id")
    order = request.query_params.get("order", "asc")
    body = compute_users(page, sort_field, order)
    return api_json(body, (time.perf_counter() - t0) * 1000)


async def api_transform(request: Request) -> JSONResponse:
    """POST /api/transform (spec §5.5). Invalid JSON → 400."""
    t0 = time.perf_counter()
    try:
        body = await request.json()
    except Exception:
        return api_json({"error": "invalid JSON"},
                        (time.perf_counter() - t0) * 1000, status_code=400)
    if not isinstance(body, dict):
        return api_json({"error": "invalid JSON"},
                        (time.perf_counter() - t0) * 1000, status_code=400)
    return api_json(compute_transform(body), (time.perf_counter() - t0) * 1000)


async def api_aggregate(request: Request) -> JSONResponse:
    """GET /api/aggregate — `range` accepted and ignored (spec §5.6)."""
    t0 = time.perf_counter()
    return api_json(compute_aggregate(), (time.perf_counter() - t0) * 1000)


async def api_search(request: Request) -> JSONResponse:
    """GET /api/search?q=<term>&limit=N (spec §5.7)."""
    t0 = time.perf_counter()
    query = request.query_params.get("q", "test")
    limit = _int_param(request, "limit", 20)
    return api_json(compute_search(query, limit), (time.perf_counter() - t0) * 1000)


async def api_upload_process(request: Request) -> JSONResponse:
    """POST /api/upload/process (spec §5.8)."""
    t0 = time.perf_counter()
    body = b"".join([chunk async for chunk in request.stream()])
    return api_json(compute_upload_process(body), (time.perf_counter() - t0) * 1000)


async def api_delayed(request: Request) -> JSONResponse:
    """GET /api/delayed?ms=N — async sleep, ms clamped to [1,100] (spec §5.9)."""
    t0 = time.perf_counter()
    ms = max(1, min(100, _int_param(request, "ms", 10)))
    # `work` is reserved: accepted and ignored.
    await asyncio.sleep(ms / 1000)
    actual_ms = (time.perf_counter() - t0) * 1000
    return api_json({"requested_ms": ms, "actual_ms": round(actual_ms, 2)}, actual_ms)


async def api_validate(request: Request) -> JSONResponse:
    """GET /api/validate?seed=N — echo dataset checksums (spec §5.10)."""
    t0 = time.perf_counter()
    seed = _int_param(request, "seed", 42)
    return api_json({"seed": seed, "checksums": EXPECTED_CHECKSUMS},
                    (time.perf_counter() - t0) * 1000)


class AuthMiddleware:
    """BENCH_API_TOKEN bearer auth on every route except /health (spec §1)."""

    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] == "http" and BENCH_API_TOKEN:
            path = scope.get("path", "")
            if path != "/health":
                headers = dict(scope.get("headers", []))
                auth = headers.get(b"authorization", b"").decode()
                if not auth.startswith("Bearer ") or auth[7:] != BENCH_API_TOKEN:
                    body = b'{"error":"unauthorized"}'
                    await send({
                        "type": "http.response.start",
                        "status": 401,
                        "headers": [
                            (b"content-type", b"application/json"),
                            (b"content-length", str(len(body)).encode()),
                        ],
                    })
                    await send({"type": "http.response.body", "body": body})
                    return
        await self.app(scope, receive, send)


app = AuthMiddleware(
    Starlette(
        routes=[
            Route("/health", health, methods=["GET"]),
            Route("/download/{size}", download, methods=["GET"]),
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
)
