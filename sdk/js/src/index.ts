/**
 * @laghound/endpoint — LagHound diagnostic endpoint for Node (contract v1).
 *
 * Zero runtime dependencies. Implements docs/sdk/contract-v1.md, pinned to
 * shared/sdk-contract-v1.json (the conformance suite in ../test loads that
 * file directly).
 *
 * One factory, three mounting styles:
 *
 *   const handler = laghound({ token: process.env.LAGHOUND_TOKEN });
 *   app.use(handler);                    // Express / Connect middleware
 *   fastify.register(handler);          // Fastify plugin
 *   http.createServer(handler);         // bare node:http handler
 */

import { createHash, timingSafeEqual } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";

// ---------------------------------------------------------------------------
// Contract constants (shared/sdk-contract-v1.json)
// ---------------------------------------------------------------------------

export const CONTRACT = "v1";
export const SDK_LANG = "js";
/** Keep in sync with package.json "version". */
export const SDK_VERSION = "0.1.0";

const ABSOLUTE_MAX_BYTES = 33554432; // 32 MiB — config cannot exceed
const DEFAULT_CAP_BYTES = 4194304; // 4 MiB
const ECHO_BODY_MAX_BYTES = 65536; // /echo request-body cap
const CHUNK_BYTES = 65536; // download stream chunk
const FILL_BYTE = 0x42; // 'B' — matches networker-endpoint DOWNLOAD_FILL
const TOKEN_MIN_BYTES = 16;
const ROTATION_MAX_TOKENS = 2;
const PER_IP_TABLE_MAX_ENTRIES = 10000;
const KILL_SWITCH_ENV = "LAGHOUND_DISABLED";
const KILL_SWITCH_CACHE_MS = 1000;
const TOKEN_ENV = "LAGHOUND_TOKEN";
const CACHE_CONTROL = "no-store, no-cache, must-revalidate";
const BYTES_HEADER = "X-LagHound-Bytes";
const MAX_MARKS = 5; // 8 metrics max − app, total, recv
const MAX_TIMING_HEADER_BYTES = 512;

/** Single per-process read-only fill buffer — never allocate per request. */
const FILL = Buffer.alloc(CHUNK_BYTES, FILL_BYTE);
/** Byte-constant /echo body for the lifetime of the process. */
const ECHO_BODY = Buffer.from(JSON.stringify({ contract: "v1", ok: true }));

const ERROR_MESSAGES: Record<string, string> = {
  invalid_param: "invalid parameter",
  method_not_allowed: "method not allowed",
  payload_too_large: "payload too large",
  rate_limited: "rate limit exceeded",
  internal: "internal error",
};

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

export interface RateSpec {
  rps: number;
  burst: number;
}

export interface ByteBudgetSpec {
  bytes: number;
  windowS: number;
}

export interface RouteToggles {
  echo?: boolean;
  download?: boolean;
  upload?: boolean;
  info?: boolean;
}

export interface LagHoundOptions {
  /**
   * Shared secret (min 16 bytes) or [current, previous] for zero-downtime
   * rotation. Falls back to LAGHOUND_TOKEN. Fail-closed: without a token the
   * factory throws instead of mounting open routes.
   */
  token?: string | string[];
  /** Route prefix. Must start with "/", no trailing slash. Default "/laghound". */
  prefix?: string;
  /** /download cap. Clamped to the 32 MiB absolute max. Default 4 MiB. */
  downloadCapBytes?: number;
  /** /upload cap. Clamped to the 32 MiB absolute max. Default 4 MiB. */
  uploadCapBytes?: number;
  /** Per-IP token bucket. Default { rps: 10, burst: 20 }. */
  ratePerIp?: RateSpec;
  /** Global token bucket. Default { rps: 50, burst: 100 }. */
  rateGlobal?: RateSpec;
  /** Max in-flight LagHound requests. Default 8. */
  maxConcurrent?: number;
  /** Max in-flight /download + /upload transfers. Default 2. */
  maxConcurrentTransfers?: number;
  /** Optional sliding-window transfer-byte budget. Default off. */
  byteBudget?: ByteBudgetSpec | null;
  /** Optional label echoed on /health and /info. Never auto-derived. */
  appName?: string;
  /** Disable individual routes (reflected in the /health capability map). */
  routes?: RouteToggles;
  /**
   * Socket peer addresses allowed to set X-Forwarded-For. Empty (default):
   * the header is ignored and the socket peer IP is used for rate limiting.
   */
  trustedProxies?: string[];
}

interface Resolved {
  tokenHashes: Buffer[];
  prefix: string;
  downloadCapBytes: number;
  uploadCapBytes: number;
  ratePerIp: RateSpec;
  rateGlobal: RateSpec;
  maxConcurrent: number;
  maxConcurrentTransfers: number;
  byteBudget: ByteBudgetSpec | null;
  appName: string | null;
  routes: { health: true; echo: boolean; download: boolean; upload: boolean; info: boolean };
  trustedProxies: string[];
}

function sha256(s: string): Buffer {
  return createHash("sha256").update(s, "utf8").digest();
}

function clampCap(v: number | undefined, name: string): number {
  if (v === undefined) return DEFAULT_CAP_BYTES;
  if (!Number.isSafeInteger(v) || v < 0) throw new Error(`laghound: ${name} must be a non-negative integer`);
  return Math.min(v, ABSOLUTE_MAX_BYTES);
}

function resolveOptions(opts: LagHoundOptions): Resolved {
  const rawTokens =
    opts.token !== undefined
      ? Array.isArray(opts.token)
        ? opts.token
        : [opts.token]
      : process.env[TOKEN_ENV] !== undefined && process.env[TOKEN_ENV] !== ""
        ? [process.env[TOKEN_ENV] as string]
        : [];
  if (rawTokens.length === 0) {
    throw new Error(`laghound: refusing to mount without a token (set options.token or ${TOKEN_ENV})`);
  }
  if (rawTokens.length > ROTATION_MAX_TOKENS) {
    throw new Error(`laghound: at most ${ROTATION_MAX_TOKENS} tokens (current + previous)`);
  }
  for (const t of rawTokens) {
    if (typeof t !== "string" || Buffer.byteLength(t, "utf8") < TOKEN_MIN_BYTES) {
      throw new Error(`laghound: token must be at least ${TOKEN_MIN_BYTES} bytes`);
    }
  }
  const prefix = opts.prefix ?? "/laghound";
  if (!prefix.startsWith("/") || (prefix.length > 1 && prefix.endsWith("/"))) {
    throw new Error('laghound: prefix must start with "/" and have no trailing slash');
  }
  const budget = opts.byteBudget ?? null;
  if (budget !== null) {
    if (!Number.isSafeInteger(budget.bytes) || budget.bytes <= 0 || !Number.isFinite(budget.windowS) || budget.windowS <= 0) {
      throw new Error("laghound: byteBudget requires positive bytes and windowS");
    }
  }
  return {
    tokenHashes: rawTokens.map(sha256),
    prefix,
    downloadCapBytes: clampCap(opts.downloadCapBytes, "downloadCapBytes"),
    uploadCapBytes: clampCap(opts.uploadCapBytes, "uploadCapBytes"),
    ratePerIp: opts.ratePerIp ?? { rps: 10, burst: 20 },
    rateGlobal: opts.rateGlobal ?? { rps: 50, burst: 100 },
    maxConcurrent: opts.maxConcurrent ?? 8,
    maxConcurrentTransfers: opts.maxConcurrentTransfers ?? 2,
    byteBudget: budget,
    appName: opts.appName ?? null,
    routes: {
      health: true,
      echo: opts.routes?.echo ?? true,
      download: opts.routes?.download ?? true,
      upload: opts.routes?.upload ?? true,
      info: opts.routes?.info ?? true,
    },
    trustedProxies: opts.trustedProxies ?? [],
  };
}

// ---------------------------------------------------------------------------
// Limiters
// ---------------------------------------------------------------------------

class TokenBucket {
  rps: number;
  burst: number;
  tokens: number;
  last: number;

  constructor(rps: number, burst: number) {
    this.rps = rps;
    this.burst = burst;
    this.tokens = burst;
    this.last = Date.now();
  }

  take(): boolean {
    const now = Date.now();
    this.tokens = Math.min(this.burst, this.tokens + ((now - this.last) / 1000) * this.rps);
    this.last = now;
    if (this.tokens >= 1) {
      this.tokens -= 1;
      return true;
    }
    return false;
  }
}

/** Per-IP token buckets, LRU-capped so address spraying cannot grow memory. */
class IpRateTable {
  spec: RateSpec;
  maxEntries: number;
  buckets: Map<string, TokenBucket> = new Map();

  constructor(spec: RateSpec, maxEntries: number = PER_IP_TABLE_MAX_ENTRIES) {
    this.spec = spec;
    this.maxEntries = maxEntries;
  }

  take(ip: string): boolean {
    let bucket = this.buckets.get(ip);
    if (bucket !== undefined) {
      this.buckets.delete(ip); // LRU touch
    } else {
      bucket = new TokenBucket(this.spec.rps, this.spec.burst);
      if (this.buckets.size >= this.maxEntries) {
        const oldest = this.buckets.keys().next();
        if (!oldest.done) this.buckets.delete(oldest.value);
      }
    }
    this.buckets.set(ip, bucket);
    return bucket.take();
  }
}

/** Sliding-window transfer-byte budget (§6.4). */
class ByteBudgetTracker {
  bytes: number;
  windowMs: number;
  entries: { t: number; b: number }[] = [];
  used = 0;

  constructor(spec: ByteBudgetSpec) {
    this.bytes = spec.bytes;
    this.windowMs = spec.windowS * 1000;
  }

  private prune(now: number): void {
    while (this.entries.length > 0 && this.entries[0].t <= now - this.windowMs) {
      this.used -= (this.entries.shift() as { t: number; b: number }).b;
    }
  }

  private retryAfterS(now: number): number {
    const head = this.entries[0];
    const ms = head !== undefined ? head.t + this.windowMs - now : this.windowMs;
    return Math.max(1, Math.ceil(ms / 1000));
  }

  /** Reserve n bytes; on failure reports the window remainder in seconds. */
  tryTake(n: number): { ok: true } | { ok: false; retryAfterS: number } {
    const now = Date.now();
    this.prune(now);
    if (this.used + n > this.bytes) return { ok: false, retryAfterS: this.retryAfterS(now) };
    this.charge(n, now);
    return { ok: true };
  }

  /** Exhaustion check without reserving (unknown-length uploads). */
  exhausted(): { ok: true } | { ok: false; retryAfterS: number } {
    const now = Date.now();
    this.prune(now);
    if (this.used >= this.bytes) return { ok: false, retryAfterS: this.retryAfterS(now) };
    return { ok: true };
  }

  charge(n: number, now: number = Date.now()): void {
    if (n > 0) {
      this.entries.push({ t: now, b: n });
      this.used += n;
    }
  }
}

/** @internal exported for the test suite only — not public API. */
export const __internals = { TokenBucket, IpRateTable, ByteBudgetTracker };

// ---------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------

function msSince(t0: bigint): number {
  const ms = Number(process.hrtime.bigint() - t0) / 1e6;
  return ms < 0 ? 0 : ms;
}

/** dur formatting: non-negative decimal, ≤ 3 decimal places. */
function fmtDur(ms: number): string {
  const v = Math.round(Math.max(0, ms) * 1000) / 1000;
  return v.toString();
}

class Core {
  cfg: Resolved;
  startedAt: number = Date.now();
  inflight = 0;
  transfers = 0;
  globalBucket: TokenBucket;
  ipTable: IpRateTable;
  budget: ByteBudgetTracker | null;
  marks: Map<string, number> = new Map();
  ksValue = false;
  ksReadAt = 0;
  healthPre: string;
  healthPost: string;

  constructor(cfg: Resolved) {
    this.cfg = cfg;
    this.globalBucket = new TokenBucket(cfg.rateGlobal.rps, cfg.rateGlobal.burst);
    this.ipTable = new IpRateTable(cfg.ratePerIp);
    this.budget = cfg.byteBudget !== null ? new ByteBudgetTracker(cfg.byteBudget) : null;
    // /health is O(1): the body is precomputed at init except uptime_s.
    const app = cfg.appName !== null ? `"app":${JSON.stringify(cfg.appName)},` : "";
    this.healthPre =
      `{"contract":"v1","status":"ok","sdk":{"lang":"${SDK_LANG}","version":"${SDK_VERSION}"},` + app + `"uptime_s":`;
    this.healthPost = `,"routes":${JSON.stringify(cfg.routes)}}`;
  }

  // -- gates ----------------------------------------------------------------

  killSwitchOn(): boolean {
    const now = Date.now();
    if (now - this.ksReadAt > KILL_SWITCH_CACHE_MS || this.ksReadAt === 0) {
      this.ksValue = process.env[KILL_SWITCH_ENV] === "1";
      this.ksReadAt = now;
    }
    return this.ksValue;
  }

  clientIp(req: IncomingMessage): string {
    const peer = req.socket?.remoteAddress ?? "unknown";
    if (this.cfg.trustedProxies.length > 0 && this.cfg.trustedProxies.includes(peer)) {
      const xff = req.headers["x-forwarded-for"];
      const raw = Array.isArray(xff) ? xff[xff.length - 1] : xff;
      if (raw !== undefined && raw !== "") {
        const hops = raw.split(",");
        const last = hops[hops.length - 1].trim();
        if (last !== "") return last;
      }
    }
    return peer;
  }

  /** Constant-time token check over hashed representations (§5). */
  authed(req: IncomingMessage): boolean {
    const xt = req.headers["x-laghound-token"];
    let candidate: string | undefined;
    if (xt !== undefined) {
      // X-LagHound-Token wins; Authorization is ignored (not compared).
      candidate = Array.isArray(xt) ? xt[0] : xt;
    } else {
      const authz = req.headers.authorization;
      if (typeof authz === "string" && authz.startsWith("Bearer ")) candidate = authz.slice(7);
    }
    if (candidate === undefined) return false;
    const h = sha256(candidate);
    let ok = false;
    for (const stored of this.cfg.tokenHashes) {
      // No early exit; ≤ 2 hashes, each compared in constant time.
      if (timingSafeEqual(h, stored)) ok = true;
    }
    return ok;
  }

  mark(name: string, durMs: number): void {
    if (!/^[a-z0-9]{1,24}$/.test(name)) throw new Error("laghound: mark name must match [a-z0-9]{1,24}");
    if (!Number.isFinite(durMs) || durMs < 0) throw new Error("laghound: mark duration must be a non-negative number");
    if (!this.marks.has(name) && this.marks.size >= MAX_MARKS) {
      const oldest = this.marks.keys().next();
      if (!oldest.done) this.marks.delete(oldest.value);
    }
    // Refresh insertion order so the newest marks survive eviction.
    this.marks.delete(name);
    this.marks.set(name, durMs);
  }

  // -- responses ------------------------------------------------------------

  bare404(req: IncomingMessage, res: ServerResponse): void {
    // No envelope, no Server-Timing, no LagHound headers — indistinguishable
    // from a route that does not exist.
    req.resume();
    res.statusCode = 404;
    res.end();
  }

  timingHeader(appMs: number, recvMs?: number, withMarks?: boolean): string {
    const parts: string[] = [];
    if (recvMs !== undefined) parts.push(`recv;dur=${fmtDur(recvMs)}`);
    parts.push(`app;dur=${fmtDur(appMs)}`);
    parts.push(`total;dur=${fmtDur(recvMs !== undefined ? recvMs + appMs : appMs)}`);
    if (withMarks === true) {
      for (const [name, dur] of this.marks) {
        const metric = `mark-${name};dur=${fmtDur(dur)}`;
        if (parts.join(", ").length + 2 + metric.length > MAX_TIMING_HEADER_BYTES) break;
        parts.push(metric);
      }
    }
    return parts.join(", ");
  }

  commonHeaders(appMs: number, recvMs?: number, withMarks?: boolean): Record<string, string> {
    return {
      "Cache-Control": CACHE_CONTROL,
      "Timing-Allow-Origin": "*",
      "Server-Timing": this.timingHeader(appMs, recvMs, withMarks),
    };
  }

  json(res: ServerResponse, status: number, body: string | Buffer, headers: Record<string, string>): void {
    if (res.headersSent) {
      res.destroy();
      return;
    }
    res.writeHead(status, {
      "Content-Type": "application/json",
      "Content-Length": String(Buffer.byteLength(body)),
      ...headers,
    });
    res.end(body);
  }

  envelope(res: ServerResponse, status: number, code: string, t0: bigint, retryAfterS?: number, close?: boolean): void {
    // Fixed strings only — never interpolate request data (§6.6).
    const retry = retryAfterS !== undefined ? `,"retry_after_ms":${retryAfterS * 1000}` : "";
    const body = `{"contract":"v1","error":{"code":${JSON.stringify(code)},"message":${JSON.stringify(
      ERROR_MESSAGES[code],
    )}${retry}}}`;
    const headers: Record<string, string> = this.commonHeaders(msSince(t0));
    if (retryAfterS !== undefined) headers["Retry-After"] = String(retryAfterS);
    if (close === true) headers.Connection = "close";
    this.json(res, status, body, headers);
  }

  internal(res: ServerResponse, t0: bigint): void {
    try {
      this.envelope(res, 500, "internal", t0);
    } catch {
      try {
        res.destroy();
      } catch {
        /* confined to the LagHound route — never crash the host (§6.7) */
      }
    }
  }

  // -- entry ----------------------------------------------------------------

  matches(url: string | undefined): boolean {
    if (url === undefined) return false;
    const q = url.indexOf("?");
    const path = q === -1 ? url : url.slice(0, q);
    return path === this.cfg.prefix || path.startsWith(this.cfg.prefix + "/");
  }

  /** Returns true when the request was under the prefix (and thus handled). */
  handle(req: IncomingMessage, res: ServerResponse): boolean {
    const url = req.url ?? "";
    const q = url.indexOf("?");
    const path = q === -1 ? url : url.slice(0, q);
    if (path !== this.cfg.prefix && !path.startsWith(this.cfg.prefix + "/")) return false;
    const t0 = process.hrtime.bigint();
    try {
      this.dispatch(req, res, path, q === -1 ? "" : url.slice(q + 1), t0);
    } catch {
      this.internal(res, t0);
    }
    return true;
  }

  /** Order of checks (§5): kill switch → rate/concurrency limits → auth → route. */
  dispatch(req: IncomingMessage, res: ServerResponse, path: string, query: string, t0: bigint): void {
    if (this.killSwitchOn()) {
      this.bare404(req, res);
      return;
    }

    // Limits run before auth so brute-forcing is throttled like everything
    // else; unauthenticated limiter rejections stay bare 404s (§6.2).
    const overConcurrency = this.inflight >= this.cfg.maxConcurrent;
    const overRate = !overConcurrency && (!this.globalBucket.take() || !this.ipTable.take(this.clientIp(req)));
    if (overConcurrency || overRate) {
      if (this.authed(req)) this.envelope(res, 429, "rate_limited", t0, 1);
      else this.bare404(req, res);
      return;
    }

    this.inflight += 1;
    res.once("close", () => {
      this.inflight -= 1;
    });

    if (!this.authed(req)) {
      this.bare404(req, res);
      return;
    }

    const sub = path === this.cfg.prefix ? "" : path.slice(this.cfg.prefix.length + 1);
    switch (sub) {
      case "health":
        this.routeHealth(req, res, t0);
        return;
      case "echo":
        if (!this.cfg.routes.echo) break;
        this.routeEcho(req, res, t0);
        return;
      case "download":
        if (!this.cfg.routes.download) break;
        this.routeDownload(req, res, query, t0);
        return;
      case "upload":
        if (!this.cfg.routes.upload) break;
        this.routeUpload(req, res, t0);
        return;
      case "info":
        if (!this.cfg.routes.info) break;
        this.routeInfo(req, res, t0);
        return;
      default:
        break;
    }
    // Unknown or disabled subpath under the prefix → bare 404 (§7).
    this.bare404(req, res);
  }

  // -- routes ---------------------------------------------------------------

  routeHealth(req: IncomingMessage, res: ServerResponse, t0: bigint): void {
    if (req.method !== "GET") {
      this.envelope(res, 405, "method_not_allowed", t0);
      return;
    }
    req.resume();
    const uptime = Math.floor((Date.now() - this.startedAt) / 1000);
    this.json(res, 200, this.healthPre + uptime + this.healthPost, this.commonHeaders(msSince(t0)));
  }

  routeEcho(req: IncomingMessage, res: ServerResponse, t0: bigint): void {
    if (req.method !== "GET") {
      this.envelope(res, 405, "method_not_allowed", t0);
      return;
    }
    const cl = req.headers["content-length"];
    if (cl !== undefined && Number(cl) > ECHO_BODY_MAX_BYTES) {
      this.envelope(res, 413, "payload_too_large", t0, undefined, true);
      res.once("finish", () => req.destroy());
      return;
    }
    req.resume(); // discard any small body — never buffered, never echoed
    this.json(res, 200, ECHO_BODY, this.commonHeaders(msSince(t0), undefined, true));
  }

  routeDownload(req: IncomingMessage, res: ServerResponse, query: string, t0: bigint): void {
    if (req.method !== "GET") {
      this.envelope(res, 405, "method_not_allowed", t0);
      return;
    }
    req.resume();
    const raw = new URLSearchParams(query).get("bytes");
    let requested = DEFAULT_CAP_BYTES;
    if (raw !== null) {
      // Present but unparsable/negative → 400 (a silent default would make
      // measurements lie). Never echo the offending value (§6.6).
      if (!/^\d+$/.test(raw)) {
        this.envelope(res, 400, "invalid_param", t0);
        return;
      }
      requested = Number(raw);
    }
    // Over-cap is clamped, not rejected; actual size is reported (§3.3).
    const effective = Math.min(requested, this.cfg.downloadCapBytes, ABSOLUTE_MAX_BYTES);

    if (!this.takeTransferSlot(res)) {
      this.envelope(res, 429, "rate_limited", t0, 1);
      return;
    }
    if (this.budget !== null) {
      const b = this.budget.tryTake(effective);
      if (!b.ok) {
        this.envelope(res, 429, "rate_limited", t0, b.retryAfterS);
        return;
      }
    }

    // app = setup time only, measured before the first chunk is written.
    const appMs = msSince(t0);
    res.writeHead(200, {
      "Content-Type": "application/octet-stream",
      "Content-Length": String(effective),
      [BYTES_HEADER]: String(effective),
      ...this.commonHeaders(appMs),
    });

    // Stream ≤ 64 KiB chunks from the single shared buffer — zero per-request
    // allocation proportional to N.
    let remaining = effective;
    const writeChunks = (): void => {
      while (remaining > 0) {
        const n = remaining >= CHUNK_BYTES ? CHUNK_BYTES : remaining;
        const chunk = n === CHUNK_BYTES ? FILL : FILL.subarray(0, n);
        remaining -= n;
        if (!res.write(chunk)) {
          res.once("drain", writeChunks);
          return;
        }
      }
      res.end();
    };
    writeChunks();
  }

  routeUpload(req: IncomingMessage, res: ServerResponse, t0: bigint): void {
    if (req.method !== "POST") {
      this.envelope(res, 405, "method_not_allowed", t0);
      return;
    }
    const cap = this.cfg.uploadCapBytes;

    // Declared over cap → immediate 413 without reading the body (§3.4).
    const cl = req.headers["content-length"];
    const declared = cl !== undefined ? Number(cl) : undefined;
    if (declared !== undefined && declared > cap) {
      this.envelope(res, 413, "payload_too_large", t0, undefined, true);
      res.once("finish", () => req.destroy());
      return;
    }

    if (!this.takeTransferSlot(res)) {
      this.envelope(res, 429, "rate_limited", t0, 1);
      return;
    }
    if (this.budget !== null) {
      const b = declared !== undefined ? this.budget.tryTake(declared) : this.budget.exhausted();
      if (!b.ok) {
        this.envelope(res, 429, "rate_limited", t0, b.retryAfterS);
        return;
      }
    }

    // Drain and count, never buffer — peak memory is O(chunk) (§3.4).
    const tRecv = process.hrtime.bigint();
    let received = 0;
    let settled = false;
    req.on("data", (chunk: Buffer) => {
      if (settled) return;
      received += chunk.length;
      if (received > cap) {
        // Chunked/unknown length over cap: stop reading, 413, close (§3.4).
        settled = true;
        req.pause();
        this.envelope(res, 413, "payload_too_large", t0, undefined, true);
        res.once("finish", () => req.destroy());
      }
    });
    req.on("end", () => {
      if (settled) return;
      settled = true;
      const recvMs = msSince(tRecv);
      if (this.budget !== null && declared === undefined) this.budget.charge(received);
      const tApp = process.hrtime.bigint();
      const body = `{"contract":"v1","received_bytes":${received}}`;
      this.json(res, 200, body, {
        [BYTES_HEADER]: String(received),
        ...this.commonHeaders(msSince(tApp), recvMs),
      });
    });
    req.on("error", () => {
      settled = true;
    });
  }

  routeInfo(req: IncomingMessage, res: ServerResponse, t0: bigint): void {
    if (req.method !== "GET") {
      this.envelope(res, 405, "method_not_allowed", t0);
      return;
    }
    req.resume();
    // Only the SDK's own config keys — never the token or any derivative,
    // no hostnames, IPs, or env dumps (§3.5).
    const info = {
      contract: CONTRACT,
      sdk: { lang: SDK_LANG, version: SDK_VERSION },
      ...(this.cfg.appName !== null ? { app: this.cfg.appName } : {}),
      prefix: this.cfg.prefix,
      uptime_s: Math.floor((Date.now() - this.startedAt) / 1000),
      token_set: true,
      caps: {
        download_bytes: this.cfg.downloadCapBytes,
        upload_bytes: this.cfg.uploadCapBytes,
        absolute_max_bytes: ABSOLUTE_MAX_BYTES,
      },
      limits: {
        rate_per_ip: this.cfg.ratePerIp,
        rate_global: this.cfg.rateGlobal,
        max_concurrent: this.cfg.maxConcurrent,
        max_concurrent_transfers: this.cfg.maxConcurrentTransfers,
        byte_budget:
          this.cfg.byteBudget !== null
            ? { bytes: this.cfg.byteBudget.bytes, window_s: this.cfg.byteBudget.windowS }
            : null,
      },
      routes: this.cfg.routes,
    };
    this.json(res, 200, JSON.stringify(info), this.commonHeaders(msSince(t0)));
  }

  takeTransferSlot(res: ServerResponse): boolean {
    if (this.transfers >= this.cfg.maxConcurrentTransfers) return false;
    this.transfers += 1;
    res.once("close", () => {
      this.transfers -= 1;
    });
    return true;
  }
}

// ---------------------------------------------------------------------------
// Public factory — Express/Connect middleware + Fastify plugin + bare handler
// ---------------------------------------------------------------------------

export interface LagHoundHandler {
  /** Bare node:http handler — unmatched paths get a plain 404. */
  (req: IncomingMessage, res: ServerResponse): void;
  /** Express/Connect middleware — unmatched paths fall through to next(). */
  (req: IncomingMessage, res: ServerResponse, next: (err?: unknown) => void): void;
  /** Fastify plugin — register with fastify.register(handler). */
  (instance: unknown, opts: unknown, done: (err?: Error) => void): void;
  /** Record a custom Server-Timing mark (emitted as mark-<name> on /echo). */
  mark(name: string, durMs: number): void;
}

function isFastifyInstance(x: unknown): boolean {
  return (
    typeof x === "object" &&
    x !== null &&
    typeof (x as { addHook?: unknown }).addHook === "function" &&
    typeof (x as { register?: unknown }).register === "function"
  );
}

/**
 * Create a LagHound endpoint handler (contract v1).
 *
 * Throws at init when no token is available (fail-closed — never mounts open
 * routes), when a token is shorter than 16 bytes, or on invalid config.
 */
export function laghound(options: LagHoundOptions = {}): LagHoundHandler {
  const core = new Core(resolveOptions(options));

  const fn = function (a: unknown, b: unknown, c: unknown): void {
    if (isFastifyInstance(a)) {
      // Fastify plugin: intercept at onRequest, before routing, via raw req/res.
      const instance = a as {
        addHook: (name: string, hook: (request: unknown, reply: unknown, done: () => void) => void) => void;
      };
      instance.addHook("onRequest", (request: unknown, reply: unknown, hookDone: () => void) => {
        const rawReq = (request as { raw: IncomingMessage }).raw;
        if (!core.matches(rawReq.url)) {
          hookDone();
          return;
        }
        const r = reply as { hijack?: () => void; raw: ServerResponse };
        if (typeof r.hijack === "function") r.hijack();
        core.handle(rawReq, r.raw);
      });
      if (typeof c === "function") (c as () => void)();
      else if (typeof b === "function") (b as () => void)();
      return;
    }

    const req = a as IncomingMessage;
    const res = b as ServerResponse;
    const next = typeof c === "function" ? (c as (err?: unknown) => void) : undefined;
    const handled = core.handle(req, res);
    if (!handled) {
      if (next !== undefined) {
        next(); // Express/Connect: not ours, fall through
      } else {
        res.statusCode = 404; // bare node:http: nothing else will answer
        res.end();
      }
    }
  } as LagHoundHandler;

  fn.mark = (name: string, durMs: number) => core.mark(name, durMs);
  // Fastify: keep the onRequest hook in the parent scope when registered.
  (fn as unknown as Record<symbol, boolean>)[Symbol.for("skip-override")] = true;
  return fn;
}

export default laghound;
