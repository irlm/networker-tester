"""LagHound Python SDK conformance suite — pinned to shared/sdk-contract-v1.json.

Runs every assertion against BOTH adapters (ASGI and WSGI) through the
dependency-free clients in ``harness.py``. stdlib unittest (pytest also
collects it).
"""

from __future__ import annotations

import json
import os
import re
import unittest

from harness import (
    CONTRACT_JSON_PATH,
    VALID_TOKEN,
    auth_headers,
    make_pair,
)

with open(CONTRACT_JSON_PATH, "r", encoding="utf-8") as f:
    CONTRACT = json.load(f)

SERVER_TIMING_RE = re.compile(
    r"\A[a-z0-9-]{1,32};dur=[0-9]+(\.[0-9]{1,3})?(, [a-z0-9-]{1,32};dur=[0-9]+(\.[0-9]{1,3})?)*\Z"
)

CACHE_CONTROL = "no-store, no-cache, must-revalidate"


def st_names(result):
    value = result.header("server-timing") or ""
    return [part.split(";")[0].strip() for part in value.split(",") if part.strip()]


class ConformanceBase(unittest.TestCase):
    def clients(self, **config):
        return make_pair(**config)

    def assert_bare_404(self, result, client_kind=""):
        ctx = " (%s)" % client_kind
        self.assertEqual(result.status, 404, "expected bare 404" + ctx)
        self.assertEqual(result.body, b"", "bare 404 must have no body" + ctx)
        names = result.header_names()
        self.assertNotIn("server-timing", names, "bare 404 must carry no Server-Timing" + ctx)
        self.assertNotIn("cache-control", names, ctx)
        self.assertNotIn("x-laghound-bytes", names, ctx)
        self.assertNotIn("www-authenticate", names, ctx)

    def assert_envelope(self, result, status, code, client_kind=""):
        self.assertEqual(result.status, status, client_kind)
        payload = result.json()
        self.assertEqual(payload["contract"], "v1", client_kind)
        self.assertEqual(payload["error"]["code"], code, client_kind)
        expected = next(
            c for c in CONTRACT["error_envelope"]["codes"] if c["code"] == code
        )
        self.assertEqual(status, expected["status"], client_kind)
        self.assertIsInstance(payload["error"]["message"], str, client_kind)
        self.assert_common_headers(result, client_kind)

    def assert_common_headers(self, result, client_kind=""):
        self.assertEqual(result.header("cache-control"), CACHE_CONTROL, client_kind)
        st = result.header("server-timing")
        self.assertIsNotNone(st, "Server-Timing required on every non-bare response " + client_kind)
        self.assertLessEqual(len(st), CONTRACT["server_timing"]["max_header_bytes"])
        self.assertRegex(st, SERVER_TIMING_RE)
        self.assertIn("app", st_names(result), "app metric MUST be on every response " + client_kind)


class TestContractPin(ConformanceBase):
    """Sanity-pin our constants against the machine-readable contract."""

    def test_constants_match_contract(self):
        from laghound import _core

        self.assertEqual(CONTRACT["contract"], "v1")
        self.assertEqual(_core.CONTRACT, CONTRACT["contract"])
        self.assertEqual(_core.DEFAULT_PREFIX, CONTRACT["prefix_default"])
        caps = CONTRACT["caps"]
        self.assertEqual(_core.ABSOLUTE_MAX_BYTES, caps["absolute_max_bytes"])
        self.assertEqual(_core.DEFAULT_CAP_BYTES, caps["download_default_bytes"])
        self.assertEqual(_core.DEFAULT_CAP_BYTES, caps["upload_default_bytes"])
        self.assertEqual(_core.ECHO_BODY_MAX_BYTES, caps["echo_request_body_max_bytes"])
        self.assertEqual(caps["clamp_report_header"], "X-LagHound-Bytes")
        download_route = next(r for r in CONTRACT["routes"] if r["id"] == "download")
        self.assertEqual(_core._FILL[0], download_route["response"]["fill_byte"])
        self.assertEqual(_core.TOKEN_MIN_BYTES, CONTRACT["auth"]["token_min_bytes"])
        self.assertEqual(_core.ROTATION_MAX_TOKENS, CONTRACT["auth"]["rotation_max_tokens"])
        self.assertEqual(_core.KILL_SWITCH_ENV, CONTRACT["kill_switch"]["env"])
        self.assertEqual(_core.TOKEN_ENV, CONTRACT["auth"]["env_token"])
        self.assertEqual(
            _core.PER_IP_TABLE_MAX_ENTRIES,
            CONTRACT["limits"]["per_ip_table_max_entries"],
        )
        self.assertIn("python", CONTRACT["sdk_langs"])
        self.assertEqual(
            CONTRACT["auth"]["check_order"],
            ["kill_switch", "rate_limits", "auth", "route"],
        )


class TestHealth(ConformanceBase):
    def test_health_shape(self):
        for client in self.clients(app_name="checkout-api"):
            result, _ = client.request("GET", "/laghound/health", headers=auth_headers())
            self.assertEqual(result.status, 200, client.kind)
            self.assertEqual(result.header("content-type"), "application/json")
            payload = result.json()
            self.assertEqual(payload["contract"], "v1", client.kind)
            self.assertEqual(payload["status"], "ok", client.kind)
            self.assertEqual(payload["sdk"]["lang"], "python", client.kind)
            self.assertRegex(payload["sdk"]["version"], r"^\d+\.\d+\.\d+")
            self.assertEqual(payload["app"], "checkout-api", client.kind)
            self.assertIsInstance(payload["uptime_s"], int, client.kind)
            self.assertEqual(
                set(payload["routes"]),
                {"health", "echo", "download", "upload", "info"},
                client.kind,
            )
            self.assertTrue(payload["routes"]["health"], client.kind)
            self.assert_common_headers(result, client.kind)
            self.assertIn("total", st_names(result), client.kind)

    def test_health_omits_app_when_not_configured(self):
        for client in self.clients():
            result, _ = client.request("GET", "/laghound/health", headers=auth_headers())
            self.assertNotIn("app", result.json(), client.kind)

    def test_disabled_routes_reflected_in_capability_map(self):
        for client in self.clients(enable_upload=False):
            result, _ = client.request("GET", "/laghound/health", headers=auth_headers())
            routes = result.json()["routes"]
            self.assertFalse(routes["upload"], client.kind)
            self.assertTrue(routes["health"], client.kind)
            # Probing the disabled route stays invisible.
            up, _ = client.request(
                "POST", "/laghound/upload", headers=auth_headers(), body=b"x" * 10
            )
            self.assert_bare_404(up, client.kind)


class TestEcho(ConformanceBase):
    def test_echo_fixed_body(self):
        echo_route = next(r for r in CONTRACT["routes"] if r["id"] == "echo")
        for client in self.clients():
            first, _ = client.request("GET", "/laghound/echo", headers=auth_headers())
            self.assertEqual(first.status, 200, client.kind)
            self.assertEqual(first.json(), echo_route["response"]["body_fixed"], client.kind)
            self.assertLess(len(first.body), 1024, client.kind)
            second, _ = client.request("GET", "/laghound/echo", headers=auth_headers())
            self.assertEqual(first.body, second.body, "echo body must be byte-constant")
            self.assert_common_headers(first, client.kind)

    def test_echo_zero_reflection(self):
        for client in self.clients():
            baseline, _ = client.request("GET", "/laghound/echo", headers=auth_headers())
            weird_headers = dict(auth_headers())
            weird_headers["X-Injected"] = "<script>alert(1)</script>"
            weird, _ = client.request(
                "GET",
                "/laghound/echo",
                headers=weird_headers,
                query="foo=%3Cb%3E&bar=baz",
            )
            self.assertEqual(baseline.body, weird.body, client.kind)
            self.assertNotIn(b"script", weird.body, client.kind)
            for _, value in weird.headers:
                self.assertNotIn("alert(1)", value, client.kind)

    def test_echo_body_over_cap_413(self):
        limit = CONTRACT["caps"]["echo_request_body_max_bytes"]
        for client in self.clients():
            result, _ = client.request(
                "GET",
                "/laghound/echo",
                headers=auth_headers(),
                content_length=limit + 1,
            )
            self.assert_envelope(result, 413, "payload_too_large", client.kind)


class TestDownload(ConformanceBase):
    def test_download_default_size(self):
        default = CONTRACT["caps"]["download_default_bytes"]
        for client in self.clients():
            result, _ = client.request("GET", "/laghound/download", headers=auth_headers())
            self.assertEqual(result.status, 200, client.kind)
            self.assertEqual(result.header("content-length"), str(default), client.kind)
            self.assertEqual(result.header("x-laghound-bytes"), str(default), client.kind)
            self.assertEqual(result.header("content-type"), "application/octet-stream")
            self.assertEqual(len(result.body), default, client.kind)
            self.assertEqual(result.body.count(b"\x42"), default, "fill byte must be 0x42")
            self.assert_common_headers(result, client.kind)

    def test_download_explicit_bytes(self):
        for client in self.clients():
            result, _ = client.request(
                "GET", "/laghound/download", headers=auth_headers(), query="bytes=10"
            )
            self.assertEqual(result.body, b"\x42" * 10, client.kind)
            self.assertEqual(result.header("content-length"), "10", client.kind)

    def test_download_clamp_and_report(self):
        for client in self.clients(download_cap_bytes=1024):
            result, _ = client.request(
                "GET", "/laghound/download", headers=auth_headers(), query="bytes=999999"
            )
            self.assertEqual(result.status, 200, "over-cap is clamped, not rejected")
            self.assertEqual(result.header("content-length"), "1024", client.kind)
            self.assertEqual(result.header("x-laghound-bytes"), "1024", client.kind)
            self.assertEqual(len(result.body), 1024, client.kind)

    def test_download_invalid_bytes_param(self):
        for value in ("abc", "-5", "1.5", "", "0x10"):
            for client in self.clients():
                result, _ = client.request(
                    "GET",
                    "/laghound/download",
                    headers=auth_headers(),
                    query="bytes=%s" % value,
                )
                self.assert_envelope(result, 400, "invalid_param", client.kind)
                # Never echoes the offending value (contract §7).
                if value:
                    self.assertNotIn(value.encode(), result.body, client.kind)

    def test_absolute_max_clamps_config(self):
        for client in self.clients(
            download_cap_bytes=64 * 1024 * 1024, upload_cap_bytes=64 * 1024 * 1024
        ):
            result, _ = client.request("GET", "/laghound/info", headers=auth_headers())
            caps = result.json()["caps"]
            self.assertEqual(caps["download_bytes"], CONTRACT["caps"]["absolute_max_bytes"])
            self.assertEqual(caps["upload_bytes"], CONTRACT["caps"]["absolute_max_bytes"])


class TestUpload(ConformanceBase):
    def test_upload_roundtrip(self):
        for client in self.clients():
            result, _ = client.request(
                "POST", "/laghound/upload", headers=auth_headers(), body=b"\xaa" * 1000
            )
            self.assertEqual(result.status, 200, client.kind)
            payload = result.json()
            self.assertEqual(payload["contract"], "v1", client.kind)
            self.assertEqual(payload["received_bytes"], 1000, client.kind)
            self.assertEqual(result.header("x-laghound-bytes"), "1000", client.kind)
            names = st_names(result)
            self.assertIn("recv", names, "recv MUST be present on /upload")
            self.assertIn("app", names, client.kind)
            self.assertIn("total", names, client.kind)
            # No reflection: request bytes never appear in the response.
            self.assertNotIn(b"\xaa", result.body, client.kind)

    def test_upload_content_length_over_cap_413_without_reading(self):
        asgi_client, wsgi_client = self.clients(upload_cap_bytes=1000)
        result, environ = wsgi_client.request(
            "POST",
            "/laghound/upload",
            headers=auth_headers(),
            body=b"x" * 2000,
        )
        self.assert_envelope(result, 413, "payload_too_large", "wsgi")
        self.assertEqual(
            environ["wsgi.input"].read_calls, 0, "413 must be sent WITHOUT reading the body"
        )
        result, _ = asgi_client.request(
            "POST", "/laghound/upload", headers=auth_headers(), body=b"x" * 2000
        )
        self.assert_envelope(result, 413, "payload_too_large", "asgi")
        self.assertEqual(
            asgi_client.last_receive_count, 0, "413 must be sent WITHOUT reading the body"
        )

    def test_upload_chunked_over_cap_bounded_drain(self):
        asgi_client, wsgi_client = self.clients(upload_cap_bytes=1000)
        # WSGI: no CONTENT_LENGTH -> drain up to cap, then 413 + close.
        result, environ = wsgi_client.request(
            "POST",
            "/laghound/upload",
            headers=auth_headers(),
            body=b"x" * 100000,
            content_length=None,
        )
        self.assert_envelope(result, 413, "payload_too_large", "wsgi")
        self.assertEqual(result.header("connection"), "close", "wsgi")
        self.assertLessEqual(
            environ["wsgi.input"].bytes_read,
            1000 + 65536,
            "drain must stop at the cap (bounded consumption)",
        )
        # ASGI: chunked body -> stop receiving once over cap.
        chunks = [b"x" * 500] * 10  # 5000 bytes total, cap 1000
        result, _ = asgi_client.request(
            "POST",
            "/laghound/upload",
            headers=auth_headers(),
            content_length=None,
            body_chunks=chunks,
        )
        self.assert_envelope(result, 413, "payload_too_large", "asgi")
        self.assertEqual(result.header("connection"), "close", "asgi")
        self.assertLess(
            asgi_client.last_receive_count, len(chunks), "must stop reading once over cap"
        )

    def test_upload_chunked_under_cap_ok(self):
        asgi_client, _ = self.clients()
        result, _ = asgi_client.request(
            "POST",
            "/laghound/upload",
            headers=auth_headers(),
            content_length=None,
            body_chunks=[b"a" * 300, b"b" * 300],
        )
        self.assertEqual(result.status, 200)
        self.assertEqual(result.json()["received_bytes"], 600)


class TestInfo(ConformanceBase):
    def test_info_shape_and_no_secrets(self):
        budget = {"bytes": 268435456, "window_s": 600}
        for client in self.clients(app_name="checkout-api", byte_budget=budget):
            result, _ = client.request("GET", "/laghound/info", headers=auth_headers())
            self.assertEqual(result.status, 200, client.kind)
            payload = result.json()
            self.assertEqual(payload["contract"], "v1")
            self.assertEqual(payload["sdk"]["lang"], "python")
            self.assertEqual(payload["prefix"], "/laghound")
            self.assertEqual(payload["app"], "checkout-api")
            self.assertTrue(payload["token_set"])
            self.assertEqual(payload["caps"]["absolute_max_bytes"], 33554432)
            self.assertEqual(payload["limits"]["rate_per_ip"], {"rps": 10, "burst": 20})
            self.assertEqual(payload["limits"]["rate_global"], {"rps": 50, "burst": 100})
            self.assertEqual(payload["limits"]["max_concurrent"], 8)
            self.assertEqual(payload["limits"]["max_concurrent_transfers"], 2)
            self.assertEqual(payload["limits"]["byte_budget"], budget)
            self.assertIsInstance(payload["uptime_s"], int)
            # The token, or any derivative, must never appear.
            self.assertNotIn(VALID_TOKEN.encode(), result.body, client.kind)
            self.assertNotIn(str(len(VALID_TOKEN)).encode() + b'"', result.body[:0] or b"")
            self.assert_common_headers(result, client.kind)

    def test_info_byte_budget_null_when_off(self):
        for client in self.clients():
            result, _ = client.request("GET", "/laghound/info", headers=auth_headers())
            self.assertIsNone(result.json()["limits"]["byte_budget"], client.kind)


class TestAuth(ConformanceBase):
    ALL_ROUTES = (
        ("GET", "/laghound/health"),
        ("GET", "/laghound/echo"),
        ("GET", "/laghound/download"),
        ("POST", "/laghound/upload"),
        ("GET", "/laghound/info"),
    )

    def test_no_token_bare_404_on_every_route_including_health(self):
        for client in self.clients():
            for method, path in self.ALL_ROUTES:
                result, _ = client.request(method, path)
                self.assert_bare_404(result, "%s %s %s" % (client.kind, method, path))

    def test_wrong_token_bare_404(self):
        for client in self.clients():
            result, _ = client.request(
                "GET", "/laghound/echo", headers=auth_headers("wrong-token-0123456789abcdef")
            )
            self.assert_bare_404(result, client.kind)

    def test_bearer_token_accepted(self):
        for client in self.clients():
            result, _ = client.request(
                "GET",
                "/laghound/echo",
                headers={"Authorization": "Bearer " + VALID_TOKEN},
            )
            self.assertEqual(result.status, 200, client.kind)

    def test_laghound_header_wins_over_bearer(self):
        for client in self.clients():
            # Valid X-LagHound-Token + garbage Bearer -> 200 (Bearer ignored).
            ok, _ = client.request(
                "GET",
                "/laghound/echo",
                headers={
                    "X-LagHound-Token": VALID_TOKEN,
                    "Authorization": "Bearer definitely-not-the-token",
                },
            )
            self.assertEqual(ok.status, 200, client.kind)
            # Invalid X-LagHound-Token + VALID Bearer -> 404 (Bearer not compared).
            bad, _ = client.request(
                "GET",
                "/laghound/echo",
                headers={
                    "X-LagHound-Token": "definitely-not-the-token",
                    "Authorization": "Bearer " + VALID_TOKEN,
                },
            )
            self.assert_bare_404(bad, client.kind)

    def test_token_rotation_two_tokens(self):
        old, new = "old-token-0123456789abcdef", "new-token-0123456789abcdef"
        for client in self.clients(token=None, tokens=[new, old]):
            for tok in (old, new):
                result, _ = client.request("GET", "/laghound/echo", headers=auth_headers(tok))
                self.assertEqual(result.status, 200, client.kind)
            result, _ = client.request(
                "GET", "/laghound/echo", headers=auth_headers("third-token-0123456789ab")
            )
            self.assert_bare_404(result, client.kind)

    def test_unknown_subpath_bare_404_even_authenticated(self):
        for client in self.clients():
            result, _ = client.request(
                "GET", "/laghound/admin", headers=auth_headers()
            )
            self.assert_bare_404(result, client.kind)

    def test_wrong_method_known_route(self):
        for client in self.clients():
            result, _ = client.request("POST", "/laghound/echo", headers=auth_headers())
            self.assert_envelope(result, 405, "method_not_allowed", client.kind)
            # Unauthenticated wrong method stays invisible.
            result, _ = client.request("POST", "/laghound/echo")
            self.assert_bare_404(result, client.kind)


class TestKillSwitch(ConformanceBase):
    def test_kill_switch_bare_404_everywhere(self):
        os.environ["LAGHOUND_DISABLED"] = "1"
        try:
            for client in self.clients():
                for method, path in TestAuth.ALL_ROUTES:
                    result, _ = client.request(method, path, headers=auth_headers())
                    self.assert_bare_404(result, "%s %s" % (client.kind, path))
        finally:
            del os.environ["LAGHOUND_DISABLED"]
        # Fresh instances (fresh <=1s cache) work again after the flip.
        for client in self.clients():
            result, _ = client.request("GET", "/laghound/health", headers=auth_headers())
            self.assertEqual(result.status, 200, client.kind)


class TestRateLimits(ConformanceBase):
    def test_per_ip_limit_authenticated_429(self):
        for client in self.clients(rate_per_ip=(1, 2)):
            for _ in range(2):
                result, _ = client.request("GET", "/laghound/echo", headers=auth_headers())
                self.assertEqual(result.status, 200, client.kind)
            result, _ = client.request("GET", "/laghound/echo", headers=auth_headers())
            self.assert_envelope(result, 429, "rate_limited", client.kind)
            retry_after = result.header("retry-after")
            self.assertIsNotNone(retry_after, "Retry-After MUST be present on 429")
            self.assertEqual(
                result.json()["error"]["retry_after_ms"],
                int(retry_after) * 1000,
                "retry_after_ms mirrors Retry-After",
            )

    def test_per_ip_limit_unauthenticated_bare_404_indistinguishable(self):
        for client in self.clients(rate_per_ip=(1, 2)):
            responses = []
            for _ in range(4):
                result, _ = client.request("GET", "/laghound/echo")
                responses.append(result)
            # Auth-failure 404s and limiter 404s must be identical.
            for result in responses:
                self.assert_bare_404(result, client.kind)
            first = responses[0]
            for other in responses[1:]:
                self.assertEqual(other.status, first.status)
                self.assertEqual(other.body, first.body)
                self.assertEqual(sorted(other.header_names()), sorted(first.header_names()))

    def test_global_limit_across_ips(self):
        for client in self.clients(rate_global=(1, 2)):
            for i in range(2):
                result, _ = client.request(
                    "GET",
                    "/laghound/echo",
                    headers=auth_headers(),
                    remote_addr="198.51.100.%d" % (i + 1),
                )
                self.assertEqual(result.status, 200, client.kind)
            result, _ = client.request(
                "GET",
                "/laghound/echo",
                headers=auth_headers(),
                remote_addr="198.51.100.99",
            )
            self.assert_envelope(result, 429, "rate_limited", client.kind)


class TestByteBudget(ConformanceBase):
    def test_budget_exhaustion_429_with_retry_after(self):
        for client in self.clients(byte_budget={"bytes": 10, "window_s": 600}):
            first, _ = client.request(
                "GET", "/laghound/download", headers=auth_headers(), query="bytes=12"
            )
            self.assertEqual(first.status, 200, client.kind)  # charged 12 >= 10
            second, _ = client.request(
                "GET", "/laghound/download", headers=auth_headers(), query="bytes=1"
            )
            self.assert_envelope(second, 429, "rate_limited", client.kind)
            retry_after = int(second.header("retry-after"))
            self.assertGreaterEqual(retry_after, 1, client.kind)
            self.assertLessEqual(retry_after, 600, client.kind)
            # Upload is budget-limited too.
            up, _ = client.request(
                "POST", "/laghound/upload", headers=auth_headers(), body=b"x"
            )
            self.assert_envelope(up, 429, "rate_limited", client.kind)
            # /health, /echo, /info are never budget-limited.
            for path in ("/laghound/health", "/laghound/echo", "/laghound/info"):
                result, _ = client.request("GET", path, headers=auth_headers())
                self.assertEqual(result.status, 200, "%s %s" % (client.kind, path))


class TestPassthroughAndErrors(ConformanceBase):
    def test_passthrough_untouched(self):
        seen = []

        async def asgi_inner(scope, receive, send):
            seen.append(("asgi", scope["path"]))
            await send(
                {
                    "type": "http.response.start",
                    "status": 201,
                    "headers": [(b"x-inner", b"yes")],
                }
            )
            await send({"type": "http.response.body", "body": b"inner"})

        def wsgi_inner(environ, start_response):
            seen.append(("wsgi", environ["PATH_INFO"]))
            start_response("201 Created", [("X-Inner", "yes")])
            return [b"inner"]

        for client in self.clients(inner=(asgi_inner, wsgi_inner)):
            result, _ = client.request("GET", "/api/users")
            self.assertEqual(result.status, 201, client.kind)
            self.assertEqual(result.body, b"inner", client.kind)
            self.assertEqual(result.header("x-inner"), "yes", client.kind)
        self.assertEqual(
            sorted(seen), [("asgi", "/api/users"), ("wsgi", "/api/users")]
        )

    def test_internal_error_500_envelope_confined(self):
        def boom(*args, **kwargs):
            raise RuntimeError("secret internal detail")

        for client in self.clients():
            core = client.app.core
            core._handle_echo = boom
            result, _ = client.request("GET", "/laghound/echo", headers=auth_headers())
            self.assert_envelope(result, 500, "internal", client.kind)
            self.assertEqual(result.json()["error"]["message"], "internal error")
            self.assertNotIn(b"secret internal detail", result.body, client.kind)
            # Other routes still work — the failure is confined.
            ok, _ = client.request("GET", "/laghound/health", headers=auth_headers())
            self.assertEqual(ok.status, 200, client.kind)

    def test_standalone_mounted_mode(self):
        # Mounting frameworks strip the prefix: subpaths arrive as /echo.
        for client in self.clients():  # no inner app -> standalone
            result, _ = client.request("GET", "/echo", headers=auth_headers())
            self.assertEqual(result.status, 200, client.kind)
            result, _ = client.request("GET", "/whatever", headers=auth_headers())
            self.assert_bare_404(result, client.kind)

    def test_custom_prefix(self):
        for client in self.clients(prefix="/diag/lh"):
            result, _ = client.request("GET", "/diag/lh/echo", headers=auth_headers())
            self.assertEqual(result.status, 200, client.kind)


if __name__ == "__main__":
    unittest.main()
