"""Minimal LagHound Python sample — a tiny 'real service' with the LagHound
diagnostic endpoint mounted, so a LagHound fleet can probe it.

Run:
    PORT=8083 LAGHOUND_TOKEN=demo-token-laghound python3 app.py

Then the fleet probes https://<host>:8083/laghound/* with that token.
Zero third-party dependencies (stdlib wsgiref + the laghound SDK).
"""

import os
import time
from wsgiref.simple_server import make_server

import laghound


def inner_app(environ, start_response):
    """The customer's own app — LagHound wraps it, never touches these routes."""
    path = environ.get("PATH_INFO", "/")
    if path == "/work":
        time.sleep(0.03)  # simulate ~30ms of real server processing
        body = b"python sample: work done\n"
    else:
        body = b"python sample ok\n"
    start_response("200 OK", [("Content-Type", "text/plain")])
    return [body]


# Wrap the app: /laghound/* is handled by the SDK, everything else falls through.
application = laghound.wsgi(
    inner_app,
    token=os.environ.get("LAGHOUND_TOKEN", "demo-token-laghound"),
    prefix="/laghound",
    app_name="python-sample",
)


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8083"))
    with make_server("0.0.0.0", port, application) as httpd:
        print(f"python sample on :{port} — LagHound at /laghound")
        httpd.serve_forever()
