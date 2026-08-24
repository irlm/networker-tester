import { useApiLogStore } from '../stores/apiLogStore';
import { getRequestSource } from '../lib/requestSource';

const API_BASE = '/api';

/**
 * Typed API error. `status` is the HTTP status code (0 for network-level
 * failures where no response was received). `body` is the raw response body,
 * when one was read. `message` is human-readable copy safe to render
 * directly in banners/toasts — never a raw response body.
 */
export class ApiError extends Error {
  readonly status: number;
  readonly body: string | null;

  constructor(status: number, message: string, body: string | null = null) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.body = body;
  }
}

// ── Human-readable error copy ──────────────────────────────────────────────
//
// SREs watch closest when things break — that's exactly when raw
// `ApiError: API error: 404 Not Found`, nginx 502 HTML dumps, and
// `{"error":"internal server error"}` used to leak into the UI (design
// audit F3/F16). Map status codes to plain copy and never surface a raw
// body verbatim.

const STATUS_COPY: Record<number, string> = {
  400: 'The server rejected this request.',
  401: 'Session expired — sign in again.',
  403: "You don't have permission to do that.",
  404: 'Not found — it may have been deleted.',
  409: 'Conflict — this already exists.',
  422: 'The server rejected the request payload.',
  429: 'Rate limited — wait a moment and try again.',
  500: 'Server error — try again shortly.',
  501: 'Not supported by this server.',
  502: 'Server unavailable — it may be restarting. Try again shortly.',
  503: 'Server unavailable — it may be restarting. Try again shortly.',
  504: 'The server timed out — try again shortly.',
};

/** Build a renderable message from an HTTP failure without leaking raw bodies. */
export function friendlyHttpError(status: number, statusText: string, body: string | null): string {
  const trimmed = (body ?? '').trim();
  let detail = '';
  // HTML (e.g. a raw nginx 502 page) is never useful copy — drop it.
  if (trimmed && !trimmed.startsWith('<')) {
    if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
      try {
        const parsed: unknown = JSON.parse(trimmed);
        if (parsed && typeof parsed === 'object') {
          const o = parsed as Record<string, unknown>;
          const m = o.error ?? o.message ?? o.detail;
          if (typeof m === 'string') detail = m;
        }
      } catch { /* not JSON — ignore */ }
    } else {
      detail = trimmed;
    }
  }
  if (detail.length > 160) detail = `${detail.slice(0, 157)}...`;
  const base = STATUS_COPY[status] ?? `Request failed (${status}${statusText ? ` ${statusText}` : ''}).`;
  // 5xx bodies are stack noise ("internal server error") — status copy only.
  // Exception: 501 is a deliberate, honest "not implemented" whose body
  // explains what is missing and what to do instead — surface it.
  return detail && (status < 500 || status === 501) && detail.toLowerCase() !== 'internal server error'
    ? `${base} (${detail})`
    : base;
}

/**
 * Render-safe message for any caught error. Use instead of `String(e)`,
 * which leaks the class name ("ApiError: ...") into user-facing banners.
 */
export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/**
 * Clear every session-scoped localStorage key. Shared by all REST clients
 * (client.ts, testers.ts, vmHistory.ts) so a 401 wipes the *whole* session,
 * not just the token.
 */
export function clearSession(): void {
  localStorage.removeItem('token');
  localStorage.removeItem('email');
  localStorage.removeItem('role');
  localStorage.removeItem('status');
  localStorage.removeItem('mustChangePassword');
  localStorage.removeItem('isPlatformAdmin');
  localStorage.removeItem('activeProjectId');
  localStorage.removeItem('activeProjectSlug');
  localStorage.removeItem('activeProjectRole');
}

/** Session expired: wipe local state and send the user to /login (once). */
export function handleUnauthorized(): void {
  clearSession();
  // Guard against a redirect loop / pointless full reload when the 401
  // arrives while we're already on the login page.
  if (window.location.pathname !== '/login') {
    window.location.href = '/login';
  }
}

// Anonymous auth endpoints where a 401 means "bad credentials", not "session
// expired" — they must surface the error to the caller instead of wiping the
// session and hard-reloading the login page mid-interaction.
const AUTH_401_EXEMPT = new Set([
  '/auth/login',
  '/auth/sso/exchange',
  '/auth/forgot-password',
  '/auth/reset-password',
]);

// Anonymous endpoints whose path embeds a dynamic token — same exemption as
// above, but matched by prefix. `/invite/{token}/accept` in particular
// returns 401 for bad credentials mid-interaction.
const AUTH_401_EXEMPT_PREFIXES = ['/invite/', '/share/'];

function isAuth401Exempt(path: string): boolean {
  return AUTH_401_EXEMPT.has(path) || AUTH_401_EXEMPT_PREFIXES.some(p => path.startsWith(p));
}

/**
 * Shared low-level REST helper: auth header, 401/403 session handling,
 * ApiError wrapping, empty-body tolerance, and perf-log instrumentation.
 *
 * All REST modules (client.ts, testers.ts, vmHistory.ts) must go through
 * this — a raw `fetch` bypasses the api-log panel and the perf-log flush.
 * The only sanctioned exceptions are streaming SSE readers (useSSE,
 * useDeployEvents) and the perf-log flush itself (usePerfLogFlush).
 */
export async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const token = localStorage.getItem('token');
  const hasFormDataBody = typeof FormData !== 'undefined' && options?.body instanceof FormData;
  const headers: Record<string, string> = {
    ...(!hasFormDataBody ? { 'Content-Type': 'application/json' } : {}),
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  };

  const method = (options?.method || 'GET').toUpperCase();
  const source = getRequestSource();
  const start = performance.now();
  let status = 0;
  let serverMs: number | null = null;
  let bytes: number | null = null;
  let errorMsg: string | null = null;
  // Aborted requests (a poll cancelled by navigation/unmount) never complete,
  // so their wall-clock is meaningless — the timer runs until teardown and logs
  // fake 30–40s entries that pollute the perf-log p95. Don't record them.
  let aborted = false;

  try {
    let res: Response;
    try {
      res = await fetch(`${API_BASE}${path}`, {
        ...options,
        headers: { ...headers, ...(options?.headers as Record<string, string>) },
      });
    } catch (fetchErr) {
      // Preserve AbortError so callers using AbortController can distinguish
      // cancellation from real network failures.
      if (fetchErr instanceof DOMException && fetchErr.name === 'AbortError') {
        aborted = true;
        throw fetchErr;
      }
      throw new ApiError(
        0,
        `Network error: ${fetchErr instanceof Error ? fetchErr.message : String(fetchErr)}`
      );
    }

    status = res.status;

    // Extract server processing time from response header
    const serverTime = res.headers.get('x-process-time-ms');
    if (serverTime) serverMs = parseFloat(serverTime);

    // Response size, so a row's network time can be READ rather than guessed
    // at: 40 ms for 2 kB is latency-bound, 40 ms for 500 kB is throughput-bound.
    // Content-Length is the cheap answer and, for a compressed response, is
    // already the wire size. It is absent on chunked responses — those fall
    // back to resource timing below, after the body has been consumed.
    const contentLength = res.headers.get('content-length');
    if (contentLength) {
      const parsed = Number(contentLength);
      if (Number.isFinite(parsed) && parsed >= 0) bytes = parsed;
    }

    if (res.status === 401 && !isAuth401Exempt(path)) {
      handleUnauthorized();
      throw new ApiError(401, 'Unauthorized');
    }

    if (res.status === 403) {
      const body = await res.text();
      if (body === 'pending_approval') {
        localStorage.setItem('status', 'pending');
        if (window.location.pathname !== '/pending') {
          window.location.href = '/pending';
        }
        throw new ApiError(403, 'pending_approval', body);
      }
      throw new ApiError(403, friendlyHttpError(403, res.statusText, body), body);
    }

    if (!res.ok) {
      const body = await res.text();
      throw new ApiError(
        res.status,
        friendlyHttpError(res.status, res.statusText, body),
        body || null
      );
    }

    // 204 / empty-body responses (e.g. DELETE → NoContent) are valid for
    // request<void> endpoints — res.json() would throw on them.
    const text = await res.text();
    return (text ? JSON.parse(text) : undefined) as T;
  } catch (err) {
    errorMsg = err instanceof Error ? err.message : String(err);
    throw err;
  } finally {
    const totalMs = performance.now() - start;
    const store = useApiLogStore.getState();
    if (store.enabled && !aborted) {
      store.add({
        timestamp: Date.now(),
        method,
        path,
        status,
        totalMs,
        serverMs,
        networkMs: serverMs !== null ? totalMs - serverMs : null,
        ...(() => {
          // One Resource Timing lookup fills both: Content-Length covers size
          // when the server sent it, but transfer time has no header equivalent.
          const timing = resourceTiming(`${API_BASE}${path}`, start);
          return {
            bytes: bytes ?? timing?.bytes ?? null,
            transferMs: timing?.transferMs ?? null,
          };
        })(),
        error: errorMsg,
        source,
      });
    }
  }
}
/**
 * Size and transfer time for the request that started at `startedAt`, read from
 * the Resource Timing buffer.
 *
 * `transferMs` is `responseEnd - responseStart`: the time the body was actually
 * arriving. It is the honest answer to "was the network time high because the
 * response was big?", and it is NOT the same as bytes ÷ network time — the
 * network leg is dominated by round-trip latency, so that division produces a
 * number in bandwidth units that is not bandwidth (measured on this API: 234 B
 * responses spend ~0.09 ms transferring out of a ~40 ms leg).
 *
 * `transferSize` includes response headers and is the COMPRESSED size, which is
 * what transfer time actually depends on.
 *
 * Matched by start time, not just URL: the same endpoint is polled repeatedly,
 * and taking the first match would keep reporting the oldest call. Returns null
 * rather than a guess when the entry is missing (the buffer is capped and
 * entries can be evicted) — a wrong number here is worse than none, because it
 * would be used to rule payload size in or out.
 */
function resourceTiming(
  url: string,
  startedAt: number,
): { bytes: number | null; transferMs: number | null } | null {
  try {
    const absolute = new URL(url, window.location.origin).href;
    const entries = performance.getEntriesByType('resource') as PerformanceResourceTiming[];
    let best: PerformanceResourceTiming | null = null;
    for (const e of entries) {
      if (e.name !== absolute || e.startTime < startedAt - 1) continue;
      if (!best || e.startTime < best.startTime) best = e;
    }
    if (!best) return null;

    // transferSize is 0 for a cache hit — a true answer, keep it. It is also 0
    // when the browser withholds timing; encodedBodySize covers that case.
    const bytes = best.transferSize || best.encodedBodySize || null;
    // Both are 0 when timing is withheld — same-origin here, so normally set.
    const transferMs =
      best.responseEnd > 0 && best.responseStart > 0
        ? Math.max(0, best.responseEnd - best.responseStart)
        : null;
    return { bytes, transferMs };
  } catch {
    return null;
  }
}

/** Authenticated binary download using the same session/error policy as JSON requests. */
export async function downloadExport(path: string, fallbackName: string): Promise<void> {
  const token = localStorage.getItem('token');
  const res = await fetch(`${API_BASE}${path}`, {
    headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}) },
  });
  if (res.status === 401) {
    handleUnauthorized();
    throw new ApiError(401, 'Session expired', null);
  }
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new ApiError(res.status, body || `Export failed: ${res.status} ${res.statusText}`, body || null);
  }

  // attachment; filename="test-run-a1b2c3d4.pdf"
  const disposition = res.headers.get('Content-Disposition') ?? '';
  const match = /filename\*?=(?:UTF-8'')?"?([^";]+)"?/i.exec(disposition);
  const filename = match?.[1] ?? fallbackName;

  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  try {
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
  } finally {
    URL.revokeObjectURL(url);
  }
}
