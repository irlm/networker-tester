"use strict";

// Networker Bench Node.js reference API.
// Conforms to the frozen contract in benchmarks/shared/API-SPEC.md (family C).
// Scaling (§3): cluster module, BENCH_WORKERS worker processes (default = cores).

const cluster = require("node:cluster");
const http = require("node:http");
const http2 = require("node:http2");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const url = require("node:url");
const zlib = require("node:zlib");

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------
const CERT_DIR = process.env.BENCH_CERT_DIR || "/opt/bench";
const PORT = parseInt(process.env.BENCH_PORT || process.env.PORT || "8443", 10);
const BENCH_API_TOKEN = process.env.BENCH_API_TOKEN || "";
const NPROC = os.availableParallelism ? os.availableParallelism() : os.cpus().length;
const WORKERS = (function () {
  const raw = process.env.BENCH_WORKERS;
  if (raw === undefined || raw === "") return NPROC;
  const n = parseInt(raw, 10);
  if (!Number.isInteger(n) || n < 1) {
    process.stderr.write(`FATAL: BENCH_WORKERS=${raw} is not a positive integer\n`);
    process.exit(1);
  }
  return n;
})();

const MAX_DOWNLOAD = 2147483648; // §5.2: 2 GiB clamp

// ---------------------------------------------------------------------------
// Structured stderr logger (LOG_LEVEL, LOG_FORMAT env vars)
// ---------------------------------------------------------------------------
const LOG_LEVEL = (process.env.LOG_LEVEL || "info").toLowerCase();
const LOG_FORMAT = (process.env.LOG_FORMAT || "text").toLowerCase();
const LOG_SERVICE = process.env.LOG_SERVICE || "nodejs";
const LOG_LEVELS = { error: 0, warn: 1, info: 2, debug: 3 };
const currentLogLevel = LOG_LEVELS[LOG_LEVEL] !== undefined ? LOG_LEVELS[LOG_LEVEL] : 2;

function _logWrite(level, message, fields) {
  if (LOG_FORMAT === "json") {
    var entry = { ts: new Date().toISOString(), service: LOG_SERVICE, level: level, message: message };
    if (fields && Object.keys(fields).length > 0) entry.fields = fields;
    process.stderr.write(JSON.stringify(entry) + "\n");
  } else {
    var prefix = "[" + new Date().toISOString() + "] " + level.toUpperCase();
    var extra = (fields && Object.keys(fields).length > 0) ? " " + JSON.stringify(fields) : "";
    process.stderr.write(prefix + " " + message + extra + "\n");
  }
}

const log = {
  error: function (msg, fields) { if (currentLogLevel >= 0) _logWrite("error", msg, fields); },
  warn:  function (msg, fields) { if (currentLogLevel >= 1) _logWrite("warn",  msg, fields); },
  info:  function (msg, fields) { if (currentLogLevel >= 2) _logWrite("info",  msg, fields); },
  debug: function (msg, fields) { if (currentLogLevel >= 3) _logWrite("debug", msg, fields); },
};

// ---------------------------------------------------------------------------
// Shared benchmark dataset (API-SPEC.md §2) — load failure is FATAL.
// No PRNG fallback: silently benchmarking different input data poisons
// cross-language comparisons (audit F2/P0#2).
// ---------------------------------------------------------------------------
function fatal(msg) {
  process.stderr.write("FATAL: " + msg + "\n");
  process.exit(1);
}

function loadBenchData() {
  let chosen = null;
  const envPath = process.env.BENCH_DATA_PATH;
  if (envPath !== undefined && envPath !== "") {
    chosen = envPath; // must exist and parse — no fallback
  } else {
    const candidates = [
      "/opt/bench/bench-data.json",
      path.join(__dirname, "..", "shared", "bench-data.json"),
    ];
    for (const c of candidates) {
      if (fs.existsSync(c)) { chosen = c; break; } // first existing path wins
    }
    if (chosen === null) {
      fatal("bench-data.json not found (tried BENCH_DATA_PATH, /opt/bench/bench-data.json, " +
            "../shared/bench-data.json); the shared dataset is required — there is no PRNG fallback");
    }
  }

  let data;
  try {
    data = JSON.parse(fs.readFileSync(chosen, "utf8"));
  } catch (e) {
    fatal(`bench-data.json at ${chosen} could not be loaded: ${e.message}`);
  }

  // §2 schema verification — exit non-zero on mismatch.
  const checks = [
    [data._version === 2, `_version=${data._version}, want 2`],
    [Array.isArray(data.users) && data.users.length === 100, "users count != 100"],
    [Array.isArray(data.search_corpus) && data.search_corpus.length === 1000, "search_corpus count != 1000"],
    [Array.isArray(data.timeseries) && data.timeseries.length === 10000, "timeseries count != 10000"],
    [Array.isArray(data.transform_inputs) && data.transform_inputs.length === 10, "transform_inputs count != 10"],
    [data.expected_checksums && Object.keys(data.expected_checksums).length === 4, "expected_checksums keys != 4"],
  ];
  for (const [ok, msg] of checks) {
    if (!ok) fatal(`bench-data.json at ${chosen}: ${msg}`);
  }

  log.info("Loaded bench-data.json", { path: chosen, version: data._version });
  return data;
}

const BENCH_DATA = loadBenchData();
// Timeseries values cached in dataset order (§5.6 reads the `value` field —
// summing the raw objects was the NaN bug, audit F2). The per-request
// copy + sort + stats is the measured workload.
const TS_VALUES = BENCH_DATA.timeseries.map(function (p) { return p.value; });

// Pre-allocate a single 8 KiB buffer filled with 0x42 ('B') for download
// streaming (§5.2: fill byte and chunk size are part of the measured workload).
const CHUNK = Buffer.alloc(8192, 0x42);

// /health is constant-work (§5.1): the body is a byte-constant precomputed here.
const HEALTH_BODY = JSON.stringify({
  status: "ok",
  runtime: "nodejs",
  version: process.version,
});

// ---------------------------------------------------------------------------
// Number formatting for canonical-JSON compatibility
// ---------------------------------------------------------------------------
// JSON.stringify(39.0) emits "39", which the Python-canonicalizing validator
// parses as int — breaking the §7 checksums when a float field lands on an
// integral value (aggregate q2.mean = 39.0 in the frozen dataset). jnum()
// keeps a trailing ".0" for integral floats.
function jnum(x) {
  return Number.isInteger(x) ? x.toFixed(1) : String(x);
}

// r2: §5.6 rounding — half away from zero to 2 decimals.
function r2(x) {
  return Math.floor(x * 100 + 0.5) / 100;
}

// Bytewise-ordinal string compare (ASCII corpus/dataset; do NOT use
// localeCompare — the spec requires ordinal comparison).
function cmpStr(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

// ---------------------------------------------------------------------------
// TLS credentials
// ---------------------------------------------------------------------------
const certPath = path.join(CERT_DIR, "cert.pem");
const keyPath = path.join(CERT_DIR, "key.pem");
const USE_TLS = fs.existsSync(certPath) && fs.existsSync(keyPath);
const tlsOptions = USE_TLS ? {
  cert: fs.readFileSync(certPath),
  key: fs.readFileSync(keyPath),
  allowHTTP1: true,
  ALPNProtocols: ["h2", "http/1.1"],
} : null;

// ---------------------------------------------------------------------------
// Response adapters — normalize HTTP/1.1 res and HTTP/2 stream to one shape
// ---------------------------------------------------------------------------
function sendH1(res, status, headers, body) {
  res.writeHead(status, headers);
  res.end(body);
}

function sendH2(stream, status, headers, body) {
  const h = Object.assign({ ":status": status }, headers);
  stream.respond(h);
  stream.end(body);
}

// ---------------------------------------------------------------------------
// Auth check (§1): all routes except /health require the bearer token when
// BENCH_API_TOKEN is set. Returns true if authorized, false if 401 was sent.
// ---------------------------------------------------------------------------
function checkAuth(urlPath, headers, send) {
  if (!BENCH_API_TOKEN) return true;
  if (urlPath === "/health") return true;
  var auth = headers["authorization"] || "";
  if (auth === "Bearer " + BENCH_API_TOKEN) return true;
  const body = '{"error":"unauthorized"}';
  send(401, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
  }, body);
  return false;
}

// ---------------------------------------------------------------------------
// API response helper: §1 benchmark headers + Server-Timing app;dur (1 dp).
// `body` is a preserialized JSON string.
// ---------------------------------------------------------------------------
function apiSend(send, status, body, startHr) {
  const elapsed = process.hrtime(startHr);
  const durationMs = elapsed[0] * 1000 + elapsed[1] / 1e6;
  send(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    "server-timing": "app;dur=" + durationMs.toFixed(1),
    "cache-control": "no-store, no-cache, must-revalidate",
    "timing-allow-origin": "*",
    "access-control-allow-origin": "*",
  }, body);
}

function apiError(send, status, message, startHr) {
  apiSend(send, status, JSON.stringify({ error: message }), startHr);
}

// ---------------------------------------------------------------------------
// Shared byte-streaming helper (backpressure-aware)
// ---------------------------------------------------------------------------
function streamBytes(writable, size) {
  let remaining = size;
  if (remaining === 0) {
    writable.end();
    return;
  }

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
// Route: GET /health — byte-constant body (§5.1). Auth-exempt. Status 200 only.
// ---------------------------------------------------------------------------
function handleHealth(send) {
  send(200, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(HEALTH_BODY),
  }, HEALTH_BODY);
}

// ---------------------------------------------------------------------------
// Route: GET /download/{size} — exactly `size` bytes of 0x42 in 8 KiB chunks
// (§5.2). Non-integer → 400; values above 2 GiB are clamped.
// ---------------------------------------------------------------------------
function handleDownload(urlPath, send, writable) {
  const startHr = process.hrtime();
  const m = /^\/download\/(\d+)$/.exec(urlPath);
  if (!m) {
    const err = '{"error":"invalid size"}';
    send(400, {
      "content-type": "application/json",
      "content-length": Buffer.byteLength(err),
    }, null); // headers only; body written below via writable
    writable.end(err);
    return;
  }
  let size = Number(m[1]);
  if (size > MAX_DOWNLOAD) size = MAX_DOWNLOAD;

  const elapsed = process.hrtime(startHr);
  const procMs = elapsed[0] * 1000 + elapsed[1] / 1e6;
  send(200, {
    "content-type": "application/octet-stream",
    "content-length": size,
    "x-download-bytes": String(size),
    "server-timing": "proc;dur=" + procMs.toFixed(1),
  }, null);
  streamBytes(writable, size);
}

// ---------------------------------------------------------------------------
// Route: POST /upload — drain the body without buffering it (§5.3).
// ---------------------------------------------------------------------------
function handleUpload(readable, reqHeaders, send) {
  const startHr = process.hrtime();
  let total = 0;

  readable.on("data", function (chunk) {
    total += chunk.length;
  });

  readable.on("end", function () {
    const elapsed = process.hrtime(startHr);
    const recvMs = elapsed[0] * 1000 + elapsed[1] / 1e6;
    const body = JSON.stringify({ received_bytes: total });
    const headers = {
      "content-type": "application/json",
      "content-length": Buffer.byteLength(body),
      "x-networker-received-bytes": String(total),
      "server-timing": "recv;dur=" + recvMs.toFixed(1),
    };
    const reqId = reqHeaders["x-networker-request-id"];
    if (reqId) headers["x-networker-request-id"] = reqId;
    send(200, headers, body);
  });
}

// ---------------------------------------------------------------------------
// GET /api/users?page=N&sort=<field>&order=<asc|desc> (§5.4)
// ---------------------------------------------------------------------------
const USER_SORT_FIELDS = ["id", "name", "email", "score", "created_at"];

function handleApiUsers(parsedUrl, send) {
  const startHr = process.hrtime();
  const params = parsedUrl.query || {};
  let page = parseInt(params.page, 10);
  if (!Number.isInteger(page) || page < 1) page = 1;
  const sortField = USER_SORT_FIELDS.includes(params.sort) ? params.sort : "id";
  const desc = params.order === "desc";

  // 100-user window; the dataset has 100 users, so page ≥ 2 is [] (status 200).
  const winStart = (page - 1) * 100;
  const win = BENCH_DATA.users.slice(winStart, winStart + 100);

  // Stable sort (V8 Array#sort is stable); string fields compare bytewise,
  // score as float64. `desc` reverses the comparator — ties keep dataset order.
  const base = sortField === "score" || sortField === "id"
    ? function (a, b) { return a[sortField] - b[sortField]; }
    : function (a, b) { return cmpStr(a[sortField], b[sortField]); };
  win.sort(desc ? function (a, b) { return base(b, a); } : base);

  apiSend(send, 200, JSON.stringify(win.slice(0, 20)), startHr);
}

// ---------------------------------------------------------------------------
// POST /api/transform (§5.5) — SHA-256 each field, reverse values.
// ---------------------------------------------------------------------------
function handleApiTransform(bodyBuf, send) {
  const startHr = process.hrtime();
  var body;
  try {
    body = JSON.parse(bodyBuf.toString());
  } catch (_e) {
    return apiError(send, 400, "invalid JSON", startHr);
  }
  if (typeof body !== "object" || body === null || Array.isArray(body)) {
    return apiError(send, 400, "invalid JSON", startHr);
  }

  const seed = body.seed !== undefined ? body.seed : 0;
  const fields = Array.isArray(body.fields) ? body.fields : [];
  const values = Array.isArray(body.values) ? body.values : [];

  const hashedFields = fields.map(function (f) {
    return crypto.createHash("sha256").update(String(f), "utf8").digest("hex");
  });
  const reversedValues = values.slice().reverse();

  apiSend(send, 200, JSON.stringify({
    seed: seed,
    hashed_fields: hashedFields,
    reversed_values: reversedValues,
  }), startHr);
}

// ---------------------------------------------------------------------------
// GET /api/aggregate[?range=start,end] — range accepted and IGNORED (§5.6).
// Hand-assembled body so integral floats keep their ".0" (see jnum()).
// ---------------------------------------------------------------------------
function computeAggregate() {
  const values = TS_VALUES.slice().sort(function (a, b) { return a - b; });
  const n = values.length;
  let sum = 0.0;
  for (let i = 0; i < n; i++) sum += values[i]; // sequential sum of SORTED values

  const chunk = Math.floor(n / 5);
  const categories = [];
  for (let i = 0; i < 5; i++) {
    let partSum = 0.0;
    for (let j = i * chunk; j < (i + 1) * chunk; j++) partSum += values[j];
    categories.push({
      category: "q" + (i + 1),
      count: chunk,
      mean: r2(partSum / chunk),
      min: r2(values[i * chunk]),
      max: r2(values[(i + 1) * chunk - 1]),
    });
  }

  return {
    total_points: n,
    mean: r2(sum / n),
    p50: r2(values[Math.floor(n * 0.50)]),
    p95: r2(values[Math.floor(n * 0.95)]),
    max: r2(values[n - 1]),
    categories: categories,
  };
}

function serializeAggregate(agg) {
  const cats = agg.categories.map(function (c) {
    return '{"category":"' + c.category + '","count":' + c.count +
      ',"mean":' + jnum(c.mean) + ',"min":' + jnum(c.min) + ',"max":' + jnum(c.max) + "}";
  });
  return '{"total_points":' + agg.total_points +
    ',"mean":' + jnum(agg.mean) +
    ',"p50":' + jnum(agg.p50) +
    ',"p95":' + jnum(agg.p95) +
    ',"max":' + jnum(agg.max) +
    ',"categories":[' + cats.join(",") + "]}";
}

function handleApiAggregate(parsedUrl, send) {
  const startHr = process.hrtime();
  apiSend(send, 200, serializeAggregate(computeAggregate()), startHr);
}

// ---------------------------------------------------------------------------
// GET /api/search?q=<term>&limit=N (§5.7) — case-sensitive regex with
// literal-substring fallback when the pattern fails to compile.
// ---------------------------------------------------------------------------
function computeSearch(query, limit) {
  var re = null;
  try {
    re = new RegExp(query);
  } catch (_e) {
    re = null; // literal fallback
  }

  const matches = [];
  for (let i = 0; i < BENCH_DATA.search_corpus.length; i++) {
    const item = BENCH_DATA.search_corpus[i];
    let pos = -1;
    if (re !== null) {
      const m = re.exec(item);
      if (m) pos = m.index;
    } else {
      pos = item.indexOf(query);
    }
    if (pos >= 0) matches.push({ pos: pos, item: item });
  }

  matches.sort(function (a, b) { return (a.pos - b.pos) || cmpStr(a.item, b.item); });

  const limited = matches.slice(0, limit);
  return {
    query: query,
    total_matches: matches.length,
    returned: limited.length,
    results: limited.map(function (m, i) {
      return { rank: i + 1, item: m.item, match_position: m.pos };
    }),
  };
}

function handleApiSearch(parsedUrl, send) {
  const startHr = process.hrtime();
  const params = parsedUrl.query || {};
  const query = params.q !== undefined && params.q !== "" ? params.q : "test";
  var limit = parseInt(params.limit, 10);
  if (!Number.isInteger(limit)) limit = 20;
  if (limit > 100) limit = 100;
  if (limit < 0) limit = 0;

  apiSend(send, 200, JSON.stringify(computeSearch(query, limit)), startHr);
}

// ---------------------------------------------------------------------------
// POST /api/upload/process (§5.8) — CRC-32 (IEEE) + SHA-256 + zlib level 6.
// ---------------------------------------------------------------------------
function handleApiUploadProcess(bodyBuf, send) {
  const startHr = process.hrtime();

  const crcVal = zlib.crc32(bodyBuf) >>> 0;
  const sha = crypto.createHash("sha256").update(bodyBuf).digest("hex");
  // zlib (RFC 1950) at level 6 — NOT raw deflate, NOT gzip.
  const compressed = zlib.deflateSync(bodyBuf, { level: 6 });

  apiSend(send, 200, JSON.stringify({
    original_size: bodyBuf.length,
    compressed_size: compressed.length,
    crc32: ("00000000" + crcVal.toString(16)).slice(-8),
    sha256: sha,
  }), startHr);
}

// ---------------------------------------------------------------------------
// GET /api/delayed?ms=N&work=<ignored> (§5.9) — setTimeout, clamp [1, 100].
// ---------------------------------------------------------------------------
function handleApiDelayed(parsedUrl, send) {
  const startHr = process.hrtime();
  const params = parsedUrl.query || {};
  var ms = parseInt(params.ms, 10);
  if (!Number.isInteger(ms)) ms = 10;
  if (ms < 1) ms = 1;
  if (ms > 100) ms = 100;
  // `work` is reserved: accepted and ignored.

  setTimeout(function () {
    const elapsed = process.hrtime(startHr);
    const actualMs = Math.round((elapsed[0] * 1000 + elapsed[1] / 1e6) * 100) / 100;
    apiSend(send, 200, JSON.stringify({
      requested_ms: ms,
      actual_ms: actualMs,
    }), startHr);
  }, ms);
}

// ---------------------------------------------------------------------------
// GET /api/validate?seed=N (§5.10) — echo the dataset's expected_checksums.
// ---------------------------------------------------------------------------
function handleApiValidate(parsedUrl, send) {
  const startHr = process.hrtime();
  const params = parsedUrl.query || {};
  var seed = parseInt(params.seed, 10);
  if (!Number.isInteger(seed)) seed = 42;

  apiSend(send, 200, JSON.stringify({
    seed: seed,
    checksums: BENCH_DATA.expected_checksums,
  }), startHr);
}

// ---------------------------------------------------------------------------
// Collect request body helper
// ---------------------------------------------------------------------------
function collectBody(readable, callback) {
  const chunks = [];
  readable.on("data", function (chunk) {
    chunks.push(chunk);
  });
  readable.on("end", function () {
    callback(Buffer.concat(chunks));
  });
}

// ---------------------------------------------------------------------------
// API route dispatcher (shared by H1 and H2)
// ---------------------------------------------------------------------------
function dispatchApi(method, urlPath, parsedUrl, getBody, send) {
  if (method === "GET" && urlPath === "/api/users") {
    handleApiUsers(parsedUrl, send);
    return true;
  }
  if (method === "POST" && urlPath === "/api/transform") {
    getBody(function (buf) { handleApiTransform(buf, send); });
    return true;
  }
  if (method === "GET" && urlPath === "/api/aggregate") {
    handleApiAggregate(parsedUrl, send);
    return true;
  }
  if (method === "GET" && urlPath === "/api/search") {
    handleApiSearch(parsedUrl, send);
    return true;
  }
  if (method === "POST" && urlPath === "/api/upload/process") {
    getBody(function (buf) { handleApiUploadProcess(buf, send); });
    return true;
  }
  if (method === "GET" && urlPath === "/api/delayed") {
    handleApiDelayed(parsedUrl, send);
    return true;
  }
  if (method === "GET" && urlPath === "/api/validate") {
    handleApiValidate(parsedUrl, send);
    return true;
  }
  return false;
}

// ---------------------------------------------------------------------------
// HTTP/2 stream dispatcher
// ---------------------------------------------------------------------------
function onStream(stream, headers) {
  const rawMethod = headers[":method"];
  // HEAD is served like GET with the body suppressed (the validator checks
  // the §1 benchmark headers with `curl -I`); Content-Length still reflects
  // the would-be body, per HTTP semantics.
  const isHead = rawMethod === "HEAD";
  const method = isHead ? "GET" : rawMethod;
  const rawPath = headers[":path"];
  const urlPath = rawPath.split("?")[0];
  const parsedUrl = url.parse(rawPath, true);
  const send = function (status, hdrs, body) {
    if (isHead) {
      stream.respond(Object.assign({ ":status": status }, hdrs));
      stream.end();
      return;
    }
    sendH2(stream, status, hdrs, body);
  };
  // For /download the body is streamed after headers:
  const sendHeadersOnly = function (status, hdrs) {
    stream.respond(Object.assign({ ":status": status }, hdrs));
  };

  if (!checkAuth(urlPath, headers, send)) return;

  if (method === "GET" && urlPath === "/health") {
    return handleHealth(send);
  }

  if (!isHead && method === "GET" && urlPath.startsWith("/download/")) {
    return handleDownload(urlPath, function (status, hdrs, _body) {
      sendHeadersOnly(status, hdrs);
    }, stream);
  }

  if (method === "POST" && urlPath === "/upload") {
    return handleUpload(stream, headers, send);
  }

  const handled = dispatchApi(
    method, urlPath, parsedUrl,
    function (cb) { collectBody(stream, cb); },
    send
  );
  if (handled) return;

  const body = JSON.stringify({ error: "not found" });
  send(404, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
  }, body);
}

// ---------------------------------------------------------------------------
// HTTP/1.1 request dispatcher (fires for allowHTTP1 fallback connections)
// ---------------------------------------------------------------------------
function onRequest(req, res) {
  // The http2 compat layer fires 'request' for h2 streams TOO -- onStream
  // already owns those, and answering here as well threw
  // ERR_HTTP2_HEADERS_SENT and killed the worker on the first h2 request
  // (found by the wave-2 validation run). Only handle real HTTP/1.x
  // (allowHTTP1 fallback) connections here.
  if (req.httpVersion === "2.0") return;
  // HEAD is served like GET with the body suppressed (Node's http layer
  // drops response bodies for HEAD automatically).
  const isHead = req.method === "HEAD";
  const method = isHead ? "GET" : req.method;
  const rawUrl = req.url;
  const urlPath = rawUrl.split("?")[0];
  const parsedUrl = url.parse(rawUrl, true);
  const send = function (status, hdrs, body) { sendH1(res, status, hdrs, body); };
  const sendHeadersOnly = function (status, hdrs) { res.writeHead(status, hdrs); };

  if (!checkAuth(urlPath, req.headers, send)) return;

  if (method === "GET" && urlPath === "/health") {
    return handleHealth(send);
  }

  if (method === "GET" && urlPath.startsWith("/download/")) {
    return handleDownload(urlPath, function (status, hdrs, _body) {
      sendHeadersOnly(status, hdrs);
    }, res);
  }

  if (method === "POST" && urlPath === "/upload") {
    return handleUpload(req, req.headers, send);
  }

  const handled = dispatchApi(
    method, urlPath, parsedUrl,
    function (cb) { collectBody(req, cb); },
    send
  );
  if (handled) return;

  const body = JSON.stringify({ error: "not found" });
  send(404, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
  }, body);
}

// ---------------------------------------------------------------------------
// Server bootstrap — cluster primary forks BENCH_WORKERS workers (§3);
// each worker handles both HTTP/2 (stream event) and HTTP/1.1 (request event).
// ---------------------------------------------------------------------------
function startServer() {
  let server;
  if (USE_TLS) {
    server = http2.createSecureServer(tlsOptions);
    server.on("stream", onStream);
    server.on("request", onRequest);
  } else {
    // Plain HTTP mode (application mode behind reverse proxy)
    server = http.createServer(onRequest);
  }

  server.on("error", (err) => {
    log.error("Server error", { error: err.message });
    process.exit(1);
  });

  server.listen(PORT, () => {
    const proto = USE_TLS ? "https" : "http";
    log.info("nodejs reference-api worker listening", {
      addr: `${proto}://0.0.0.0:${PORT}`, tls: USE_TLS, pid: process.pid,
    });
  });
}

if (require.main === module) {
  if (cluster.isPrimary) {
    log.info("Worker policy", { nproc: NPROC, bench_workers: WORKERS, mechanism: "cluster" });
    for (let i = 0; i < WORKERS; i++) {
      cluster.fork();
    }
    cluster.on("exit", function (worker, code, signal) {
      // Fail loud: a dying worker means the benchmark environment is broken.
      log.error("Worker exited", { pid: worker.process.pid, code: code, signal: signal });
      process.exit(code || 1);
    });
  } else {
    startServer();
  }
}

// Pure logic exported for unit tests (test.js).
module.exports = {
  jnum: jnum,
  r2: r2,
  cmpStr: cmpStr,
  computeAggregate: computeAggregate,
  serializeAggregate: serializeAggregate,
  computeSearch: computeSearch,
  HEALTH_BODY: HEALTH_BODY,
  CHUNK: CHUNK,
  BENCH_DATA: BENCH_DATA,
};
