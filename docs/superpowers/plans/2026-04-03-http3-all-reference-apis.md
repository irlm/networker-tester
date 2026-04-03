# HTTP/3 QUIC Support for All Reference APIs

> **For agentic workers:** These tasks are INDEPENDENT — dispatch one agent per language in parallel using isolated worktrees. Each task modifies only its own directory under `benchmarks/reference-apis/`. No conflicts possible.

**Goal:** Add HTTP/3 (QUIC) support to all benchmark reference API servers so every language can be tested with `--modes http3`.

**Architecture:** Each reference API server adds a QUIC listener on the same port (8443 UDP) alongside the existing TLS TCP listener. The server serves the same 3 endpoints (/health, /download/{size}, /upload) over HTTP/3. All servers read TLS certs from `BENCH_CERT_DIR` env var (default `/opt/bench`).

**Tech Stack:** Language-specific QUIC libraries (see per-task details)

---

## Shared Requirements (apply to ALL languages)

Every HTTP/3 implementation MUST:
1. Listen on UDP port 8443 (same port as TCP for HTTP/1.1+HTTP/2)
2. Use the same TLS certificate (`$BENCH_CERT_DIR/cert.pem` + `key.pem`)
3. Serve the same 3 endpoints: `GET /health`, `GET /download/{size}`, `POST /upload`
4. Return the same JSON responses as the HTTP/1.1+HTTP/2 server
5. Advertise HTTP/3 via `Alt-Svc: h3=":8443"; ma=86400` header on HTTP/2 responses
6. NOT break HTTP/1.1 or HTTP/2 — both must continue working

## Test procedure (same for all languages)

After deploying, verify with networker-tester:
```bash
networker-tester --target https://<vm-ip>:8443/health --modes http1,http2,http3 --runs 3 --insecure
```
Expected: All 3 modes show successful responses with latency > 0.

---

## Task 1: Go — Add HTTP/3 via quic-go

**Files:**
- Modify: `benchmarks/reference-apis/go/main.go`
- Modify: `benchmarks/reference-apis/go/go.mod` (add dependency)

**Current state:** Uses `net/http` with `ListenAndServeTLS`. No external dependencies.

- [ ] **Step 1: Add quic-go dependency**

```bash
cd benchmarks/reference-apis/go
go get github.com/quic-go/quic-go/http3
```

This adds the `quic-go` HTTP/3 server library.

- [ ] **Step 2: Add HTTP/3 server alongside existing HTTP/2 server**

In `main.go`, after the existing `srv.ListenAndServeTLS()` call (which blocks), restructure to run both servers concurrently:

```go
import (
    "github.com/quic-go/quic-go/http3"
    // ... existing imports
)

func main() {
    // ... existing handler setup (mux with /health, /download, /upload) ...

    certPath := filepath.Join(certDir, "cert.pem")
    keyPath := filepath.Join(certDir, "key.pem")

    // Add Alt-Svc header to advertise HTTP/3
    handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
        w.Header().Set("Alt-Svc", `h3=":8443"; ma=86400`)
        mux.ServeHTTP(w, r)
    })

    // HTTP/1.1 + HTTP/2 (TCP)
    tcpServer := &http.Server{
        Addr:    listenAddr,
        Handler: handler,
        TLSConfig: &tls.Config{MinVersion: tls.VersionTLS12},
    }

    // HTTP/3 (QUIC/UDP)
    h3Server := &http3.Server{
        Addr:    listenAddr,
        Handler: handler,
    }

    // Run both concurrently
    go func() {
        log.Printf("HTTP/3 (QUIC) listening on %s", listenAddr)
        if err := h3Server.ListenAndServeTLS(certPath, keyPath); err != nil {
            log.Printf("HTTP/3 server error: %v", err)
        }
    }()

    log.Printf("HTTP/1.1+HTTP/2 listening on %s", listenAddr)
    log.Fatal(tcpServer.ListenAndServeTLS(certPath, keyPath))
}
```

- [ ] **Step 3: Build and verify**

```bash
cd benchmarks/reference-apis/go && go build -o server . && echo "Build OK"
```

- [ ] **Step 4: Commit**

```bash
git add benchmarks/reference-apis/go/
git commit -m "feat(bench): add HTTP/3 QUIC support to Go reference API"
```

---

## Task 2: Node.js — Add HTTP/3 via node built-in or quic library

**Files:**
- Modify: `benchmarks/reference-apis/nodejs/server.js`
- Modify: `benchmarks/reference-apis/nodejs/package.json` (if adding dependency)

**Current state:** Uses `node:http2` with `createSecureServer`. Port from `PORT` env.

Node.js 20+ has experimental HTTP/3 behind a flag, but it's not stable. The recommended approach is to use a reverse proxy or the `@pvibes/h3` library. However, for simplicity and no-dependency approach, we can use the built-in `node:net` with a QUIC library.

**Recommended approach:** Use `@pvibes/h3` (lightweight HTTP/3 server for Node.js).

Alternative: If the target Node.js version is 22+, use the experimental `node --experimental-quic` flag.

- [ ] **Step 1: Add HTTP/3 dependency**

```bash
cd benchmarks/reference-apis/nodejs
npm install @pvibes/h3
```

If `@pvibes/h3` is not available or too experimental, use `quic` npm package or implement a fallback that gracefully skips HTTP/3 if the library isn't available.

**Alternative simpler approach — run HTTP/3 via a Go sidecar:** Since Go's quic-go is the most mature QUIC implementation, an alternative is to have the Node.js server only handle HTTP/1.1+HTTP/2 and skip HTTP/3. Document this clearly.

- [ ] **Step 2: Add HTTP/3 listener or document limitation**

If using a library, add alongside existing `http2.createSecureServer()`:

```javascript
// Add Alt-Svc header to advertise HTTP/3
const originalHandler = handleRequest;
function handleWithAltSvc(req, res) {
    res.setHeader('Alt-Svc', 'h3=":' + PORT + '"; ma=86400');
    originalHandler(req, res);
}
```

If no mature HTTP/3 library exists for Node.js, document that HTTP/3 is not supported and add a comment explaining why. The `Alt-Svc` header should NOT be sent if HTTP/3 is not available.

- [ ] **Step 3: Commit**

```bash
git add benchmarks/reference-apis/nodejs/
git commit -m "feat(bench): add HTTP/3 support to Node.js reference API (or document limitation)"
```

---

## Task 3: Python — Enable HTTP/3 via Hypercorn

**Files:**
- Modify: `benchmarks/reference-apis/python/requirements.txt`
- Modify: `benchmarks/reference-apis/python/server.py` (add Alt-Svc header)
- Modify: deploy command in `install.sh` `deploy_benchmark_server()` python section

**Current state:** Uses Starlette + Uvicorn. TLS via uvicorn CLI flags.

**Approach:** Replace Uvicorn with Hypercorn, which has built-in HTTP/3 support via `aioquic`.

- [ ] **Step 1: Update requirements.txt**

```
starlette
hypercorn[h3]
```

Replace `uvicorn` with `hypercorn[h3]` which includes `aioquic` for QUIC support.

- [ ] **Step 2: Add Alt-Svc middleware to server.py**

```python
from starlette.middleware import Middleware
from starlette.responses import Response

class AltSvcMiddleware:
    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] == "http":
            async def send_with_alt_svc(message):
                if message["type"] == "http.response.start":
                    headers = list(message.get("headers", []))
                    port = os.environ.get("BENCH_PORT", "8443")
                    headers.append((b"alt-svc", f'h3=":{port}"; ma=86400'.encode()))
                    message["headers"] = headers
                await send(message)
            await self.app(scope, receive, send_with_alt_svc)
        else:
            await self.app(scope, receive, send)

app = Starlette(...)  # existing app
app = AltSvcMiddleware(app)
```

- [ ] **Step 3: Update install.sh deploy command**

In `install.sh`, in the `deploy_benchmark_server()` function, update the python case to use hypercorn instead of uvicorn:

```bash
        python)
            # ... existing venv setup ...
            BENCH_CERT_DIR="$BENCH_DIR" \
                nohup "$BENCH_DIR/pyenv/bin/hypercorn" server:app \
                    --bind "0.0.0.0:8443" \
                    --certfile "$CERT_PEM" --keyfile "$CERT_KEY" \
                    --quic-bind "0.0.0.0:8443" \
                    > "$BENCH_DIR/python.log" 2>&1 &
```

The `--quic-bind` flag enables HTTP/3 on the same port.

- [ ] **Step 4: Commit**

```bash
git add benchmarks/reference-apis/python/ install.sh
git commit -m "feat(bench): add HTTP/3 QUIC support to Python reference API via Hypercorn"
```

---

## Task 4: C# .NET — Enable HTTP/3 in Kestrel

**Files:**
- Modify: `benchmarks/reference-apis/csharp-net8/Program.cs` (and all other .NET versions)

**Current state:** Kestrel configured with `HttpProtocols.Http1AndHttp2`. HTTP/3 is a one-line change.

**Note:** HTTP/3 in Kestrel requires .NET 7+. For .NET 6, HTTP/3 is experimental. For .NET 4.8 (Windows Framework), HTTP/3 is not available.

- [ ] **Step 1: Enable HTTP/3 in Kestrel for .NET 7+ versions**

In each `Program.cs` (csharp-net7, net8, net8-aot, net9, net9-aot, net10, net10-aot), change the protocol configuration:

```csharp
// Before:
listenOptions.Protocols = HttpProtocols.Http1AndHttp2;

// After:
listenOptions.Protocols = HttpProtocols.Http1AndHttp2AndHttp3;
```

Also add the `Alt-Svc` header. In the middleware pipeline, add:

```csharp
app.Use(async (context, next) =>
{
    context.Response.Headers["Alt-Svc"] = $"h3=\":{port}\"; ma=86400";
    await next();
});
```

- [ ] **Step 2: For .NET 6, keep HTTP/2 only (HTTP/3 experimental)**

In `csharp-net6/Program.cs`, keep `HttpProtocols.Http1AndHttp2` and add a comment:
```csharp
// HTTP/3 not supported on .NET 6 (experimental only, requires preview flag)
```

- [ ] **Step 3: Build each variant to verify**

```bash
for dir in csharp-net7 csharp-net8 csharp-net8-aot csharp-net9 csharp-net9-aot csharp-net10 csharp-net10-aot; do
    cd benchmarks/reference-apis/$dir && dotnet build && echo "$dir OK" && cd -
done
```

- [ ] **Step 4: Commit**

```bash
git add benchmarks/reference-apis/csharp-*/
git commit -m "feat(bench): enable HTTP/3 in all .NET 7+ Kestrel reference APIs"
```

---

## Task 5: Java — Add HTTP/3 via Jetty or document limitation

**Files:**
- Modify: `benchmarks/reference-apis/java/Server.java`

**Current state:** Uses JDK built-in `com.sun.net.httpserver.HttpsServer`. No external dependencies.

**Challenge:** The JDK built-in HTTP server has no HTTP/3 support. Adding HTTP/3 requires either:
- Jetty 12+ (has HTTP/3 via `jetty-http3-server`) — adds significant dependency
- Netty with `netty-incubator-codec-quic` — complex
- A standalone QUIC proxy

**Recommended approach:** Since the goal is minimal code, add Jetty HTTP/3 support. However, this changes the server from zero-dependency to framework-based. If the goal is to keep it minimal, document the limitation.

**Pragmatic approach:** Use the Jetty HTTP/3 module.

- [ ] **Step 1: Convert to Maven/Gradle project or add Jetty JAR**

Create `pom.xml` for Maven build:

```xml
<project>
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.alethabench</groupId>
    <artifactId>java-bench-server</artifactId>
    <version>1.0</version>
    <dependencies>
        <dependency>
            <groupId>org.eclipse.jetty</groupId>
            <artifactId>jetty-server</artifactId>
            <version>12.0.14</version>
        </dependency>
        <dependency>
            <groupId>org.eclipse.jetty.http3</groupId>
            <artifactId>jetty-http3-server</artifactId>
            <version>12.0.14</version>
        </dependency>
    </dependencies>
</project>
```

**Alternative:** Keep the current `HttpsServer` for HTTP/1.1+HTTP/2 and document that Java's built-in server doesn't support HTTP/3. This is the honest approach — Java's stdlib doesn't have QUIC.

- [ ] **Step 2: Implement or document**

If adding Jetty: rewrite Server.java to use Jetty with HTTP/3 connector.
If documenting: add a comment to Server.java and update the benchmark README.

- [ ] **Step 3: Commit**

```bash
git add benchmarks/reference-apis/java/
git commit -m "feat(bench): add HTTP/3 support to Java reference API (or document limitation)"
```

---

## Task 6: C++ — Add HTTP/3 via ngtcp2+nghttp3 or document limitation

**Files:**
- Modify: `benchmarks/reference-apis/cpp/server.cpp`
- Modify: `benchmarks/reference-apis/cpp/CMakeLists.txt`

**Current state:** Uses Boost.Beast + Boost.Asio + OpenSSL. No QUIC.

**Challenge:** C++ QUIC is complex. Options:
- `ngtcp2` + `nghttp3` (low-level, requires manual integration)
- Cloudflare's `quiche` (Rust-based with C bindings)
- `lsquic` (LiteSpeed QUIC library)
- `msquic` (Microsoft's QUIC implementation)

**Recommended:** Use `msquic` or `lsquic` as they have the simplest C/C++ APIs. However, all add significant build complexity.

**Pragmatic approach:** Document HTTP/3 as unsupported for C++ benchmark and skip. The C++ benchmark tests raw TCP/TLS performance — HTTP/3 adds QUIC transport which is a fundamentally different test.

- [ ] **Step 1: Document limitation or implement with msquic**

If implementing: add `msquic` dependency to CMakeLists.txt and add QUIC listener.
If documenting: add comment to server.cpp.

- [ ] **Step 2: Commit**

```bash
git add benchmarks/reference-apis/cpp/
git commit -m "feat(bench): document HTTP/3 limitation for C++ reference API"
```

---

## Task 7: Ruby — Document HTTP/3 limitation

**Files:**
- Modify: `benchmarks/reference-apis/ruby/config.ru` (add comment)

**Current state:** Uses Puma + Rack. Puma has no QUIC support.

**Reality:** No mature Ruby HTTP/3 server exists. Puma, Unicorn, and Falcon don't support QUIC. This is a language ecosystem limitation.

- [ ] **Step 1: Document limitation**

Add to `config.ru`:
```ruby
# HTTP/3 (QUIC) is not supported by Puma. No mature Ruby QUIC server exists.
# Benchmarks will run with HTTP/1.1 and HTTP/2 only.
```

- [ ] **Step 2: Commit**

```bash
git add benchmarks/reference-apis/ruby/
git commit -m "docs(bench): document HTTP/3 limitation for Ruby (no Puma QUIC support)"
```

---

## Task 8: PHP — Enable HTTP/3 via Swoole QUIC

**Files:**
- Modify: `benchmarks/reference-apis/php/server.php`

**Current state:** Uses Swoole HTTP server with TLS. Swoole 5.1+ has experimental QUIC support.

**Challenge:** Swoole's QUIC support requires compilation with `--enable-quic` flag and OpenSSL 3.0+. The PECL install may not include QUIC by default.

- [ ] **Step 1: Check Swoole QUIC availability and enable if possible**

```php
// Add HTTP/3 if Swoole supports QUIC
if (defined('SWOOLE_SOCK_UDP') && defined('SWOOLE_QUIC')) {
    $server->addListener('0.0.0.0', $port, SWOOLE_SOCK_UDP | SWOOLE_SSL | SWOOLE_QUIC);
}
```

If Swoole QUIC is not available on the target system, document the limitation.

- [ ] **Step 2: Update install.sh to install Swoole with QUIC**

In `deploy_benchmark_server()` php section, add:
```bash
sudo pecl install swoole -- --enable-quic < /dev/null
```

- [ ] **Step 3: Commit**

```bash
git add benchmarks/reference-apis/php/ install.sh
git commit -m "feat(bench): enable HTTP/3 QUIC in PHP Swoole reference API (if available)"
```

---

## Task 9: nginx — Already done (config + mainline install)

**Status: COMPLETE** — nginx.conf already has `listen 8443 quic reuseport;` and `install.sh` installs nginx mainline 1.27+.

No additional work needed.

---

## Task 10: Orchestrator — Skip HTTP/3 for unsupported languages

**Files:**
- Modify: `benchmarks/orchestrator/src/executor.rs`

After all languages have been updated, some (Ruby, possibly Java, C++) still won't support HTTP/3. The orchestrator should skip `http3` from the modes list for these languages to avoid wasting time on guaranteed failures.

- [ ] **Step 1: Add HTTP/3 capability list**

```rust
/// Languages that support HTTP/3 (QUIC).
/// Others will have http3 stripped from modes to avoid wasted benchmark time.
fn supports_http3(language: &str) -> bool {
    matches!(language,
        "rust" | "nginx" | "go" | "python"
        | "csharp-net7" | "csharp-net8" | "csharp-net8-aot"
        | "csharp-net9" | "csharp-net9-aot"
        | "csharp-net10" | "csharp-net10-aot"
        | "php"
    )
}
```

- [ ] **Step 2: Filter modes in run_language_benchmark**

Before building the tester args, filter http3 if the language doesn't support it:

```rust
    let effective_modes = if supports_http3(language) {
        modes.to_string()
    } else {
        modes.split(',')
            .filter(|m| m.trim() != "http3")
            .collect::<Vec<_>>()
            .join(",")
    };
```

Use `effective_modes` instead of `modes` for the `--modes` arg.

- [ ] **Step 3: Build and test**

```bash
cd benchmarks/orchestrator && cargo build
```

- [ ] **Step 4: Commit**

```bash
git add benchmarks/orchestrator/src/executor.rs
git commit -m "feat: orchestrator skips http3 mode for languages without QUIC support"
```

---

## Parallel Execution Strategy

Tasks 1-8 are **fully independent** — each modifies only its own language directory. Run them in parallel:

```
Agent 1: Task 1 (Go)        → worktree: feat/h3-go
Agent 2: Task 2 (Node.js)   → worktree: feat/h3-nodejs
Agent 3: Task 3 (Python)    → worktree: feat/h3-python
Agent 4: Task 4 (C# .NET)   → worktree: feat/h3-csharp
Agent 5: Task 5 (Java)      → worktree: feat/h3-java
Agent 6: Task 6 (C++)       → worktree: feat/h3-cpp
Agent 7: Task 7 (Ruby)      → worktree: feat/h3-ruby
Agent 8: Task 8 (PHP)       → worktree: feat/h3-php
```

After all merge, run Task 10 (orchestrator skip logic) on main.

## Priority Order

If doing sequentially instead of parallel:
1. **Go** (most impactful — quic-go is mature, Go is a primary benchmark language)
2. **C# .NET** (one-line change, covers 7 variants)
3. **Python** (swap uvicorn → hypercorn, medium effort)
4. **nginx** (already done)
5. **Orchestrator skip logic** (prevents wasted time on unsupported languages)
6. **PHP** (if Swoole QUIC works)
7. **Java** (requires Jetty, significant rewrite)
8. **Node.js** (limited library support)
9. **C++** (very complex, low priority)
10. **Ruby** (document only — no QUIC library exists)
