"use strict";

const http2 = require("node:http2");
const fs = require("node:fs");
const path = require("node:path");

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------
const CERT_DIR = process.env.BENCH_CERT_DIR || "/opt/bench";
const PORT = parseInt(process.env.PORT || "8443", 10);

// Pre-allocate a single 8 KiB buffer filled with 0x42 ('B') for download
// streaming. Reusing one buffer avoids per-request allocation pressure.
const CHUNK = Buffer.alloc(8192, 0x42);

// ---------------------------------------------------------------------------
// TLS credentials
// ---------------------------------------------------------------------------
const tlsOptions = {
  cert: fs.readFileSync(path.join(CERT_DIR, "cert.pem")),
  key: fs.readFileSync(path.join(CERT_DIR, "key.pem")),
  allowHTTP1: true, // accept both HTTP/1.1 and HTTP/2
};

// ---------------------------------------------------------------------------
// Route: GET /health
// ---------------------------------------------------------------------------
function handleHealth(stream, headers) {
  const body = JSON.stringify({
    status: "ok",
    runtime: "nodejs",
    version: process.version,
  });
  stream.respond({
    ":status": 200,
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
  });
  stream.end(body);
}

// ---------------------------------------------------------------------------
// Route: GET /download/:size
// ---------------------------------------------------------------------------
function handleDownload(stream, headers, size) {
  if (size <= 0 || !Number.isFinite(size)) {
    const err = JSON.stringify({ error: "invalid size" });
    stream.respond({ ":status": 400, "content-type": "application/json" });
    stream.end(err);
    return;
  }

  stream.respond({
    ":status": 200,
    "content-type": "application/octet-stream",
    "content-length": size,
  });

  let remaining = size;

  function write() {
    let ok = true;
    while (remaining > 0 && ok) {
      const toSend = Math.min(remaining, CHUNK.length);
      const slice = toSend === CHUNK.length ? CHUNK : CHUNK.subarray(0, toSend);
      remaining -= toSend;

      if (remaining === 0) {
        stream.end(slice);
        return;
      }
      ok = stream.write(slice);
    }
    // Backpressure: wait for drain before continuing
    if (remaining > 0) {
      stream.once("drain", write);
    }
  }

  write();
}

// ---------------------------------------------------------------------------
// Route: POST /upload
// ---------------------------------------------------------------------------
function handleUpload(stream, headers) {
  let total = 0;

  stream.on("data", (chunk) => {
    total += chunk.length;
  });

  stream.on("end", () => {
    const body = JSON.stringify({ bytes_received: total });
    stream.respond({
      ":status": 200,
      "content-type": "application/json",
      "content-length": Buffer.byteLength(body),
    });
    stream.end(body);
  });
}

// ---------------------------------------------------------------------------
// Request dispatcher
// ---------------------------------------------------------------------------
function onStream(stream, headers) {
  const method = headers[":method"];
  const urlPath = headers[":path"];

  // GET /health
  if (method === "GET" && urlPath === "/health") {
    return handleHealth(stream, headers);
  }

  // GET /download/:size
  if (method === "GET" && urlPath.startsWith("/download/")) {
    const size = parseInt(urlPath.slice("/download/".length), 10);
    return handleDownload(stream, headers, size);
  }

  // POST /upload
  if (method === "POST" && urlPath === "/upload") {
    return handleUpload(stream, headers);
  }

  // 404
  const body = JSON.stringify({ error: "not found" });
  stream.respond({ ":status": 404, "content-type": "application/json" });
  stream.end(body);
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------
const server = http2.createSecureServer(tlsOptions);

server.on("stream", onStream);

server.on("error", (err) => {
  console.error("server error:", err.message);
});

server.listen(PORT, () => {
  console.log(`nodejs reference-api listening on https://0.0.0.0:${PORT}`);
});
