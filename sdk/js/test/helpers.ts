// Shared test helpers — run natively by Node's test runner (type stripping).
import { createServer, request as httpRequest, type Server, type IncomingMessage } from "node:http";
import { readFileSync } from "node:fs";
import { laghound, type LagHoundOptions, type LagHoundHandler } from "../src/index.ts";

/** The machine-readable contract every SDK pins its conformance tests to. */
export const contract = JSON.parse(
  readFileSync(new URL("../../../shared/sdk-contract-v1.json", import.meta.url), "utf8"),
) as Record<string, any>;

export const TOKEN = "conformance-test-token-0123456789";

export interface TestServer {
  url: string;
  server: Server;
  handler: LagHoundHandler;
  close(): Promise<void>;
}

export async function startServer(opts: LagHoundOptions = {}): Promise<TestServer> {
  const handler = laghound({ token: TOKEN, ...opts });
  const server = createServer(handler);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const addr = server.address();
  const port = typeof addr === "object" && addr !== null ? addr.port : 0;
  return {
    url: `http://127.0.0.1:${port}`,
    server,
    handler,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

export function auth(extra: Record<string, string> = {}): Record<string, string> {
  return { [contract.auth.header as string]: TOKEN, ...extra };
}

export interface RawResponse {
  status: number;
  headers: IncomingMessage["headers"];
  body: Buffer;
}

/**
 * Raw client for cases fetch can't express (send headers without body, hold a
 * chunked body open, read the response before the request is finished).
 */
export function rawRequest(
  url: string,
  options: {
    method?: string;
    path: string;
    headers?: Record<string, string | number>;
    /** Written immediately; the request is NOT ended unless end is true. */
    write?: Buffer | string;
    end?: boolean;
  },
): { response: Promise<RawResponse>; write: (b: Buffer | string) => void; end: () => void; destroy: () => void } {
  const u = new URL(url);
  const req = httpRequest({
    host: u.hostname,
    port: u.port,
    method: options.method ?? "GET",
    path: options.path,
    headers: options.headers ?? {},
  });
  const response = new Promise<RawResponse>((resolve, reject) => {
    req.on("response", (res) => {
      const chunks: Buffer[] = [];
      res.on("data", (c: Buffer) => chunks.push(c));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks) }));
      res.on("error", reject);
    });
    req.on("error", reject);
  });
  if (options.write !== undefined) req.write(options.write);
  if (options.end === true) req.end();
  return {
    response,
    write: (b) => req.write(b),
    end: () => req.end(),
    destroy: () => req.destroy(),
  };
}

/** Assert helpers shared by both suites. */
export function isBare404(status: number, headers: Headers | IncomingMessage["headers"], body: string | Buffer): boolean {
  const get = (name: string): string | null => {
    if (headers instanceof Headers) return headers.get(name);
    const v = headers[name.toLowerCase()];
    return v === undefined ? null : Array.isArray(v) ? v.join(",") : v;
  };
  return (
    status === 404 &&
    body.length === 0 &&
    get("server-timing") === null &&
    get("cache-control") === null &&
    get("x-laghound-bytes") === null &&
    get("www-authenticate") === null
  );
}
