"""Pure WSGI adapter for the LagHound endpoint (contract v1).

Works two ways:

- Middleware: ``app.wsgi_app = laghound.wsgi(app.wsgi_app, token=...)``
  (Flask) or ``application = laghound.wsgi(get_wsgi_application(), token=...)``
  (Django ``wsgi.py``). Requests under ``prefix`` are handled; everything else
  passes through untouched (plus optional ``laghound.mark`` marks on host
  responses).
- Standalone app: serve the object returned by ``laghound.wsgi(token=...)``
  directly (no inner app); non-route paths get the bare 404.

Concurrency model: WSGI servers run one request per worker thread; the shared
core's limiter state is guarded by a ``threading.Lock``, so per-IP/global rate
buckets, the concurrency counters, and the byte budget are consistent across
all threads in the process. Concurrency slots are released via the response
iterable's ``close()``, which every compliant WSGI server calls even on client
disconnect. Caps are per-process.
"""

from __future__ import annotations

import time

from . import _marks
from ._core import (
    CHUNK_BYTES,
    LagHoundCore,
    UploadIntent,
    merge_marks_into_server_timing,
    reason_phrase,
)


class _ClosingIterable:
    """Wraps response chunks so the WSGI server's close() releases slots."""

    __slots__ = ("_chunks", "_on_close")

    def __init__(self, chunks, on_close):
        self._chunks = iter(chunks)
        self._on_close = on_close

    def __iter__(self):
        return self

    def __next__(self):
        return next(self._chunks)

    def close(self):
        try:
            close = getattr(self._chunks, "close", None)
            if close is not None:
                close()
        finally:
            self._on_close()


class LagHoundWSGIMiddleware:
    def __init__(self, app=None, **config):
        self.app = app
        self.core = LagHoundCore(**config)

    def __call__(self, environ, start_response):
        path = environ.get("PATH_INFO", "")
        subpath = self.core.resolve(path)
        if subpath is None:
            if self.app is not None:
                return self._passthrough(environ, start_response)
            # Standalone: treat the raw path as the subpath (mounting servers
            # strip the prefix into SCRIPT_NAME).
            subpath = path

        return self._handle(environ, start_response, subpath)

    # ------------------------------------------------------------ passthrough

    def _passthrough(self, environ, start_response):
        token = _marks.open_request()

        def start_response_with_marks(status, headers, exc_info=None):
            marks = _marks.current_marks()
            if marks:
                headers = _merge_mark_headers(list(headers), marks)
            if exc_info is not None:
                return start_response(status, headers, exc_info)
            return start_response(status, headers)

        try:
            return self.app(environ, start_response_with_marks)
        finally:
            # Marks recorded during response iteration are dropped by design
            # (headers are already sent).
            _marks.close_request(token)

    # ---------------------------------------------------------------- laghound

    def _handle(self, environ, start_response, subpath):
        core = self.core
        try:
            headers = _header_dict(environ)
            result = core.handle(
                environ.get("REQUEST_METHOD", "GET"),
                subpath,
                environ.get("QUERY_STRING", ""),
                headers,
                environ.get("REMOTE_ADDR", ""),
            )
            if isinstance(result, UploadIntent):
                result = self._drain_upload(environ, result)
        except Exception:
            result = core.internal_error_response()

        status_line = "%d %s" % (result.status, reason_phrase(result.status))
        headers = list(result.headers)
        if result.close_connection:
            headers.append(("Connection", "close"))
        start_response(status_line, headers)
        return _ClosingIterable(result.chunks, result.on_close)

    def _drain_upload(self, environ, intent):
        """Drain-and-count, never buffer: peak memory O(chunk) (contract §3.4)."""
        stream = environ.get("wsgi.input")
        received = 0
        truncated = False
        t0 = time.perf_counter()
        try:
            if stream is not None:
                if intent.content_length is not None:
                    # Known length, already <= cap. Never read past it (PEP 3333).
                    remaining = intent.content_length
                    while remaining > 0:
                        chunk = stream.read(min(CHUNK_BYTES, remaining))
                        if not chunk:
                            break
                        received += len(chunk)
                        remaining -= len(chunk)
                else:
                    # Unknown length: drain up to cap, then stop and 413.
                    while True:
                        chunk = stream.read(min(CHUNK_BYTES, intent.cap + 1 - received))
                        if not chunk:
                            break
                        received += len(chunk)
                        if received > intent.cap:
                            truncated = True
                            break  # do not keep reading
        except Exception:
            intent.abort()
            raise
        recv_ms = (time.perf_counter() - t0) * 1000.0
        return intent.finish(received, truncated, recv_ms)


def _header_dict(environ):
    headers = {}
    for key, value in environ.items():
        if key.startswith("HTTP_"):
            name = key[5:].replace("_", "-").lower()
            if name not in headers:
                headers[name] = value
    # PEP 3333: these two live outside HTTP_*.
    if "CONTENT_LENGTH" in environ and environ["CONTENT_LENGTH"]:
        headers.setdefault("content-length", environ["CONTENT_LENGTH"])
    if "CONTENT_TYPE" in environ and environ["CONTENT_TYPE"]:
        headers.setdefault("content-type", environ["CONTENT_TYPE"])
    return headers


def _merge_mark_headers(headers, marks):
    existing = None
    index = None
    for i, (name, value) in enumerate(headers):
        if name.lower() == "server-timing":
            existing = value
            index = i
            break
    merged = merge_marks_into_server_timing(existing, marks)
    if merged is None:
        return headers
    if index is not None:
        headers[index] = ("Server-Timing", merged)
    else:
        headers.append(("Server-Timing", merged))
    return headers
