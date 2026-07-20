// Smoke test for the sample app (example/server.mjs): it must start, serve its
// two app routes, mount LagHound at /laghound behind the demo token, and keep
// the routes invisible (bare 404) without it. Spawned as a child process so
// this exercises the real script exactly as `node example/server.mjs` would.
import { test, describe, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../example/server.mjs", import.meta.url));
const TOKEN = "demo-token-laghound";
const PORT = 18082;
const BASE = `http://127.0.0.1:${PORT}`;

let child: ChildProcess;

before(async () => {
  child = spawn(process.execPath, [script], {
    env: { ...process.env, PORT: String(PORT), LAGHOUND_TOKEN: TOKEN },
    stdio: ["ignore", "pipe", "inherit"],
  });
  // Wait for the "listening" log line before probing.
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("sample did not start in time")), 8000);
    child.stdout!.on("data", (c: Buffer) => {
      if (c.toString().includes("listening")) {
        clearTimeout(timer);
        resolve();
      }
    });
    child.once("error", reject);
    child.once("exit", (code) => reject(new Error(`sample exited early (${code})`)));
  });
});

after(() => {
  child.kill();
});

describe("sample app", () => {
  test("GET / -> js sample ok", async () => {
    const r = await fetch(`${BASE}/`);
    assert.equal(r.status, 200);
    assert.equal((await r.text()).trim(), "js sample ok");
  });

  test("GET /work -> done after a delay", async () => {
    const t0 = Date.now();
    const r = await fetch(`${BASE}/work`);
    assert.equal(r.status, 200);
    assert.equal((await r.text()).trim(), "done");
    assert.ok(Date.now() - t0 >= 25, "should take ~30ms");
  });

  test("LagHound mounted at /laghound behind the demo token", async () => {
    const r = await fetch(`${BASE}/laghound/health`, { headers: { "x-laghound-token": TOKEN } });
    assert.equal(r.status, 200);
    const j = await r.json();
    assert.equal(j.contract, "v1");
    assert.equal(j.app, "js-sample");
  });

  test("LagHound routes are invisible without the token (bare 404)", async () => {
    const r = await fetch(`${BASE}/laghound/health`);
    assert.equal(r.status, 404);
    assert.equal((await r.arrayBuffer()).byteLength, 0);
  });

  test("/work records a mark surfaced on the next /echo", async () => {
    await fetch(`${BASE}/work`); // records mark-work
    const r = await fetch(`${BASE}/laghound/echo`, { headers: { "x-laghound-token": TOKEN } });
    await r.arrayBuffer();
    assert.match(r.headers.get("server-timing") ?? "", /mark-work;dur=/);
  });
});
