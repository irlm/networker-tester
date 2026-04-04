"use strict";

const http2 = require("node:http2");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const url = require("node:url");
const zlib = require("node:zlib");

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------
const CERT_DIR = process.env.BENCH_CERT_DIR || "/opt/bench";
const PORT = parseInt(process.env.PORT || "8443", 10);

// ---------------------------------------------------------------------------
// Shared benchmark dataset
// ---------------------------------------------------------------------------
let BENCH_DATA = null;

function loadBenchData() {
  const candidates = [];
  if (process.env.BENCH_DATA_PATH) {
    candidates.push(process.env.BENCH_DATA_PATH);
  }
  candidates.push("/opt/bench/bench-data.json");
  candidates.push(path.join(__dirname, "..", "shared", "bench-data.json"));

  for (let i = 0; i < candidates.length; i++) {
    try {
      const raw = fs.readFileSync(candidates[i], "utf8");
      BENCH_DATA = JSON.parse(raw);
      console.log(
        "Loaded bench-data.json from " + candidates[i] +
        " (version " + BENCH_DATA._version +
        ", " + (BENCH_DATA.users || []).length + " users" +
        ", " + (BENCH_DATA.search_corpus || []).length + " corpus" +
        ", " + (BENCH_DATA.timeseries || []).length + " timeseries)"
      );
      return;
    } catch (_e) {
      // try next path
    }
  }
  console.log("WARN: bench-data.json not found, falling back to per-language PRNG");
}

loadBenchData();

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
// Seeded PRNG (mulberry32) — deterministic output across runs
// ---------------------------------------------------------------------------

function mulberry32(seed) {
  let s = seed | 0;
  return function () {
    s = (s + 0x6d2b79f5) | 0;
    let t = Math.imul(s ^ (s >>> 15), 1 | s);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ---------------------------------------------------------------------------
// JSON API response helpers
// ---------------------------------------------------------------------------

function apiJsonResponse(res, status, obj, startHr) {
  const elapsed = process.hrtime(startHr);
  const durationMs = elapsed[0] * 1000 + elapsed[1] / 1e6;
  const body = JSON.stringify(obj);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    "server-timing": "app;dur=" + durationMs.toFixed(1),
    "cache-control": "no-store, no-cache, must-revalidate",
    "timing-allow-origin": "*",
    "access-control-allow-origin": "*",
  });
  res.end(body);
}

function apiJsonResponseH2(stream, status, obj, startHr) {
  const elapsed = process.hrtime(startHr);
  const durationMs = elapsed[0] * 1000 + elapsed[1] / 1e6;
  const body = JSON.stringify(obj);
  stream.respond({
    ":status": status,
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    "server-timing": "app;dur=" + durationMs.toFixed(1),
    "cache-control": "no-store, no-cache, must-revalidate",
    "timing-allow-origin": "*",
    "access-control-allow-origin": "*",
  });
  stream.end(body);
}

// ---------------------------------------------------------------------------
// Shared data for JSON API endpoints
// ---------------------------------------------------------------------------

const FIRST_NAMES = [
  "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Heidi",
  "Ivan", "Judy", "Karl", "Laura", "Mallory", "Nina", "Oscar", "Peggy",
  "Quentin", "Ruth", "Steve", "Trent", "Ursula", "Victor", "Wendy",
  "Xander", "Yvonne", "Zack",
];
const LAST_NAMES = [
  "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller",
  "Davis", "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez",
  "Wilson", "Anderson", "Thomas", "Taylor", "Moore", "Jackson", "Martin",
];
const DEPARTMENTS = [
  "Engineering", "Marketing", "Sales", "Finance", "HR",
  "Operations", "Legal", "Support", "Design", "Product",
];

function generateUsers(rng, count) {
  const users = [];
  for (let i = 0; i < count; i++) {
    users.push({
      id: i + 1,
      name: FIRST_NAMES[Math.floor(rng() * FIRST_NAMES.length)] + " " +
            LAST_NAMES[Math.floor(rng() * LAST_NAMES.length)],
      email: "user" + (i + 1) + "@example.com",
      age: 22 + Math.floor(rng() * 44),
      department: DEPARTMENTS[Math.floor(rng() * DEPARTMENTS.length)],
      score: Math.round(rng() * 10000) / 100,
    });
  }
  return users;
}

const SEARCH_WORDS = [
  "network", "latency", "throughput", "bandwidth", "packet",
  "routing", "firewall", "proxy", "endpoint", "server",
  "client", "protocol", "socket", "buffer", "stream",
  "timeout", "retry", "cache", "queue", "load",
];

// Simple Box-Muller for Gaussian
function gaussianRng(rng, mean, stddev) {
  const u1 = rng();
  const u2 = rng();
  const z = Math.sqrt(-2.0 * Math.log(u1)) * Math.cos(2.0 * Math.PI * u2);
  return mean + stddev * z;
}

// CRC32 (IEEE) lookup table
const CRC32_TABLE = (function () {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let j = 0; j < 8; j++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[i] = c;
  }
  return table;
})();

function crc32(buf) {
  let crc = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    crc = CRC32_TABLE[(crc ^ buf[i]) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

// ---------------------------------------------------------------------------
// GET /api/users?page=N&sort=field&order=asc|desc
// ---------------------------------------------------------------------------

function handleApiUsers(parsedUrl, respond) {
  const startHr = process.hrtime();
  const params = parsedUrl.query || {};
  const page = Math.max(1, parseInt(params.page, 10) || 1);
  const sortField = params.sort || "id";
  const order = params.order || "asc";

  var users;
  if (BENCH_DATA && BENCH_DATA.users && BENCH_DATA.users.length > 0) {
    users = BENCH_DATA.users.slice(); // shallow copy for sorting
  } else {
    const rng = mulberry32(page);
    users = generateUsers(rng, 100);
  }

  const validFields = ["id", "name", "email", "age", "department", "score"];
  if (validFields.includes(sortField)) {
    users.sort(function (a, b) {
      const va = a[sortField];
      const vb = b[sortField];
      if (typeof va === "string") return va.localeCompare(vb);
      return va - vb;
    });
  }
  if (order === "desc") users.reverse();

  const pageSize = 20;
  const startIdx = (page - 1) * pageSize;
  const pageUsers = users.slice(startIdx, startIdx + pageSize);

  respond(200, {
    page: page,
    page_size: pageSize,
    total: users.length,
    sort: sortField,
    order: order,
    users: pageUsers,
  }, startHr);
}

// ---------------------------------------------------------------------------
// POST /api/transform — SHA-256 hash string fields, reverse arrays
// ---------------------------------------------------------------------------

function handleApiTransform(bodyBuf, respond) {
  const startHr = process.hrtime();
  var body;
  try {
    body = JSON.parse(bodyBuf.toString());
  } catch (_e) {
    return respond(400, { error: "invalid JSON" }, startHr);
  }

  const transformed = {};
  const keys = Object.keys(body);
  for (let i = 0; i < keys.length; i++) {
    const key = keys[i];
    const value = body[key];
    if (typeof value === "string") {
      const hashed = crypto.createHash("sha256").update(value).digest("hex");
      transformed[key] = { original_reversed: value.split("").reverse().join(""), sha256: hashed };
    } else if (Array.isArray(value)) {
      transformed[key] = value.slice().reverse();
    } else {
      transformed[key] = value;
    }
  }

  respond(200, {
    original_fields: keys.length,
    transformed: transformed,
  }, startHr);
}

// ---------------------------------------------------------------------------
// GET /api/aggregate?range=start,end — stats over 10,000 generated points
// ---------------------------------------------------------------------------

function handleApiAggregate(parsedUrl, respond) {
  const startHr = process.hrtime();
  const rangeParam = (parsedUrl.query || {}).range || "0,1000";
  const parts = rangeParam.split(",");
  const rangeStart = parseInt(parts[0], 10) || 0;
  const rangeEnd = parseInt(parts[1], 10) || 1000;

  var values;
  if (BENCH_DATA && BENCH_DATA.timeseries && BENCH_DATA.timeseries.length > 0) {
    values = BENCH_DATA.timeseries.slice();
  } else {
    const rng = mulberry32(rangeStart);
    values = [];
    for (let i = 0; i < 10000; i++) {
      values.push(gaussianRng(rng, 50, 15));
    }
  }

  const count = values.length;
  const categories = ["alpha", "beta", "gamma", "delta", "epsilon"];
  const catRng = mulberry32(rangeStart + 1);
  const assignments = [];

  for (let i = 0; i < count; i++) {
    assignments.push(categories[Math.floor(catRng() * categories.length)]);
  }

  const sorted = values.slice().sort(function (a, b) { return a - b; });
  const mean = values.reduce(function (s, v) { return s + v; }, 0) / count;
  const p50 = sorted[Math.floor(count / 2)];
  const p95 = sorted[Math.floor(count * 0.95)];
  const maxVal = sorted[count - 1];

  const groups = {};
  for (let ci = 0; ci < categories.length; ci++) {
    const cat = categories[ci];
    const catVals = [];
    for (let i = 0; i < count; i++) {
      if (assignments[i] === cat) catVals.push(values[i]);
    }
    if (catVals.length > 0) {
      const catSorted = catVals.slice().sort(function (a, b) { return a - b; });
      groups[cat] = {
        count: catVals.length,
        mean: Math.round((catVals.reduce(function (s, v) { return s + v; }, 0) / catVals.length) * 10000) / 10000,
        p50: Math.round(catSorted[Math.floor(catSorted.length / 2)] * 10000) / 10000,
        max: Math.round(catSorted[catSorted.length - 1] * 10000) / 10000,
      };
    }
  }

  respond(200, {
    range: { start: rangeStart, end: rangeEnd },
    total_points: count,
    stats: {
      mean: Math.round(mean * 10000) / 10000,
      p50: Math.round(p50 * 10000) / 10000,
      p95: Math.round(p95 * 10000) / 10000,
      max: Math.round(maxVal * 10000) / 10000,
    },
    groups: groups,
  }, startHr);
}

// ---------------------------------------------------------------------------
// GET /api/search?q=term&limit=N — regex search over 1,000 generated strings
// ---------------------------------------------------------------------------

function handleApiSearch(parsedUrl, respond) {
  const startHr = process.hrtime();
  const params = parsedUrl.query || {};
  const query = params.q || "test";
  var limit = parseInt(params.limit, 10) || 10;
  if (limit < 1 || limit > 100) limit = 10;

  var corpus;
  if (BENCH_DATA && BENCH_DATA.search_corpus && BENCH_DATA.search_corpus.length > 0) {
    corpus = [];
    for (let i = 0; i < BENCH_DATA.search_corpus.length; i++) {
      corpus.push({ id: i + 1, text: BENCH_DATA.search_corpus[i] });
    }
  } else {
    const rng = mulberry32(42);
    corpus = [];
    for (let i = 0; i < 1000; i++) {
      const wordCount = 3 + Math.floor(rng() * 6);
      const words = [];
      for (let j = 0; j < wordCount; j++) {
        words.push(SEARCH_WORDS[Math.floor(rng() * SEARCH_WORDS.length)]);
      }
      corpus.push({ id: i + 1, text: words.join(" ") });
    }
  }

  var pattern;
  try {
    pattern = new RegExp(query, "i");
  } catch (_e) {
    return respond(400, { error: "invalid regex" }, startHr);
  }

  const results = [];
  for (let i = 0; i < corpus.length; i++) {
    const match = pattern.exec(corpus[i].text);
    if (match) {
      const score = Math.round((1.0 / (1 + match.index)) * 10000) / 10000;
      results.push({ id: corpus[i].id, text: corpus[i].text, score: score });
    }
  }

  results.sort(function (a, b) { return b.score - a.score; });
  const limited = results.slice(0, limit);

  respond(200, {
    query: query,
    total_matches: results.length,
    limit: limit,
    results: limited,
  }, startHr);
}

// ---------------------------------------------------------------------------
// POST /api/upload/process — CRC32 + SHA-256 + zlib compress
// ---------------------------------------------------------------------------

function handleApiUploadProcess(bodyBuf, respond) {
  const startHr = process.hrtime();

  const crcVal = crc32(bodyBuf);
  const sha = crypto.createHash("sha256").update(bodyBuf).digest("hex");
  const compressed = zlib.deflateSync(bodyBuf);

  respond(200, {
    original_size: bodyBuf.length,
    compressed_size: compressed.length,
    compression_ratio: Math.round((compressed.length / Math.max(bodyBuf.length, 1)) * 10000) / 10000,
    crc32: ("00000000" + crcVal.toString(16)).slice(-8),
    sha256: sha,
  }, startHr);
}

// ---------------------------------------------------------------------------
// GET /api/delayed?ms=N&work=light — clamped setTimeout, return actual duration
// ---------------------------------------------------------------------------

function handleApiDelayed(parsedUrl, respond) {
  const startHr = process.hrtime();
  const params = parsedUrl.query || {};
  var ms = parseInt(params.ms, 10) || 50;
  if (ms < 1) ms = 1;
  if (ms > 100) ms = 100;
  const work = params.work || "none";

  setTimeout(function () {
    if (work === "light") {
      crypto.createHash("sha256").update("benchmark".repeat(100)).digest("hex");
    }

    const elapsed = process.hrtime(startHr);
    const actualMs = Math.round((elapsed[0] * 1000 + elapsed[1] / 1e6) * 100) / 100;

    respond(200, {
      requested_ms: ms,
      actual_ms: actualMs,
      work: work,
    }, startHr);
  }, ms);
}

// ---------------------------------------------------------------------------
// GET /api/validate?seed=42 — checksums for deterministic verification
// ---------------------------------------------------------------------------

function handleApiValidate(parsedUrl, respond) {
  const startHr = process.hrtime();
  const params = parsedUrl.query || {};
  const seed = parseInt(params.seed, 10) || 42;

  // If shared data is loaded, return pre-computed checksums.
  if (BENCH_DATA && BENCH_DATA.expected_checksums) {
    const checksums = Object.assign({}, BENCH_DATA.expected_checksums);
    respond(200, { seed: seed, checksums: checksums }, startHr);
    return;
  }

  // PRNG fallback.
  // Users checksum (page=1)
  const usersRng = mulberry32(1);
  const users = generateUsers(usersRng, 100);
  const usersHash = crypto.createHash("sha256")
    .update(JSON.stringify(users))
    .digest("hex");

  // Aggregate checksum (start=0)
  const aggRng = mulberry32(0);
  const aggValues = [];
  for (let i = 0; i < 10000; i++) {
    aggValues.push(gaussianRng(aggRng, 50, 15));
  }
  const aggSorted = aggValues.slice().sort(function (a, b) { return a - b; });
  const aggHash = crypto.createHash("sha256")
    .update(JSON.stringify(aggSorted.map(function (v) { return Math.round(v * 10000) / 10000; })))
    .digest("hex");

  // Search corpus checksum (seed=42)
  const searchRng = mulberry32(42);
  const corpus = [];
  for (let i = 0; i < 1000; i++) {
    const wordCount = 3 + Math.floor(searchRng() * 6);
    const words = [];
    for (let j = 0; j < wordCount; j++) {
      words.push(SEARCH_WORDS[Math.floor(searchRng() * SEARCH_WORDS.length)]);
    }
    corpus.push(words.join(" "));
  }
  const searchHash = crypto.createHash("sha256")
    .update(JSON.stringify(corpus))
    .digest("hex");

  respond(200, {
    seed: seed,
    checksums: {
      users_page1: usersHash.slice(0, 16),
      aggregate_start0: aggHash.slice(0, 16),
      search_corpus: searchHash.slice(0, 16),
    },
  }, startHr);
}

// ---------------------------------------------------------------------------
// Collect request body helper
// ---------------------------------------------------------------------------

function collectBody(readable, callback) {
  const chunks = [];
  readable.on("data", function (chunk) { chunks.push(chunk); });
  readable.on("end", function () { callback(Buffer.concat(chunks)); });
}

// ---------------------------------------------------------------------------
// API route dispatcher (shared by H1 and H2)
// ---------------------------------------------------------------------------

function dispatchApi(method, urlPath, parsedUrl, getBody, respond) {
  if (method === "GET" && urlPath === "/api/users") {
    handleApiUsers(parsedUrl, respond);
    return true;
  }
  if (method === "POST" && urlPath === "/api/transform") {
    getBody(function (buf) { handleApiTransform(buf, respond); });
    return true;
  }
  if (method === "GET" && urlPath === "/api/aggregate") {
    handleApiAggregate(parsedUrl, respond);
    return true;
  }
  if (method === "GET" && urlPath === "/api/search") {
    handleApiSearch(parsedUrl, respond);
    return true;
  }
  if (method === "POST" && urlPath === "/api/upload/process") {
    getBody(function (buf) { handleApiUploadProcess(buf, respond); });
    return true;
  }
  if (method === "GET" && urlPath === "/api/delayed") {
    handleApiDelayed(parsedUrl, respond);
    return true;
  }
  if (method === "GET" && urlPath === "/api/validate") {
    handleApiValidate(parsedUrl, respond);
    return true;
  }
  return false;
}

// ---------------------------------------------------------------------------
// HTTP/2 stream dispatcher
// ---------------------------------------------------------------------------
function onStream(stream, headers) {
  const method = headers[":method"];
  const rawPath = headers[":path"];
  const urlPath = rawPath.split("?")[0];
  const parsedUrl = url.parse(rawPath, true);

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

  // JSON API endpoints
  const handled = dispatchApi(
    method, urlPath, parsedUrl,
    function (cb) { collectBody(stream, cb); },
    function (status, obj, startHr) { apiJsonResponseH2(stream, status, obj, startHr); }
  );
  if (handled) return;

  const body = JSON.stringify({ error: "not found" });
  stream.respond({ ":status": 404, "content-type": "application/json" });
  stream.end(body);
}

// ---------------------------------------------------------------------------
// HTTP/1.1 request dispatcher (fires for allowHTTP1 fallback connections)
// ---------------------------------------------------------------------------
function onRequest(req, res) {
  const method = req.method;
  const rawUrl = req.url;
  const urlPath = rawUrl.split("?")[0];
  const parsedUrl = url.parse(rawUrl, true);

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
    req.on("data", function (chunk) {
      total += chunk.length;
    });
    req.on("end", function () {
      jsonResponse(res, 200, { bytes_received: total });
    });
    return;
  }

  // JSON API endpoints
  const handled = dispatchApi(
    method, urlPath, parsedUrl,
    function (cb) { collectBody(req, cb); },
    function (status, obj, startHr) { apiJsonResponse(res, status, obj, startHr); }
  );
  if (handled) return;

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
