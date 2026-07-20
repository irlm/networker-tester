// Isolated server-process memory probe for the streaming bound (contract §9).
//
// This child runs ONLY the LagHound server. A separate client process (the
// parent test) drains a 32 MiB download from it. The child samples its own
// RSS when the request arrives and again after the response has fully
// flushed, then prints {port, delta} as JSON on a control line. Because the
// 32 MiB never lives in this process's heap (it streams from the single
// shared FILL buffer out through the socket), the delta stays flat; a
// per-request O(N) allocation would spike it by ~32 MiB.
import { createServer } from "node:http";
import { laghound } from "../src/index.ts";

const TOKEN = "conformance-test-token-0123456789";
const BYTES = 33554432; // 32 MiB — abs-max config

const gc = (): void => {
  global.gc?.();
};

const handler = laghound({ token: TOKEN, downloadCapBytes: BYTES });
const server = createServer((req, res) => {
  let before = 0;
  if ((req.url ?? "").startsWith("/laghound/download")) {
    gc();
    before = process.memoryUsage().rss;
    res.on("finish", () => {
      gc();
      const delta = process.memoryUsage().rss - before;
      // Control line consumed by the parent.
      process.stdout.write("RESULT " + JSON.stringify({ delta }) + "\n");
      server.close();
    });
  }
  handler(req, res);
});

server.listen(0, "127.0.0.1", () => {
  const addr = server.address();
  const port = typeof addr === "object" && addr !== null ? addr.port : 0;
  process.stdout.write("PORT " + port + "\n");
});
