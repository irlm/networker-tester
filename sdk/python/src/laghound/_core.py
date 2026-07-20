"""Framework-agnostic core of the LagHound Python SDK.

Implements LagHound SDK endpoint contract v1 (docs/sdk/contract-v1.md,
machine-readable twin shared/sdk-contract-v1.json). The ASGI and WSGI
adapters (`laghound._asgi`, `laghound._wsgi`) translate their protocol's
request shape into `LagHoundCore.handle(...)` calls and stream the returned
response.

Design notes (contract §6 — production safety):

- Zero runtime dependencies; stdlib only.
- Constant-time token comparison: candidate and expected tokens are hashed
  (SHA-256) and the digests compared with ``hmac.compare_digest`` — length
  mismatches cannot short-circuit observably (contract §5).
- Streamed downloads: chunks are sliced from one per-process read-only fill
  buffer; per-request allocation is O(chunk), never O(N).
- Limiter state (token buckets, concurrency counters, byte budget) is guarded
  by a single ``threading.Lock``. Critical sections are tiny and never block
  on I/O, so the same core is safe under sync WSGI worker threads and under
  asyncio event loops (no ``await`` ever happens while the lock is held).
  All caps are per-process: with k workers the effective fleet-facing cap is
  k × the configured value.
- Zero logging: this module never logs. Nothing here imports ``logging``.
- Zero reflection: no request input is ever copied into a response body or
  error message.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import math
import os
import re
import threading
import time
from collections import OrderedDict, deque

from ._version import __version__

CONTRACT = "v1"
SDK_LANG = "python"

DEFAULT_PREFIX = "/laghound"
ABSOLUTE_MAX_BYTES = 33554432  # 32 MiB — hard cap, config cannot exceed
DEFAULT_CAP_BYTES = 4194304  # 4 MiB
ECHO_BODY_MAX_BYTES = 65536  # /echo request bodies over this -> 413
CHUNK_BYTES = 65536  # max streamed chunk size
TOKEN_MIN_BYTES = 16
ROTATION_MAX_TOKENS = 2
PER_IP_TABLE_MAX_ENTRIES = 10000
KILL_SWITCH_ENV = "LAGHOUND_DISABLED"
TOKEN_ENV = "LAGHOUND_TOKEN"
KILL_SWITCH_CACHE_S = 1.0

CACHE_CONTROL = "no-store, no-cache, must-revalidate"
TIMING_ALLOW_ORIGIN = "*"

# Single per-process read-only fill buffer (0x42 == 'B', matching
# networker-endpoint's DOWNLOAD_FILL). Downloads slice from this; the only
# per-request copy is the final partial chunk (<= 64 KiB).
_FILL = b"\x42" * CHUNK_BYTES

_ECHO_BODY = b'{"contract":"v1","ok":true}'

_MARK_NAME_RE = re.compile(r"\A[a-z0-9]{1,24}\Z")
_BYTES_PARAM_RE = re.compile(r"\A[0-9]+\Z")

_REASONS = {
    200: "OK",
    400: "Bad Request",
    404: "Not Found",
    405: "Method Not Allowed",
    413: "Payload Too Large",
    429: "Too Many Requests",
    500: "Internal Server Error",
}

_ERROR_MESSAGES = {
    "invalid_param": "invalid query parameter",
    "method_not_allowed": "method not allowed",
    "payload_too_large": "payload too large",
    "rate_limited": "rate limit exceeded",
    "internal": "internal error",
}


class LagHoundConfigError(ValueError):
    """Raised at init for invalid configuration (fail-closed: refuse to mount)."""


def reason_phrase(status):
    return _REASONS.get(status, "Unknown")


def _fmt_dur(ms):
    """Format a Server-Timing dur value: ms, <= 3 decimal places."""
    if ms < 0:
        ms = 0.0
    return format(ms, ".3f")


def build_server_timing(pairs):
    """pairs: iterable of (name, dur_ms). Enforces contract limits: <= 8
    metrics, header value <= 512 bytes (excess entries are dropped)."""
    parts = []
    size = 0
    for name, ms in pairs:
        if len(parts) >= 8:
            break
        part = "%s;dur=%s" % (name, _fmt_dur(ms))
        add = len(part) + (2 if parts else 0)
        if size + add > 512:
            break
        parts.append(part)
        size += add
    return ", ".join(parts)


class Response:
    """A fully-determined response the adapters can stream.

    - ``headers``: list of (name, value) str pairs.
    - ``chunks``: iterable of bytes objects (each <= 64 KiB).
    - ``on_close``: idempotent callable the adapter MUST invoke once the body
      is fully sent (or the request is abandoned) — it releases concurrency
      slots. WSGI wraps it in the iterable's ``close()``; ASGI calls it in a
      ``finally``.
    - ``bare``: True for the invisible 404 (no LagHound headers at all).
    - ``close_connection``: hint that the transport should not be reused
      (chunked upload over cap).
    """

    __slots__ = ("status", "headers", "chunks", "on_close", "bare", "close_connection")

    def __init__(self, status, headers, chunks, on_close=None, bare=False, close_connection=False):
        self.status = status
        self.headers = headers
        self.chunks = chunks
        self.on_close = on_close or (lambda: None)
        self.bare = bare
        self.close_connection = close_connection


class UploadIntent:
    """Returned by ``handle`` for an authorized POST /upload. The adapter
    drains the body itself (protocol-specific), then calls ``finish``.

    - ``cap``: drain at most this many bytes (+1 to detect overflow when the
      length is unknown).
    - ``content_length``: parsed Content-Length if present (already known to
      be <= cap, otherwise ``handle`` would have returned 413).
    - ``finish(received, truncated, recv_ms)`` -> Response.
    - ``abort()``: release slots if draining fails; idempotent.
    """

    __slots__ = ("_core", "cap", "content_length", "_release", "_t0")

    def __init__(self, core, cap, content_length, release, t0):
        self._core = core
        self.cap = cap
        self.content_length = content_length
        self._release = release
        self._t0 = t0

    def finish(self, received, truncated, recv_ms):
        t_app0 = time.perf_counter()
        core = self._core
        if truncated:
            resp = core._envelope(
                413, "payload_too_large", self._release, close_connection=True
            )
            return resp
        core._budget_charge(received)
        body = ('{"contract":"v1","received_bytes":%d}' % received).encode("ascii")
        app_ms = (time.perf_counter() - t_app0) * 1000.0
        st = build_server_timing(
            [("recv", recv_ms), ("app", app_ms), ("total", recv_ms + app_ms)]
        )
        headers = [
            ("Content-Type", "application/json"),
            ("Content-Length", str(len(body))),
            ("X-LagHound-Bytes", str(received)),
            ("Server-Timing", st),
            ("Cache-Control", CACHE_CONTROL),
            ("Timing-Allow-Origin", TIMING_ALLOW_ORIGIN),
        ]
        return Response(200, headers, [body], on_close=self._release)

    def abort(self):
        self._release()


class _TokenBucket:
    __slots__ = ("rate", "burst", "tokens", "ts")

    def __init__(self, rate, burst, now):
        self.rate = float(rate)
        self.burst = float(burst)
        self.tokens = float(burst)
        self.ts = now

    def allow(self, now):
        elapsed = now - self.ts
        if elapsed > 0:
            self.tokens = min(self.burst, self.tokens + elapsed * self.rate)
            self.ts = now
        if self.tokens >= 1.0:
            self.tokens -= 1.0
            return True
        return False

    def retry_after_s(self):
        need = 1.0 - self.tokens
        return max(1, int(math.ceil(need / self.rate)))


class _ByteBudget:
    """Sliding-window transfer byte budget (contract §6.4)."""

    __slots__ = ("budget", "window_s", "entries", "spent")

    def __init__(self, budget, window_s):
        self.budget = int(budget)
        self.window_s = float(window_s)
        self.entries = deque()  # (monotonic_ts, bytes)
        self.spent = 0

    def _prune(self, now):
        cutoff = now - self.window_s
        entries = self.entries
        while entries and entries[0][0] <= cutoff:
            _, n = entries.popleft()
            self.spent -= n

    def check(self, now):
        """Return None if a transfer may proceed, else Retry-After seconds."""
        self._prune(now)
        if self.spent < self.budget:
            return None
        if self.entries:
            remain = self.entries[0][0] + self.window_s - now
            return max(1, int(math.ceil(remain)))
        return max(1, int(math.ceil(self.window_s)))

    def charge(self, now, n):
        if n > 0:
            self.entries.append((now, n))
            self.spent += n


def _norm_rate(value, name):
    if isinstance(value, dict):
        try:
            rps, burst = value["rps"], value["burst"]
        except KeyError:
            raise LagHoundConfigError("%s must provide rps and burst" % name)
    else:
        try:
            rps, burst = value
        except (TypeError, ValueError):
            raise LagHoundConfigError("%s must be (rps, burst) or {'rps':..,'burst':..}" % name)
    rps = float(rps)
    burst = float(burst)
    if rps <= 0 or burst < 1:
        raise LagHoundConfigError("%s must have rps > 0 and burst >= 1" % name)
    return rps, burst


class LagHoundCore:
    def __init__(
        self,
        token=None,
        tokens=None,
        prefix=DEFAULT_PREFIX,
        download_cap_bytes=DEFAULT_CAP_BYTES,
        upload_cap_bytes=DEFAULT_CAP_BYTES,
        rate_per_ip=(10, 20),
        rate_global=(50, 100),
        max_concurrent=8,
        max_concurrent_transfers=2,
        byte_budget=None,
        app_name=None,
        enable_echo=True,
        enable_download=True,
        enable_upload=True,
        enable_info=True,
        trusted_proxies=None,
    ):
        # --- token (fail-closed: refuse to mount without one, contract §2)
        token_list = []
        if tokens is not None:
            token_list = [t for t in tokens if t]
        elif token:
            token_list = [token]
        else:
            env_token = os.environ.get(TOKEN_ENV)
            if env_token:
                token_list = [env_token]
        if not token_list:
            raise LagHoundConfigError(
                "LagHound requires a token (token=..., tokens=[...], or the "
                "LAGHOUND_TOKEN environment variable); refusing to mount open routes"
            )
        if len(token_list) > ROTATION_MAX_TOKENS:
            raise LagHoundConfigError(
                "at most %d tokens (current + previous) are accepted" % ROTATION_MAX_TOKENS
            )
        for t in token_list:
            if not isinstance(t, str) or len(t.encode("utf-8")) < TOKEN_MIN_BYTES:
                raise LagHoundConfigError(
                    "each token must be a string of at least %d bytes" % TOKEN_MIN_BYTES
                )
        # Constant-time auth: compare SHA-256 digests of candidate vs expected
        # (hashed representation — length mismatch cannot short-circuit).
        self._token_digests = tuple(
            hashlib.sha256(t.encode("utf-8")).digest() for t in token_list
        )

        # --- prefix
        if not isinstance(prefix, str) or not prefix.startswith("/") or (
            len(prefix) > 1 and prefix.endswith("/")
        ):
            raise LagHoundConfigError(
                "prefix must start with '/' and have no trailing slash"
            )
        self.prefix = prefix

        # --- caps (clamped to the absolute max, never rejected)
        self.download_cap_bytes = min(int(download_cap_bytes), ABSOLUTE_MAX_BYTES)
        self.upload_cap_bytes = min(int(upload_cap_bytes), ABSOLUTE_MAX_BYTES)
        if self.download_cap_bytes < 0 or self.upload_cap_bytes < 0:
            raise LagHoundConfigError("byte caps must be non-negative")

        # --- limits
        self._rate_per_ip = _norm_rate(rate_per_ip, "rate_per_ip")
        self._rate_global = _norm_rate(rate_global, "rate_global")
        self.max_concurrent = int(max_concurrent)
        self.max_concurrent_transfers = int(max_concurrent_transfers)
        if self.max_concurrent < 1 or self.max_concurrent_transfers < 0:
            raise LagHoundConfigError("invalid concurrency caps")
        if byte_budget is not None:
            try:
                self._budget = _ByteBudget(byte_budget["bytes"], byte_budget["window_s"])
            except (TypeError, KeyError):
                raise LagHoundConfigError(
                    "byte_budget must be {'bytes': <int>, 'window_s': <int>}"
                )
        else:
            self._budget = None
        self._byte_budget_cfg = (
            {"bytes": self._budget.budget, "window_s": int(self._budget.window_s)}
            if self._budget
            else None
        )

        self.app_name = app_name if app_name else None
        self.trusted_proxies = frozenset(trusted_proxies or ())

        self.routes_enabled = {
            "health": True,  # always true while mounted+enabled (contract §3.1)
            "echo": bool(enable_echo),
            "download": bool(enable_download),
            "upload": bool(enable_upload),
            "info": bool(enable_info),
        }

        # --- limiter state (single lock; see module docstring)
        self._lock = threading.Lock()
        self._global_bucket = _TokenBucket(*self._rate_global, now=time.monotonic())
        self._ip_buckets = OrderedDict()  # ip -> _TokenBucket, LRU-capped
        self._inflight = 0
        self._inflight_transfers = 0

        # --- kill switch cache (<= 1 s, contract §6.5)
        self._ks_value = None
        self._ks_read_at = -math.inf

        self._start = time.monotonic()

        # --- precomputed /health body parts (O(1) per request except uptime_s)
        sdk_json = '{"lang":"%s","version":"%s"}' % (SDK_LANG, __version__)
        app_part = (
            ',"app":%s' % json.dumps(self.app_name) if self.app_name else ""
        )
        routes_json = json.dumps(self.routes_enabled, separators=(",", ":"))
        self._health_pre = (
            '{"contract":"v1","status":"ok","sdk":%s%s,"uptime_s":' % (sdk_json, app_part)
        ).encode("utf-8")
        self._health_post = (',"routes":%s}' % routes_json).encode("utf-8")

    # ------------------------------------------------------------------ paths

    def resolve(self, path):
        """Return the LagHound subpath for ``path`` if it is under the prefix,
        else None."""
        p = self.prefix
        if path == p:
            return "/"
        if path.startswith(p) and len(path) > len(p) and path[len(p)] == "/":
            return path[len(p):]
        return None

    # ------------------------------------------------------------- kill switch

    def _disabled(self):
        now = time.monotonic()
        if now - self._ks_read_at > KILL_SWITCH_CACHE_S:
            self._ks_value = os.environ.get(KILL_SWITCH_ENV) == "1"
            self._ks_read_at = now
        return self._ks_value

    # -------------------------------------------------------------------- auth

    def _authenticated(self, headers):
        candidate = headers.get("x-laghound-token")
        if candidate is None:
            auth = headers.get("authorization")
            if auth is not None and auth[:7].lower() == "bearer ":
                candidate = auth[7:]
        if candidate is None:
            return False
        digest = hashlib.sha256(candidate.encode("utf-8", "surrogateescape")).digest()
        ok = False
        for expected in self._token_digests:  # no early exit across tokens
            if hmac.compare_digest(digest, expected):
                ok = True
        return ok

    # --------------------------------------------------------------- client ip

    def client_ip(self, peer_ip, headers):
        """Peer IP unless it is an explicitly trusted proxy (contract §6.2:
        never trust X-Forwarded-For by default)."""
        if self.trusted_proxies and peer_ip in self.trusted_proxies:
            xff = headers.get("x-forwarded-for")
            if xff:
                hops = [h.strip() for h in xff.split(",") if h.strip()]
                for hop in reversed(hops):
                    if hop not in self.trusted_proxies:
                        return hop
        return peer_ip or ""

    # ------------------------------------------------------------------ limits

    def _rate_check(self, ip, now):
        """Return None if allowed, else Retry-After seconds."""
        with self._lock:
            buckets = self._ip_buckets
            bucket = buckets.get(ip)
            if bucket is None:
                bucket = _TokenBucket(*self._rate_per_ip, now=now)
                buckets[ip] = bucket
                if len(buckets) > PER_IP_TABLE_MAX_ENTRIES:
                    buckets.popitem(last=False)  # LRU eviction, bounded memory
            else:
                buckets.move_to_end(ip)
            ip_ok = bucket.allow(now)
            global_ok = self._global_bucket.allow(now)
            if ip_ok and global_ok:
                return None
            retry = 1
            if not ip_ok:
                retry = max(retry, bucket.retry_after_s())
            if not global_ok:
                retry = max(retry, self._global_bucket.retry_after_s())
            return retry

    def _acquire_slots(self, transfer):
        """Try to acquire concurrency slot(s). Returns an idempotent release
        callable, or None if the cap is hit."""
        with self._lock:
            if self._inflight >= self.max_concurrent:
                return None
            if transfer and self._inflight_transfers >= self.max_concurrent_transfers:
                return None
            self._inflight += 1
            if transfer:
                self._inflight_transfers += 1
        released = []

        def release():
            if released:
                return
            released.append(True)
            with self._lock:
                self._inflight -= 1
                if transfer:
                    self._inflight_transfers -= 1

        return release

    def _budget_check(self):
        if self._budget is None:
            return None
        with self._lock:
            return self._budget.check(time.monotonic())

    def _budget_charge(self, n):
        if self._budget is None:
            return
        with self._lock:
            self._budget.charge(time.monotonic(), n)

    # -------------------------------------------------------------- responses

    @staticmethod
    def bare_404():
        """The invisible 404: no body, no LagHound headers (contract §5)."""
        return Response(404, [("Content-Length", "0")], [], bare=True)

    def _envelope(self, status, code, release=None, retry_after_s=None, close_connection=False):
        err = {"code": code, "message": _ERROR_MESSAGES[code]}
        headers_extra = []
        if retry_after_s is not None:
            err["retry_after_ms"] = int(retry_after_s) * 1000
            headers_extra.append(("Retry-After", str(int(retry_after_s))))
        body = json.dumps(
            {"contract": CONTRACT, "error": err}, separators=(",", ":")
        ).encode("utf-8")
        headers = [
            ("Content-Type", "application/json"),
            ("Content-Length", str(len(body))),
            ("Server-Timing", build_server_timing([("app", 0.0), ("total", 0.0)])),
            ("Cache-Control", CACHE_CONTROL),
            ("Timing-Allow-Origin", TIMING_ALLOW_ORIGIN),
        ] + headers_extra
        return Response(
            status, headers, [body], on_close=release, close_connection=close_connection
        )

    def internal_error_response(self, release=None):
        return self._envelope(500, "internal", release)

    def _json_response(self, body, t0, release=None, extra_headers=()):
        app_ms = (time.perf_counter() - t0) * 1000.0
        st = build_server_timing([("app", app_ms), ("total", app_ms)])
        headers = [
            ("Content-Type", "application/json"),
            ("Content-Length", str(len(body))),
            ("Server-Timing", st),
            ("Cache-Control", CACHE_CONTROL),
            ("Timing-Allow-Origin", TIMING_ALLOW_ORIGIN),
        ] + list(extra_headers)
        return Response(200, headers, [body], on_close=release)

    # ---------------------------------------------------------------- handling

    def handle(self, method, subpath, query_string, headers, peer_ip):
        """Main entry. ``headers`` is a dict with lower-case keys.

        Returns a Response, or an UploadIntent for an authorized upload.
        Check order per contract §5: kill switch -> rate/concurrency limits ->
        auth -> route logic.
        """
        t0 = time.perf_counter()

        # 1. kill switch
        if self._disabled():
            return self.bare_404()

        ip = self.client_ip(peer_ip, headers)
        is_transfer = subpath in ("/download", "/upload")

        # 2a. rate limits (before auth — brute forcing is throttled too)
        retry = self._rate_check(ip, time.monotonic())
        if retry is not None:
            if self._authenticated(headers):
                return self._envelope(429, "rate_limited", retry_after_s=retry)
            return self.bare_404()  # unauthenticated limiter hits stay invisible

        # 2b. concurrency caps
        release = self._acquire_slots(is_transfer)
        if release is None:
            if self._authenticated(headers):
                return self._envelope(429, "rate_limited", retry_after_s=1)
            return self.bare_404()

        try:
            # 3. auth
            if not self._authenticated(headers):
                release()
                return self.bare_404()

            # 4. route logic
            return self._dispatch(method, subpath, query_string, headers, release, t0)
        except Exception:
            # §6.7: a failure inside LagHound never crashes the host process.
            return self.internal_error_response(release)

    def _dispatch(self, method, subpath, query_string, headers, release, t0):
        enabled = self.routes_enabled
        if subpath == "/health":
            if method != "GET":
                return self._envelope(405, "method_not_allowed", release)
            return self._handle_health(t0, release)
        if subpath == "/echo" and enabled["echo"]:
            if method != "GET":
                return self._envelope(405, "method_not_allowed", release)
            return self._handle_echo(headers, t0, release)
        if subpath == "/download" and enabled["download"]:
            if method != "GET":
                return self._envelope(405, "method_not_allowed", release)
            return self._handle_download(query_string, t0, release)
        if subpath == "/upload" and enabled["upload"]:
            if method != "POST":
                return self._envelope(405, "method_not_allowed", release)
            return self._handle_upload(headers, release, t0)
        if subpath == "/info" and enabled["info"]:
            if method != "GET":
                return self._envelope(405, "method_not_allowed", release)
            return self._handle_info(t0, release)
        # Unknown or disabled subpath under the prefix: invisible.
        release()
        return self.bare_404()

    # ----------------------------------------------------------------- routes

    def _handle_health(self, t0, release):
        uptime = int(time.monotonic() - self._start)
        body = b"%s%d%s" % (self._health_pre, uptime, self._health_post)
        return self._json_response(body, t0, release)

    def _handle_echo(self, headers, t0, release):
        cl = _parse_content_length(headers)
        if cl is not None and cl > ECHO_BODY_MAX_BYTES:
            return self._envelope(413, "payload_too_large", release)
        # Fixed, byte-constant body; request input is never reflected.
        return self._json_response(_ECHO_BODY, t0, release)

    def _handle_download(self, query_string, t0, release):
        requested = _parse_bytes_param(query_string)
        if requested is _INVALID:
            return self._envelope(400, "invalid_param", release)
        if requested is None:
            requested = DEFAULT_CAP_BYTES
        effective = min(requested, self.download_cap_bytes, ABSOLUTE_MAX_BYTES)

        retry = self._budget_check()
        if retry is not None:
            return self._envelope(429, "rate_limited", release, retry_after_s=retry)
        self._budget_charge(effective)

        # app;dur covers setup only — measured before the first chunk.
        app_ms = (time.perf_counter() - t0) * 1000.0
        st = build_server_timing([("app", app_ms), ("total", app_ms)])
        headers = [
            ("Content-Type", "application/octet-stream"),
            ("Content-Length", str(effective)),
            ("X-LagHound-Bytes", str(effective)),
            ("Server-Timing", st),
            ("Cache-Control", CACHE_CONTROL),
            ("Timing-Allow-Origin", TIMING_ALLOW_ORIGIN),
        ]
        return Response(200, headers, _fill_chunks(effective), on_close=release)

    def _handle_upload(self, headers, release, t0):
        cap = min(self.upload_cap_bytes, ABSOLUTE_MAX_BYTES)
        cl = _parse_content_length(headers)
        if cl is not None and cl > cap:
            # 413 without reading the body (contract §3.4).
            return self._envelope(413, "payload_too_large", release)
        retry = self._budget_check()
        if retry is not None:
            return self._envelope(429, "rate_limited", release, retry_after_s=retry)
        return UploadIntent(self, cap, cl, release, t0)

    def _handle_info(self, t0, release):
        # Config echo minus secrets: only the boolean token_set, never the
        # token or any derivative (contract §3.5).
        info = {
            "contract": CONTRACT,
            "sdk": {"lang": SDK_LANG, "version": __version__},
            "prefix": self.prefix,
            "uptime_s": int(time.monotonic() - self._start),
            "token_set": True,
            "caps": {
                "download_bytes": self.download_cap_bytes,
                "upload_bytes": self.upload_cap_bytes,
                "absolute_max_bytes": ABSOLUTE_MAX_BYTES,
            },
            "limits": {
                "rate_per_ip": {
                    "rps": self._rate_per_ip[0],
                    "burst": self._rate_per_ip[1],
                },
                "rate_global": {
                    "rps": self._rate_global[0],
                    "burst": self._rate_global[1],
                },
                "max_concurrent": self.max_concurrent,
                "max_concurrent_transfers": self.max_concurrent_transfers,
                "byte_budget": self._byte_budget_cfg,
            },
            "routes": self.routes_enabled,
        }
        if self.app_name:
            info["app"] = self.app_name
        body = json.dumps(info, separators=(",", ":")).encode("utf-8")
        return self._json_response(body, t0, release)


_INVALID = object()


def _parse_bytes_param(query_string):
    """Parse ``bytes=N`` from a raw query string. Returns int, None (absent),
    or _INVALID (unparsable/negative -> 400, contract §3.3)."""
    if not query_string:
        return None
    value = None
    for pair in query_string.split("&"):
        if pair.startswith("bytes="):
            value = pair[6:]
            break
        if pair == "bytes":
            value = ""
            break
    if value is None:
        return None
    if not _BYTES_PARAM_RE.match(value):
        return _INVALID
    try:
        return int(value)
    except ValueError:  # pragma: no cover - regex already guarantees digits
        return _INVALID


def _parse_content_length(headers):
    raw = headers.get("content-length")
    if raw is None or raw == "":
        return None
    try:
        n = int(raw)
    except (TypeError, ValueError):
        return None
    return n if n >= 0 else None


def _fill_chunks(total):
    """Stream ``total`` fill bytes in chunks <= 64 KiB from the shared
    per-process buffer. O(chunk) memory, never O(total)."""
    full, rest = divmod(total, CHUNK_BYTES)
    for _ in range(full):
        yield _FILL
    if rest:
        yield _FILL[:rest]


def merge_marks_into_server_timing(existing_value, marks):
    """Merge host-app marks (from laghound.mark) into a Server-Timing header
    value. ``marks`` is a list of (name, dur_ms) with names already validated.
    Enforces <= 8 metrics / <= 512 bytes across the combined header."""
    pairs = []
    if existing_value:
        # Count existing metrics without re-parsing durations.
        base = existing_value.strip()
    else:
        base = ""
    existing_count = base.count(",") + 1 if base else 0
    size = len(base)
    parts = []
    for name, ms in marks:
        if existing_count + len(parts) >= 8:
            break
        part = "mark-%s;dur=%s" % (name, _fmt_dur(ms))
        add = len(part) + (2 if (base or parts) else 0)
        if size + add > 512:
            break
        parts.append(part)
        size += add
    if not parts:
        return base or None
    if base:
        return base + ", " + ", ".join(parts)
    return ", ".join(parts)


def validate_mark(name, dur_ms):
    """Validate a host-app mark; returns (name, float_ms) or None."""
    if not isinstance(name, str) or not _MARK_NAME_RE.match(name):
        return None
    try:
        ms = float(dur_ms)
    except (TypeError, ValueError):
        return None
    if not math.isfinite(ms) or ms < 0:
        return None
    return (name, ms)
