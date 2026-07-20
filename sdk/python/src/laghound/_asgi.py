"""Pure ASGI adapter for the LagHound endpoint (contract v1).

Works two ways:

- Middleware: ``app.add_middleware(LagHoundMiddleware, token=...)`` or
  ``asgi_app = LagHoundMiddleware(asgi_app, token=...)``. Requests under
  ``prefix`` are handled; everything else passes through untouched (plus
  optional ``laghound.mark`` Server-Timing marks on host responses).
- Standalone app: ``app.mount("/laghound", laghound.asgi(token=...))`` or
  served directly. With no inner app, any path that is not a LagHound route
  gets the bare 404 (mounting frameworks strip the prefix, so subpaths arrive
  as e.g. ``/echo``).

Concurrency model: the shared core's limiter state is guarded by a
``threading.Lock`` whose critical sections contain no awaits, so holding it
briefly from coroutine code cannot deadlock the event loop. Download chunks
come from an in-memory buffer (no blocking I/O), so streaming them inline is
event-loop safe. Caps are per-process.
"""

from __future__ import annotations

import time

from . import _marks
from ._core import (
    LagHoundCore,
    Response,
    UploadIntent,
    merge_marks_into_server_timing,
)


class LagHoundMiddleware:
    def __init__(self, app=None, **config):
        self.app = app
        self.core = LagHoundCore(**config)

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            if self.app is not None:
                await self.app(scope, receive, send)
            elif scope["type"] == "lifespan":
                await _standalone_lifespan(receive, send)
            return

        path = scope.get("path", "")
        subpath = self.core.resolve(path)
        if subpath is None:
            if self.app is not None:
                await self._passthrough(scope, receive, send)
                return
            # Standalone: the mounting framework already stripped the prefix.
            subpath = path

        await self._handle(scope, receive, send, subpath)

    # ------------------------------------------------------------ passthrough

    async def _passthrough(self, scope, receive, send):
        token = _marks.open_request()
        try:

            async def send_with_marks(message):
                if message["type"] == "http.response.start":
                    marks = _marks.current_marks()
                    if marks:
                        message = dict(message)
                        message["headers"] = _merge_mark_headers(
                            list(message.get("headers") or []), marks
                        )
                await send(message)

            await self.app(scope, receive, send_with_marks)
        finally:
            _marks.close_request(token)

    # ---------------------------------------------------------------- laghound

    async def _handle(self, scope, receive, send, subpath):
        core = self.core
        try:
            headers = _header_dict(scope.get("headers") or [])
            client = scope.get("client")
            peer_ip = client[0] if client else ""
            query = (scope.get("query_string") or b"").decode("latin-1")
            result = core.handle(
                scope.get("method", "GET"), subpath, query, headers, peer_ip
            )
            if isinstance(result, UploadIntent):
                result = await self._drain_upload(receive, result)
        except Exception:
            result = core.internal_error_response()

        await self._send_response(send, result)

    async def _drain_upload(self, receive, intent):
        received = 0
        truncated = False
        t0 = time.perf_counter()
        try:
            while True:
                message = await receive()
                mtype = message["type"]
                if mtype == "http.request":
                    received += len(message.get("body", b""))
                    if received > intent.cap:
                        truncated = True
                        break  # stop reading (contract §3.4)
                    if not message.get("more_body", False):
                        break
                elif mtype == "http.disconnect":
                    break
                else:  # unknown message type — stop, fail closed
                    break
        except Exception:
            intent.abort()
            raise
        recv_ms = (time.perf_counter() - t0) * 1000.0
        return intent.finish(received, truncated, recv_ms)

    async def _send_response(self, send, response: Response):
        try:
            raw_headers = [
                (name.lower().encode("latin-1"), value.encode("latin-1"))
                for name, value in response.headers
            ]
            if response.close_connection:
                raw_headers.append((b"connection", b"close"))
            await send(
                {
                    "type": "http.response.start",
                    "status": response.status,
                    "headers": raw_headers,
                }
            )
            for chunk in response.chunks:
                if chunk:
                    await send(
                        {
                            "type": "http.response.body",
                            "body": chunk if isinstance(chunk, bytes) else bytes(chunk),
                            "more_body": True,
                        }
                    )
            await send({"type": "http.response.body", "body": b"", "more_body": False})
        finally:
            response.on_close()


def _header_dict(raw_headers):
    headers = {}
    for name, value in raw_headers:
        key = name.decode("latin-1").lower()
        if key not in headers:  # first occurrence wins
            headers[key] = value.decode("latin-1")
    return headers


def _merge_mark_headers(raw_headers, marks):
    existing = None
    index = None
    for i, (name, value) in enumerate(raw_headers):
        if name.lower() == b"server-timing":
            existing = value.decode("latin-1")
            index = i
            break
    merged = merge_marks_into_server_timing(existing, marks)
    if merged is None:
        return raw_headers
    encoded = (b"server-timing", merged.encode("latin-1"))
    if index is not None:
        raw_headers[index] = encoded
    else:
        raw_headers.append(encoded)
    return raw_headers


async def _standalone_lifespan(receive, send):
    while True:
        message = await receive()
        if message["type"] == "lifespan.startup":
            await send({"type": "lifespan.startup.complete"})
        elif message["type"] == "lifespan.shutdown":
            await send({"type": "lifespan.shutdown.complete"})
            return
