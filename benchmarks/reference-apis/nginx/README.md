# Nginx Static File Baseline

This is **not** a real API implementation. It is the theoretical maximum
baseline: pure network stack, kernel `sendfile`, zero application code.

Nginx serves the same `/health`, `/download/{size}`, and `/upload` endpoints
as the application-level reference APIs (Rust, C#, Go, etc.), but using only
static files and built-in nginx directives. This shows the ceiling that no
application server can exceed on the same hardware.

## What it measures

- **Download**: kernel-level `sendfile()` of pre-generated blobs through TLS.
  This is the fastest possible file delivery on Linux.
- **Health**: a trivial `return 200` with a static JSON string. Measures the
  minimum overhead of the TLS + HTTP/2 stack.
- **Upload**: nginx reads the full POST body, then returns a fixed JSON
  response. It cannot report `bytes_received` accurately (the response is
  static), so this measures raw ingestion throughput only.

## Limitations

- `/download/{size}` only works for pre-generated sizes (1024, 8192, 65536,
  1048576). To add more, edit `generate-download-files.sh`.
- `/upload` always returns `{"bytes_received":"unknown"}` because nginx has
  no application logic to count bytes.
- No HTTP/3 (QUIC). Nginx mainline does not include stable QUIC support in
  the default package. Use the Rust endpoint for HTTP/3 benchmarks.

## Deploy

```bash
# 1. Generate the shared TLS certificate (one-time)
bash benchmarks/shared/generate-cert.sh

# 2. Deploy to a target VM
bash benchmarks/reference-apis/nginx/deploy.sh user@hostname
```
