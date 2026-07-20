# laghound (Rust SDK)

Embed a tiny LagHound diagnostic endpoint into your **axum / tower** app. The
LagHound multi-cloud tester fleet then measures your *real* app from outside and
splits total request time into:

```
DNS  →  TCP  →  TLS  →  network transfer  →  server processing
```

The last phase — **server processing** — is the piece only your app can report,
and this SDK stamps it on every response as `Server-Timing: app;dur=<ms>` (plus
a `total` compat alias every already-deployed tester parses). That split is the
whole point.

Implements **endpoint contract v1** — the prose spec
([`docs/sdk/contract-v1.md`](../../docs/sdk/contract-v1.md)) and its
machine-readable twin ([`shared/sdk-contract-v1.json`](../../shared/sdk-contract-v1.json)),
which the conformance suite pins to.

## Quickstart (axum)

```toml
# Cargo.toml
laghound = { version = "1", features = ["axum"] }
```

```rust
use axum::{routing::get, Router};

#[tokio::main]
async fn main() {
    let token = std::env::var("LAGHOUND_TOKEN").expect("set LAGHOUND_TOKEN");

    let app = Router::new()
        .route("/", get(|| async { "my app" }))
        // Mounts GET /laghound/{health,echo,download,info} and POST /laghound/upload.
        .merge(laghound::router(laghound::Config::new(token)).unwrap());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    // ConnectInfo gives LagHound the real peer IP for per-IP rate limiting.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .unwrap();
}
```

`laghound::router(config)` returns an `axum::Router` you can `.merge()` (or
`.nest()`) into your app. It handles everything under the prefix and passes
anything else straight through, so it composes with your own routes.

## Without axum (bare tower)

The crate is a tower `Layer` / `Service` at its core; the `axum` feature is
only sugar. Wrap any inner `Service<Request<B>, Response<Response<...>>>`:

```rust
// default features (no axum)
let svc = laghound::service(inner_service, laghound::Config::new(token))?;
// or as a Layer:
let layer = laghound::layer(laghound::Config::new(token))?;
```

Insert a `laghound::ClientIp(ip)` request extension yourself for accurate
per-IP rate limiting (the axum router does this from `ConnectInfo`).

## Options

Builder on `Config` (all optional except the token):

| Method | Default | Contract |
|--------|---------|----------|
| `Config::new(token)` | — (required, ≥16 bytes) | §2, §5 |
| `Config::from_env()` | reads `LAGHOUND_TOKEN` | §2 |
| `.prefix("/laghound")` | `/laghound` | §2 |
| `.add_token(prev)` | — (≤2 total, for rotation) | §5 |
| `.download_cap(bytes)` | 4 MiB (hard max 32 MiB) | §2, §3.3 |
| `.upload_cap(bytes)` | 4 MiB (hard max 32 MiB) | §2, §3.4 |
| `.rate_per_ip(rps, burst)` | 10 / 20 | §6.2 |
| `.rate_global(rps, burst)` | 50 / 100 | §6.2 |
| `.max_concurrent(n)` | 8 | §6.3 |
| `.max_concurrent_transfers(n)` | 2 | §6.3 |
| `.byte_budget(bytes, window_s)` | off | §6.4 |
| `.app_name("checkout-api")` | off | §3.1, §3.5 |
| `.routes(RouteToggles { .. })` | all on | §3.1 |

Config is validated when the layer/router is built; it **fails closed** — a
missing/short token, a bad prefix, or more than two tokens returns a
`ConfigError` and refuses to mount rather than exposing open routes.

## Safety (contract §6)

Embedding LagHound can never make an outage worse:

- **Hard byte cap** — 32 MiB absolute max, config cannot exceed it.
- **Streamed download** — the `0x42` fill is sliced from a single per-process
  buffer; peak memory is O(chunk ≤ 64 KiB), never O(N).
- **Upload drain-and-count** — the body is counted, never buffered; a
  `Content-Length` over the cap is `413`d *without reading* the body.
- **Rate limits** — per-IP + global token buckets; the per-IP table is
  LRU-bounded so address-spraying can't grow memory.
- **Concurrency caps** — ≤ 8 in-flight, of which ≤ 2 transfers; a busy app
  serves at most 2 × 4 MiB of diagnostic traffic. A download's slot is held for
  the whole transfer (the permit rides in the streamed body).
- **Byte budget** — optional sliding window; exhaustion → `429` + `Retry-After`.
- **Kill switch** — `LAGHOUND_DISABLED=1` makes every route a bare 404 without
  a redeploy.
- **404-invisibility** — a bad/missing token, a rate-limited *unauthenticated*
  request, and the kill switch all return the same bare, header-less 404 as
  "route not found" — including `/health`. Rate limiting runs *before* auth, so
  token brute-forcing is throttled and stays invisible.
- **Constant-time auth** — tokens compared with `subtle::ConstantTimeEq`; length
  mismatch does not short-circuit observably.
- **Zero logging / zero reflection** — bodies and tokens are never logged; no
  route reflects request input; error messages are fixed strings.
- **Fail closed** — a panic inside LagHound code is caught and converted to a
  `500` envelope confined to the LagHound route; it never crashes the host.

## Authentication

Send the shared secret as either header (contract §5):

```
X-LagHound-Token: <token>
Authorization: Bearer <token>     # equivalent; X-LagHound-Token wins if both
```

## Conformance & tests

```bash
cargo test --all-features     # unit + conformance + safety + doc tests
cargo clippy --all-features --all-targets -- -D warnings
cargo fmt --all --check
```

`tests/conformance.rs` loads `shared/sdk-contract-v1.json` and drives the
service via `tower::ServiceExt::oneshot`; `tests/safety.rs` covers the §6
safety properties; `tests/killswitch.rs` isolates the `LAGHOUND_DISABLED` env
test.

> This crate is a **standalone workspace**, intentionally excluded from the repo
> root `Cargo.toml` so it versions independently and never triggers the root's
> five-file version-sync. Run cargo commands from within `sdk/rust/`.

## Sample app

`example/` is a runnable axum service nesting `laghound::router` at `/laghound`
plus two app routes:

```bash
cd example
cargo run
# LAGHOUND_TOKEN defaults to "demo-token-laghound", PORT to 8084.

curl -s http://localhost:8084/            # -> "rust sample ok"
curl -s http://localhost:8084/work        # -> ~30ms of work (tokio sleep)

# LagHound routes (need the token):
curl -si -H "X-LagHound-Token: demo-token-laghound" \
  http://localhost:8084/laghound/health   # -> 200 + Server-Timing: app;dur=...
curl -si http://localhost:8084/laghound/health   # -> bare 404 (no token)
curl -si -H "Authorization: Bearer demo-token-laghound" \
  "http://localhost:8084/laghound/download?bytes=1024"   # -> octet-stream + X-LagHound-Bytes
```

Override with `PORT=9000 LAGHOUND_TOKEN=my-secret-token cargo run`.
