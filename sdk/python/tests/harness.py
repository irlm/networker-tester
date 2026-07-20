"""Minimal in-repo test clients for the ASGI and WSGI adapters.

No third-party dependencies: the ASGI client drives the app coroutine with
``asyncio``; the WSGI client crafts a PEP 3333 environ by hand (wsgiref-style,
no werkzeug). Both expose the same ``request(...)`` API so the conformance
suite runs identically against both adapters.
"""

from __future__ import annotations

import asyncio
import io
import os
import sys

SDK_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO_ROOT = os.path.dirname(os.path.dirname(SDK_ROOT))
CONTRACT_JSON_PATH = os.path.join(REPO_ROOT, "shared", "sdk-contract-v1.json")

_src = os.path.join(SDK_ROOT, "src")
if _src not in sys.path:
    sys.path.insert(0, _src)


class HttpResult:
    def __init__(self, status, headers, body):
        self.status = status
        self.headers = headers  # list of (lower-name, value) str pairs
        self.body = body

    def header(self, name):
        name = name.lower()
        for key, value in self.headers:
            if key == name:
                return value
        return None

    def header_names(self):
        return [key for key, _ in self.headers]

    def json(self):
        import json

        return json.loads(self.body.decode("utf-8"))


# --------------------------------------------------------------------- WSGI


class SpyInput:
    """BytesIO wrapper that records whether/how much was read."""

    def __init__(self, data=b""):
        self._io = io.BytesIO(data)
        self.read_calls = 0
        self.bytes_read = 0

    def read(self, size=-1):
        self.read_calls += 1
        chunk = self._io.read(size)
        self.bytes_read += len(chunk)
        return chunk


class WsgiClient:
    kind = "wsgi"

    def __init__(self, app):
        self.app = app

    def request(
        self,
        method,
        path,
        headers=None,
        body=b"",
        query="",
        remote_addr="203.0.113.10",
        content_length="auto",
        input_stream=None,
    ):
        environ = {
            "REQUEST_METHOD": method,
            "SCRIPT_NAME": "",
            "PATH_INFO": path,
            "QUERY_STRING": query,
            "SERVER_NAME": "testserver",
            "SERVER_PORT": "80",
            "SERVER_PROTOCOL": "HTTP/1.1",
            "REMOTE_ADDR": remote_addr,
            "wsgi.version": (1, 0),
            "wsgi.url_scheme": "http",
            "wsgi.input": input_stream
            if input_stream is not None
            else SpyInput(body),
            "wsgi.errors": io.StringIO(),
            "wsgi.multithread": True,
            "wsgi.multiprocess": False,
            "wsgi.run_once": False,
        }
        if content_length == "auto":
            if body:
                environ["CONTENT_LENGTH"] = str(len(body))
        elif content_length is not None:
            environ["CONTENT_LENGTH"] = str(content_length)
        for name, value in (headers or {}).items():
            key = name.upper().replace("-", "_")
            if key in ("CONTENT_LENGTH", "CONTENT_TYPE"):
                environ[key] = value
            else:
                environ["HTTP_" + key] = value

        captured = {}

        def start_response(status, response_headers, exc_info=None):
            captured["status"] = int(status.split(" ", 1)[0])
            captured["headers"] = [(k.lower(), v) for k, v in response_headers]
            return lambda data: None

        iterable = self.app(environ, start_response)
        try:
            chunks = b"".join(bytes(c) for c in iterable)
        finally:
            close = getattr(iterable, "close", None)
            if close is not None:
                close()
        return HttpResult(captured["status"], captured["headers"], chunks), environ

    def start_streaming(self, method, path, headers=None, remote_addr="203.0.113.10"):
        """Begin a request but do not consume/close the response iterable.
        Returns (result_meta, iterable) so tests can hold a slot open."""
        environ = {
            "REQUEST_METHOD": method,
            "SCRIPT_NAME": "",
            "PATH_INFO": path,
            "QUERY_STRING": "",
            "SERVER_PROTOCOL": "HTTP/1.1",
            "REMOTE_ADDR": remote_addr,
            "wsgi.input": SpyInput(b""),
            "wsgi.errors": io.StringIO(),
            "wsgi.url_scheme": "http",
            "wsgi.version": (1, 0),
            "wsgi.multithread": True,
            "wsgi.multiprocess": False,
            "wsgi.run_once": False,
            "SERVER_NAME": "testserver",
            "SERVER_PORT": "80",
        }
        for name, value in (headers or {}).items():
            environ["HTTP_" + name.upper().replace("-", "_")] = value
        captured = {}

        def start_response(status, response_headers, exc_info=None):
            captured["status"] = int(status.split(" ", 1)[0])
            captured["headers"] = [(k.lower(), v) for k, v in response_headers]
            return lambda data: None

        iterable = self.app(environ, start_response)
        # Force headers out (generators may defer until first iteration —
        # our adapter calls start_response eagerly, but be safe).
        return captured, iterable


# --------------------------------------------------------------------- ASGI


class AsgiClient:
    kind = "asgi"

    def __init__(self, app):
        self.app = app

    def request(
        self,
        method,
        path,
        headers=None,
        body=b"",
        query="",
        remote_addr="203.0.113.10",
        content_length="auto",
        body_chunks=None,
    ):
        return asyncio.run(
            self._request(
                method, path, headers, body, query, remote_addr, content_length, body_chunks
            )
        ), None

    async def _request(
        self, method, path, headers, body, query, remote_addr, content_length, body_chunks
    ):
        raw_headers = []
        header_names = {name.lower() for name in (headers or {})}
        for name, value in (headers or {}).items():
            raw_headers.append(
                (name.lower().encode("latin-1"), value.encode("latin-1"))
            )
        if content_length == "auto":
            if body and "content-length" not in header_names:
                raw_headers.append((b"content-length", str(len(body)).encode()))
        elif content_length is not None:
            raw_headers.append((b"content-length", str(content_length).encode()))

        scope = {
            "type": "http",
            "asgi": {"version": "3.0"},
            "http_version": "1.1",
            "method": method,
            "scheme": "http",
            "path": path,
            "raw_path": path.encode("latin-1"),
            "query_string": query.encode("latin-1"),
            "root_path": "",
            "headers": raw_headers,
            "client": (remote_addr, 51234),
            "server": ("testserver", 80),
        }

        if body_chunks is None:
            body_chunks = [body] if body else [b""]
        messages = []
        for i, chunk in enumerate(body_chunks):
            messages.append(
                {
                    "type": "http.request",
                    "body": chunk,
                    "more_body": i < len(body_chunks) - 1,
                }
            )
        message_iter = iter(messages)
        self.last_receive_count = 0

        async def receive():
            self.last_receive_count += 1
            try:
                return next(message_iter)
            except StopIteration:
                return {"type": "http.disconnect"}

        sent = []

        async def send(message):
            sent.append(message)

        await self.app(scope, receive, send)

        status = None
        headers_out = []
        chunks = []
        for message in sent:
            if message["type"] == "http.response.start":
                status = message["status"]
                headers_out = [
                    (k.decode("latin-1").lower(), v.decode("latin-1"))
                    for k, v in message.get("headers", [])
                ]
            elif message["type"] == "http.response.body":
                chunks.append(message.get("body", b""))
        return HttpResult(status, headers_out, b"".join(chunks))


VALID_TOKEN = "conformance-token-0123456789abcdef"


def make_pair(inner=None, **config):
    """Build (asgi_client, wsgi_client) with identical config.

    Each gets its own core instance (independent limiter state)."""
    import laghound

    config.setdefault("token", VALID_TOKEN)
    asgi_inner, wsgi_inner = inner if inner else (None, None)
    return (
        AsgiClient(laghound.asgi(asgi_inner, **config)),
        WsgiClient(laghound.wsgi(wsgi_inner, **config)),
    )


def auth_headers(token=VALID_TOKEN):
    return {"X-LagHound-Token": token}
