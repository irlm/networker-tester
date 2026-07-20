"""LagHound endpoint SDK for Python — contract v1.

Embed a tiny diagnostic endpoint into your ASGI or WSGI app so the LagHound
tester fleet can split request time into network vs. server processing via
the ``Server-Timing`` header.

Spec: docs/sdk/contract-v1.md (repo root); machine-readable twin:
shared/sdk-contract-v1.json. Zero runtime dependencies — stdlib only.

Quickstart (FastAPI/Starlette)::

    from laghound import LagHoundMiddleware
    app.add_middleware(LagHoundMiddleware, token=os.environ["LAGHOUND_TOKEN"])

Quickstart (Flask/Django — WSGI)::

    import laghound
    app.wsgi_app = laghound.wsgi(app.wsgi_app, token=os.environ["LAGHOUND_TOKEN"])
"""

from ._asgi import LagHoundMiddleware
from ._core import (
    ABSOLUTE_MAX_BYTES,
    CONTRACT,
    LagHoundConfigError,
)
from ._marks import mark
from ._version import __version__
from ._wsgi import LagHoundWSGIMiddleware

__all__ = [
    "ABSOLUTE_MAX_BYTES",
    "CONTRACT",
    "LagHoundConfigError",
    "LagHoundMiddleware",
    "LagHoundWSGIMiddleware",
    "__version__",
    "asgi",
    "mark",
    "wsgi",
]


def asgi(app=None, **config):
    """Build the LagHound ASGI component.

    - ``laghound.asgi(inner_app, token=...)`` — middleware wrapping an app.
    - ``laghound.asgi(token=...)`` — standalone ASGI app, e.g. for
      ``app.mount("/laghound", laghound.asgi(token=...))``.

    Config keys (contract §2): ``token``/``tokens``, ``prefix`` (default
    ``/laghound``), ``download_cap_bytes``/``upload_cap_bytes`` (default 4 MiB,
    hard max 32 MiB), ``rate_per_ip``/``rate_global`` ((rps, burst) tuples),
    ``max_concurrent``, ``max_concurrent_transfers``, ``byte_budget``
    (``{"bytes": n, "window_s": s}``), ``app_name``, ``enable_echo``/
    ``enable_download``/``enable_upload``/``enable_info``, ``trusted_proxies``.
    """
    return LagHoundMiddleware(app, **config)


def wsgi(app=None, **config):
    """Build the LagHound WSGI component (same config keys as :func:`asgi`).

    - ``laghound.wsgi(inner_wsgi_app, token=...)`` — middleware.
    - ``laghound.wsgi(token=...)`` — standalone WSGI app.
    """
    return LagHoundWSGIMiddleware(app, **config)
