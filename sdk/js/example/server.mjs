// A tiny "real service" that embeds @laghound/endpoint at /laghound.
//
// Bare node:http so it runs with zero installs (the SDK itself has zero
// runtime deps). Two ordinary app routes sit alongside the mounted LagHound
// endpoint:
//
//   GET /       -> "js sample ok"
//   GET /work   -> "done" after ~30ms of simulated work
//   /laghound/* -> the LagHound diagnostic endpoint (contract v1)
//
// Run:
//   node example/server.mjs
//   # then, with the token:
//   curl -H "X-LagHound-Token: demo-token-laghound" localhost:8082/laghound/health
//
// Uses the built package when present (dist/), else the TypeScript source
// directly (Node >= 22.6 strips types), so `node example/server.mjs` works
// straight from a checkout without a build step.
import http from "node:http";
import { createRequire } from "node:module";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

const distEsm = fileURLToPath(new URL("../dist/esm/index.js", import.meta.url));
const { laghound } = existsSync(distEsm)
  ? await import(distEsm)
  : await import("../src/index.ts");

const TOKEN = process.env.LAGHOUND_TOKEN ?? "demo-token-laghound";
const PORT = Number(process.env.PORT ?? 8082);

// Mount LagHound. Optional: label the app + record a custom Server-Timing mark.
const lh = laghound({ token: TOKEN, appName: "js-sample" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const server = http.createServer(async (req, res) => {
  // LagHound owns everything under /laghound.
  if (req.url === "/laghound" || req.url?.startsWith("/laghound/") || req.url?.startsWith("/laghound?")) {
    return lh(req, res);
  }

  const path = (req.url ?? "/").split("?")[0];

  if (req.method === "GET" && path === "/") {
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("js sample ok\n");
    return;
  }

  if (req.method === "GET" && path === "/work") {
    const t0 = process.hrtime.bigint();
    await sleep(30); // simulate ~30ms of real work
    const ms = Number(process.hrtime.bigint() - t0) / 1e6;
    // Surface the work time to LagHound reports as a custom mark.
    lh.mark("work", ms);
    res.writeHead(200, { "content-type": "text/plain" });
    res.end("done\n");
    return;
  }

  res.writeHead(404, { "content-type": "text/plain" });
  res.end("not found\n");
});

server.listen(PORT, () => {
  console.log(`js sample listening on http://localhost:${PORT}`);
  console.log(`  GET /                         -> "js sample ok"`);
  console.log(`  GET /work                     -> ~30ms delay`);
  console.log(`  GET /laghound/health          (needs X-LagHound-Token: ${TOKEN})`);
});
