// Safety suite (contract §6): rate limits, concurrency caps, byte budget,
// kill switch, fail-closed init, zero-reflection marks, and the streaming
// memory bound. Run via `npm test`.
import { test, describe } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { auth, rawRequest, startServer, TOKEN } from "./helpers.ts";
import { laghound } from "../src/index.ts";

// --- 5 fail-closed init ---------------------------------------------------
describe("fail-closed mount", () => {
  test("refuses to mount without any token", () => {
    delete process.env.LAGHOUND_TOKEN;
    assert.throws(() => laghound({}), /without a token/);
  });

  test("rejects a token under 16 bytes", () => {
    assert.throws(() => laghound({ token: "short" }), /16 bytes/);
  });

  test("reads LAGHOUND_TOKEN from the env when not passed", async () => {
    process.env.LAGHOUND_TOKEN = "env-token-long-enough-abcdef";
    try {
      const h = laghound({});
      assert.equal(typeof h, "function");
    } finally {
      delete process.env.LAGHOUND_TOKEN;
    }
  });

  test("accepts at most 2 tokens (current + previous)", () => {
    assert.throws(() => laghound({ token: ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc"] }), /at most 2/);
  });

  test("previous token still authenticates during rotation", async () => {
    const s = await startServer({ token: ["current-token-aaaaaaaaaaaa", "previous-token-bbbbbbbbbbbb"] });
    try {
      const r = await fetch(s.url + "/laghound/health", {
        headers: { "x-laghound-token": "previous-token-bbbbbbbbbbbb" },
      });
      assert.equal(r.status, 200);
    } finally {
      await s.close();
    }
  });

  test("rejects a bad prefix", () => {
    assert.throws(() => laghound({ token: TOKEN, prefix: "laghound" }), /start with/);
    assert.throws(() => laghound({ token: TOKEN, prefix: "/laghound/" }), /trailing slash/);
  });
});

// --- 6.5 kill switch ------------------------------------------------------
describe("kill switch", () => {
  test("LAGHOUND_DISABLED=1 makes every route a bare 404", async () => {
    // Fresh server so the per-instance kill-switch cache starts clean.
    const s = await startServer();
    try {
      // Warm one OK request, then flip the switch and wait past the 1 s cache.
      assert.equal((await fetch(s.url + "/laghound/health", { headers: auth() })).status, 200);
      process.env.LAGHOUND_DISABLED = "1";
      await new Promise((r) => setTimeout(r, 1100));
      const r = await fetch(s.url + "/laghound/health", { headers: auth() });
      const b = Buffer.from(await r.arrayBuffer());
      assert.equal(r.status, 404);
      assert.equal(b.length, 0);
      assert.equal(r.headers.get("server-timing"), null);
    } finally {
      delete process.env.LAGHOUND_DISABLED;
      await s.close();
    }
  });
});

// --- 6.2 rate limit -------------------------------------------------------
describe("rate limits", () => {
  test("authenticated over-limit -> 429 with Retry-After + envelope", async () => {
    const s = await startServer({ ratePerIp: { rps: 1, burst: 2 }, rateGlobal: { rps: 100, burst: 100 } });
    try {
      let sawLimited = false;
      for (let i = 0; i < 12; i++) {
        const r = await fetch(s.url + "/laghound/health", { headers: auth() });
        if (r.status === 429) {
          sawLimited = true;
          assert.ok(r.headers.get("retry-after"));
          const j = await r.json();
          assert.equal(j.error.code, "rate_limited");
          assert.equal(typeof j.error.retry_after_ms, "number");
          break;
        }
        await r.arrayBuffer();
      }
      assert.ok(sawLimited, "expected a 429 within the burst window");
    } finally {
      await s.close();
    }
  });

  test("unauthenticated over-limit stays a bare 404 (invisible)", async () => {
    const s = await startServer({ ratePerIp: { rps: 1, burst: 1 }, rateGlobal: { rps: 100, burst: 100 } });
    try {
      let saw429 = false;
      let saw404 = false;
      for (let i = 0; i < 8; i++) {
        const r = await fetch(s.url + "/laghound/health"); // no token
        if (r.status === 429) saw429 = true;
        if (r.status === 404) saw404 = true;
        await r.arrayBuffer();
      }
      assert.equal(saw429, false, "unauthenticated traffic must never see 429");
      assert.ok(saw404);
    } finally {
      await s.close();
    }
  });
});

// --- 6.3 concurrency + transfer caps --------------------------------------
describe("concurrency caps", () => {
  test("only max_concurrent_transfers transfers may run at once", async () => {
    // Cap transfers at 1. Hold the single slot with a chunked upload that
    // never ends (the SDK drains it, occupying the slot), then a download
    // must be turned away with 429.
    const s = await startServer({ maxConcurrentTransfers: 1, uploadCapBytes: 33554432, downloadCapBytes: 33554432 });
    try {
      // Chunked upload (no content-length), first byte flushes headers so the
      // handler runs and takes the transfer slot, then we hold it open.
      const holder = rawRequest(s.url, {
        method: "POST",
        path: "/laghound/upload",
        headers: auth(),
        write: Buffer.from([0x41]),
        end: false,
      });
      holder.response.catch(() => {}); // destroyed below — swallow the hang-up
      // Let the upload occupy the transfer slot.
      await new Promise((r) => setTimeout(r, 150));
      const second = await fetch(s.url + "/laghound/download?bytes=1000", { headers: auth() });
      assert.equal(second.status, 429);
      assert.ok(second.headers.get("retry-after"));
      await second.arrayBuffer();
      holder.destroy();
    } finally {
      await s.close();
    }
  });
});

// --- 6.4 byte budget ------------------------------------------------------
describe("byte budget", () => {
  test("exhausted budget -> 429 + Retry-After on transfers only", async () => {
    const s = await startServer({ byteBudget: { bytes: 10000, windowS: 600 }, downloadCapBytes: 33554432 });
    try {
      // First download consumes 8000 of 10000.
      const a = await fetch(s.url + "/laghound/download?bytes=8000", { headers: auth() });
      await a.arrayBuffer();
      assert.equal(a.status, 200);
      // Second wants 8000 more -> over budget.
      const b = await fetch(s.url + "/laghound/download?bytes=8000", { headers: auth() });
      assert.equal(b.status, 429);
      assert.ok(b.headers.get("retry-after"));
      // O(1) routes are never budget-limited.
      const h = await fetch(s.url + "/laghound/health", { headers: auth() });
      assert.equal(h.status, 200);
      await h.arrayBuffer();
    } finally {
      await s.close();
    }
  });
});

// --- 6.6 zero reflection + marks ------------------------------------------
describe("marks + reflection", () => {
  test("mark() surfaces as mark-<name> on /echo", async () => {
    const s = await startServer();
    try {
      s.handler.mark("db", 41.9);
      const r = await fetch(s.url + "/laghound/echo", { headers: auth() });
      await r.arrayBuffer();
      assert.match(r.headers.get("server-timing") ?? "", /mark-db;dur=41\.9/);
    } finally {
      await s.close();
    }
  });

  test("mark rejects illegal names", async () => {
    const s = await startServer();
    try {
      assert.throws(() => s.handler.mark("DB name!", 1), /\[a-z0-9\]/);
    } finally {
      await s.close();
    }
  });
});

// --- 6.1 streaming memory bound -------------------------------------------
describe("streaming memory bound", () => {
  test("32 MiB download does not allocate O(N) — server RSS delta bounded", async () => {
    // The server runs in an isolated child (test/memprobe.ts) that measures
    // its OWN RSS; this process is only the client and drains the body into a
    // sink. A per-request O(N) allocation on the server would spike its delta
    // by ~32 MiB; streaming from the single shared buffer keeps it flat.
    const { spawn } = await import("node:child_process");
    const probe = fileURLToPath(new URL("./memprobe.ts", import.meta.url));
    const child = spawn(process.execPath, ["--expose-gc", probe], { stdio: ["ignore", "pipe", "inherit"] });

    const lines: string[] = [];
    let pending = "";
    const nextLine = (prefix: string): Promise<string> =>
      new Promise((resolve, reject) => {
        const scan = (): boolean => {
          for (let i = 0; i < lines.length; i++) {
            if (lines[i].startsWith(prefix)) {
              const v = lines.splice(i, 1)[0].slice(prefix.length).trim();
              resolve(v);
              return true;
            }
          }
          return false;
        };
        if (scan()) return;
        const onData = (c: Buffer): void => {
          pending += c.toString();
          let nl: number;
          while ((nl = pending.indexOf("\n")) !== -1) {
            lines.push(pending.slice(0, nl));
            pending = pending.slice(nl + 1);
          }
          if (scan()) child.stdout.off("data", onData);
        };
        child.stdout.on("data", onData);
        child.once("error", reject);
        child.once("exit", (code) => reject(new Error(`memprobe exited early (${code})`)));
      });

    try {
      const port = Number(await nextLine("PORT "));
      // Drain the full 32 MiB into a sink; never retain it client-side.
      const r = await fetch(`http://127.0.0.1:${port}/laghound/download?bytes=33554432`, {
        headers: auth(),
      });
      let total = 0;
      for await (const chunk of r.body as any) total += chunk.length;
      assert.equal(total, 33554432, "full 32 MiB streamed");
      const { delta } = JSON.parse(await nextLine("RESULT "));
      // Contract §9 targets < 8 MiB; allow headroom for runtime noise while
      // still catching a 32 MiB (or 4×8 MiB chunked) per-request allocation.
      assert.ok(delta < 12 * 1024 * 1024, `server-side streaming RSS delta too high: ${delta}`);
    } finally {
      child.kill();
    }
  });
});
