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
  allowHTTP1: true,
  ALPNProtocols: ["h2", "http/1.1"], // accept both HTTP/1.1 and HTTP/2
};

// ---------------------------------------------------------------------------
// Shared response helpers (work with both HTTP/2 streams and HTTP/1.1 res)
// ---------------------------------------------------------------------------

function jsonResponse(res, status, obj) {
  const body = JSON.stringify(obj);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
  });
  res.end(body);
}

// ---------------------------------------------------------------------------
// Route: GET /health
// ---------------------------------------------------------------------------
function handleHealthH2(stream) {
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
function handleDownloadH2(stream, size) {
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

  streamBytes(stream, size);
}

// ---------------------------------------------------------------------------
// Route: POST /upload
// ---------------------------------------------------------------------------
function handleUploadH2(stream) {
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
// Shared byte-streaming helper (backpressure-aware)
// ---------------------------------------------------------------------------
function streamBytes(writable, size) {
  let remaining = size;

  function write() {
    let ok = true;
    while (remaining > 0 && ok) {
      const toSend = Math.min(remaining, CHUNK.length);
      const slice = toSend === CHUNK.length ? CHUNK : CHUNK.subarray(0, toSend);
      remaining -= toSend;

      if (remaining === 0) {
        writable.end(slice);
        return;
      }
      ok = writable.write(slice);
    }
    if (remaining > 0) {
      writable.once("drain", write);
    }
  }

  write();
}

// ---------------------------------------------------------------------------
// HTTP/2 stream dispatcher
// ---------------------------------------------------------------------------
function onStream(stream, headers) {
  const method = headers[":method"];
  const urlPath = headers[":path"];

  if (method === "GET" && urlPath === "/health") {
    return handleHealthH2(stream);
  }

  if (method === "GET" && urlPath.startsWith("/download/")) {
    const size = parseInt(urlPath.slice("/download/".length), 10);
    return handleDownloadH2(stream, size);
  }

  if (method === "POST" && urlPath === "/upload") {
    return handleUploadH2(stream);
  }

  const body = JSON.stringify({ error: "not found" });
  stream.respond({ ":status": 404, "content-type": "application/json" });
  stream.end(body);
}

// ---------------------------------------------------------------------------
// HTTP/1.1 request dispatcher (fires for allowHTTP1 fallback connections)
// ---------------------------------------------------------------------------
function onRequest(req, res) {
  const method = req.method;
  const urlPath = req.url;

  if (method === "GET" && urlPath === "/health") {
    return jsonResponse(res, 200, {
      status: "ok",
      runtime: "nodejs",
      version: process.version,
    });
  }

  if (method === "GET" && urlPath.startsWith("/download/")) {
    const size = parseInt(urlPath.slice("/download/".length), 10);
    if (size <= 0 || !Number.isFinite(size)) {
      return jsonResponse(res, 400, { error: "invalid size" });
    }
    res.writeHead(200, {
      "content-type": "application/octet-stream",
      "content-length": size,
    });
    return streamBytes(res, size);
  }

  if (method === "POST" && urlPath === "/upload") {
    let total = 0;
    req.on("data", (chunk) => {
      total += chunk.length;
    });
    req.on("end", () => {
      jsonResponse(res, 200, { bytes_received: total });
    });
    return;
  }

  jsonResponse(res, 404, { error: "not found" });
}

// ---------------------------------------------------------------------------
// Server — handles both HTTP/2 (stream event) and HTTP/1.1 (request event)
// ---------------------------------------------------------------------------
const server = http2.createSecureServer(tlsOptions);

server.on("stream", onStream);
server.on("request", onRequest);

server.on("error", (err) => {
  console.error("server error:", err.message);
});

server.listen(PORT, () => {
  console.log(`nodejs reference-api listening on https://0.0.0.0:${PORT}`);
});
