"""Request-scoped Server-Timing marks (contract §4.2, ``mark-<name>``).

Host-app handlers call ``laghound.mark("db", elapsed_ms)`` while serving a
request that passes through the LagHound middleware; the middleware appends
``mark-db;dur=<ms>`` to that response's Server-Timing header.

A ``contextvars.ContextVar`` gives correct isolation under both models:
- WSGI: each worker thread runs in its own context, so marks never leak
  between threads.
- ASGI: the middleware and the wrapped app run in the same task context, and
  concurrent tasks each get their own copy.

Marks recorded after response headers have been sent are dropped (headers are
gone). Invalid names (must match ``[a-z0-9]{1,24}``) or negative/non-finite
durations are ignored silently — never raised into the host app.
"""

from __future__ import annotations

from contextvars import ContextVar

from ._core import validate_mark

_marks: ContextVar = ContextVar("laghound_marks", default=None)


def mark(name, dur_ms):
    """Record a custom Server-Timing mark for the current request.

    ``name`` must match ``[a-z0-9]{1,24}`` (it is emitted as ``mark-<name>``);
    ``dur_ms`` is a non-negative duration in milliseconds. No-op outside a
    request handled through the LagHound middleware, and for invalid input.
    """
    bucket = _marks.get()
    if bucket is None:
        return
    validated = validate_mark(name, dur_ms)
    if validated is None:
        return
    bucket.append(validated)


def open_request():
    """Middleware: begin collecting marks for the current context."""
    return _marks.set([])


def close_request(token):
    """Middleware: stop collecting marks."""
    _marks.reset(token)


def current_marks():
    return _marks.get()
