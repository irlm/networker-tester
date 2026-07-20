// Conformance suite for @laghound/endpoint, pinned to
// shared/sdk-contract-v1.json (loaded via test/helpers.ts). Asserts every
// route shape, cap, header and status against a live node:http server.
//
// Run: `npm test` (node --test, type-stripping — zero devDeps beyond tsc).
import { test, before, after, describe } from "node:test";
import assert from "node:assert/strict";
import { auth, contract, rawRequest, startServer, TOKEN, type TestServer } from "./helpers.ts";

const ABS_MAX = 33554432;
const DEFAULT_CAP = 4194304;

let srv: TestServer;

before(async () => {
  srv = await startServer();
});
after(async () => {
  await srv.close();
});

async function get(path: string, headers: Record<string, string> = auth()) {
  const res = await fetch(srv.url + path, { headers });
  const buf = Buffer.from(await res.arrayBuffer());
  return { res, buf, text: buf.toString("utf8") };
}

// --- contract sanity: the file we pin to is the v1 twin -------------------
describe("contract file", () => {
  test("is v1 and lists this SDK lang", () => {
    assert.equal(contract.contract, "v1");
    assert.ok((contract.sdk_langs as string[]).includes("js"));
    assert.equal(contract.prefix_default, "/laghound");
  });
});

// --- 3.1 /health ----------------------------------------------------------
describe("GET /health", () => {
  test("200 with contract-shaped body", async () => {
    const { res, text } = await get("/laghound/health");
    assert.equal(res.status, 200);
    assert.match(res.headers.get("content-type") ?? "", /application\/json/);
    const j = JSON.parse(text);
    assert.equal(j.contract, "v1");
    assert.equal(j.status, "ok");
    assert.equal(j.sdk.lang, "js");
    assert.match(j.sdk.version, /^\d+\.\d+\.\d+/);
    assert.equal(typeof j.uptime_s, "number");
    assert.deepEqual(Object.keys(j.routes).sort(), ["download", "echo", "health", "info", "upload"]);
    assert.equal(j.routes.health, true);
  });

  test("carries Server-Timing app + Cache-Control no-store", async () => {
    const { res } = await get("/laghound/health");
    const st = res.headers.get("server-timing") ?? "";
    assert.match(st, /(^|,\s*)app;dur=/);
    assert.match(st, /(^|,\s*)total;dur=/);
    assert.match(res.headers.get("cache-control") ?? "", /no-store/);
  });

  test("app omitted unless app_name configured; present when set", async () => {
    const { text } = await get("/laghound/health");
    assert.equal("app" in JSON.parse(text), false);
    const s = await startServer({ appName: "checkout-api" });
    try {
      const r = await fetch(s.url + "/laghound/health", { headers: auth() });
      assert.equal((await r.json()).app, "checkout-api");
    } finally {
      await s.close();
    }
  });
});

// --- 3.2 /echo ------------------------------------------------------------
describe("GET /echo", () => {
  test("returns the fixed < 1 KiB body, no reflection", async () => {
    const { res, buf, text } = await get("/laghound/echo?injected=<script>");
    assert.equal(res.status, 200);
    assert.deepEqual(JSON.parse(text), { contract: "v1", ok: true });
    assert.ok(buf.length < 1024);
    assert.equal(text.includes("script"), false);
  });

  test("byte-constant across calls", async () => {
    const a = (await get("/laghound/echo")).buf;
    const b = (await get("/laghound/echo?x=1")).buf;
    assert.deepEqual(a, b);
  });

  test("413 for a declared body over 64 KiB", async () => {
    const { response } = rawRequest(srv.url, {
      method: "GET",
      path: "/laghound/echo",
      headers: { ...auth(), "content-length": String(65536 + 1) },
      write: Buffer.alloc(10),
      end: true,
    });
    const r = await response;
    assert.equal(r.status, 413);
    assert.equal(JSON.parse(r.body.toString()).error.code, "payload_too_large");
  });
});

// --- 3.3 /download --------------------------------------------------------
describe("GET /download", () => {
  test("default size when bytes omitted", async () => {
    const { res, buf } = await get("/laghound/download");
    assert.equal(res.status, 200);
    assert.equal(res.headers.get("content-type"), "application/octet-stream");
    assert.equal(res.headers.get("content-length"), String(DEFAULT_CAP));
    assert.equal(res.headers.get("x-laghound-bytes"), String(DEFAULT_CAP));
    assert.equal(buf.length, DEFAULT_CAP);
    assert.ok(buf.every((b) => b === 0x42)); // fill 'B'
  });

  test("exact requested size under the cap", async () => {
    const { res, buf } = await get("/laghound/download?bytes=1000");
    assert.equal(res.headers.get("content-length"), "1000");
    assert.equal(res.headers.get("x-laghound-bytes"), "1000");
    assert.equal(buf.length, 1000);
  });

  test("over-cap is clamped and reported, not rejected", async () => {
    const s = await startServer({ downloadCapBytes: 8192 });
    try {
      const r = await fetch(s.url + "/laghound/download?bytes=1000000", { headers: auth() });
      const b = Buffer.from(await r.arrayBuffer());
      assert.equal(r.status, 200);
      assert.equal(r.headers.get("x-laghound-bytes"), "8192");
      assert.equal(b.length, 8192);
    } finally {
      await s.close();
    }
  });

  test("clamps to the 32 MiB absolute max even above config", async () => {
    const s = await startServer({ downloadCapBytes: ABS_MAX });
    try {
      const r = await fetch(s.url + `/laghound/download?bytes=${ABS_MAX + 1000000}`, { headers: auth() });
      await r.arrayBuffer();
      assert.equal(r.headers.get("x-laghound-bytes"), String(ABS_MAX));
    } finally {
      await s.close();
    }
  });

  test("invalid bytes -> 400 invalid_param, no value echo", async () => {
    const { res, text } = await get("/laghound/download?bytes=abc");
    assert.equal(res.status, 400);
    const j = JSON.parse(text);
    assert.equal(j.error.code, "invalid_param");
    assert.equal(text.includes("abc"), false);
  });

  test("negative bytes -> 400", async () => {
    const { res } = await get("/laghound/download?bytes=-5");
    assert.equal(res.status, 400);
  });
});

// --- 3.4 /upload ----------------------------------------------------------
describe("POST /upload", () => {
  test("counts drained bytes, reports via header + JSON", async () => {
    const payload = Buffer.alloc(50000, 0x41);
    const r = await fetch(srv.url + "/laghound/upload", { method: "POST", headers: auth(), body: payload });
    const j = await r.json();
    assert.equal(r.status, 200);
    assert.equal(j.contract, "v1");
    assert.equal(j.received_bytes, 50000);
    assert.equal(r.headers.get("x-laghound-bytes"), "50000");
    const st = r.headers.get("server-timing") ?? "";
    assert.match(st, /recv;dur=/);
    assert.match(st, /app;dur=/);
  });

  test("declared Content-Length over cap -> 413 without reading body", async () => {
    const s = await startServer({ uploadCapBytes: 8192 });
    try {
      // Send the headers (Content-Length: 1000000) plus a single byte to
      // flush them, but never the rest of the declared body. The server must
      // respond 413 from the Content-Length header alone, without draining.
      const { response } = rawRequest(s.url, {
        method: "POST",
        path: "/laghound/upload",
        headers: { ...auth(), "content-length": String(1000000) },
        write: Buffer.from([0x41]),
        end: false,
      });
      const r = await response;
      assert.equal(r.status, 413);
      assert.equal(JSON.parse(r.body.toString()).error.code, "payload_too_large");
    } finally {
      await s.close();
    }
  });

  test("chunked/unknown length over cap -> 413", async () => {
    const s = await startServer({ uploadCapBytes: 4096 });
    try {
      // No content-length -> chunked; body larger than cap.
      const r = await fetch(s.url + "/laghound/upload", {
        method: "POST",
        headers: auth(),
        body: Buffer.alloc(20000, 0x41),
      }).catch((e) => e);
      // Either the fetch sees a 413 or the connection is closed mid-stream.
      if (r instanceof Error) {
        assert.ok(true);
      } else {
        assert.equal(r.status, 413);
      }
    } finally {
      await s.close();
    }
  });
});

// --- 3.5 /info ------------------------------------------------------------
describe("GET /info", () => {
  test("echoes config, never the token or a derivative", async () => {
    const { res, text } = await get("/laghound/info");
    assert.equal(res.status, 200);
    const j = JSON.parse(text);
    assert.equal(j.contract, "v1");
    assert.equal(j.sdk.lang, "js");
    assert.equal(j.prefix, "/laghound");
    assert.equal(j.token_set, true);
    assert.equal(j.caps.absolute_max_bytes, ABS_MAX);
    assert.equal(j.limits.rate_per_ip.rps, 10);
    // The token, or any derivative, must not appear anywhere.
    assert.equal(text.includes(TOKEN), false);
    assert.equal("token" in j, false);
  });
});

// --- 5 auth ---------------------------------------------------------------
describe("auth", () => {
  test("missing token -> bare 404 (no headers, no body)", async () => {
    const r = await fetch(srv.url + "/laghound/health");
    const b = Buffer.from(await r.arrayBuffer());
    assert.equal(r.status, 404);
    assert.equal(b.length, 0);
    assert.equal(r.headers.get("server-timing"), null);
    assert.equal(r.headers.get("cache-control"), null);
    assert.equal(r.headers.get("www-authenticate"), null);
    assert.equal(r.headers.get("x-laghound-bytes"), null);
  });

  test("bad token -> bare 404 indistinguishable from missing", async () => {
    const r = await fetch(srv.url + "/laghound/health", {
      headers: { "x-laghound-token": "wrong-but-long-enough-token-here" },
    });
    assert.equal(r.status, 404);
    assert.equal((await r.arrayBuffer()).byteLength, 0);
  });

  test("Authorization: Bearer is accepted", async () => {
    const r = await fetch(srv.url + "/laghound/health", { headers: { authorization: `Bearer ${TOKEN}` } });
    assert.equal(r.status, 200);
  });

  test("X-LagHound-Token wins when both present (bad Bearer ignored)", async () => {
    const r = await fetch(srv.url + "/laghound/health", {
      headers: { "x-laghound-token": TOKEN, authorization: "Bearer nonsense-value-goes-here-ok" },
    });
    assert.equal(r.status, 200);
  });

  test("even /health is auth-gated (differs from networker-endpoint)", async () => {
    const r = await fetch(srv.url + "/laghound/health");
    assert.equal(r.status, 404);
  });

  test("unknown subpath under prefix -> bare 404", async () => {
    const r = await fetch(srv.url + "/laghound/nope", { headers: auth() });
    assert.equal(r.status, 404);
    assert.equal((await r.arrayBuffer()).byteLength, 0);
  });
});

// --- 6 method / envelopes -------------------------------------------------
describe("method + envelopes", () => {
  test("wrong method on a known route -> 405 method_not_allowed", async () => {
    const r = await fetch(srv.url + "/laghound/echo", { method: "POST", headers: auth() });
    assert.equal(r.status, 405);
    assert.equal((await r.json()).error.code, "method_not_allowed");
  });

  test("error envelope shape matches the contract", async () => {
    const r = await fetch(srv.url + "/laghound/download?bytes=xyz", { headers: auth() });
    const j = await r.json();
    assert.equal(j.contract, "v1");
    assert.equal(typeof j.error.code, "string");
    assert.equal(typeof j.error.message, "string");
  });
});

// --- routes toggle --------------------------------------------------------
describe("route toggles", () => {
  test("disabled route reported false in /health and 404s", async () => {
    const s = await startServer({ routes: { upload: false } });
    try {
      const h = await (await fetch(s.url + "/laghound/health", { headers: auth() })).json();
      assert.equal(h.routes.upload, false);
      const u = await fetch(s.url + "/laghound/upload", { method: "POST", headers: auth(), body: "x" });
      assert.equal(u.status, 404);
    } finally {
      await s.close();
    }
  });
});
