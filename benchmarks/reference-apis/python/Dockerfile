FROM python:3.12-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY server.py .

EXPOSE 8443

ENV BENCH_CERT_DIR=/opt/bench

# Shell form so BENCH_WORKERS / BENCH_PORT env knobs apply (API-SPEC.md §3):
# workers default to logical CPU count; --no-access-log because per-request
# stderr I/O is measurement noise no other language pays (audit F11).
# Cert-absent → plain HTTP on the same port (application mode behind a
# TLS-terminating reverse proxy, audit F8), mirroring the Go/Node/Java pattern.
CMD CERT_DIR="${BENCH_CERT_DIR:-/opt/bench}"; \
    if [ -f "$CERT_DIR/cert.pem" ] && [ -f "$CERT_DIR/key.pem" ]; then \
        SSL_OPTS="--ssl-keyfile $CERT_DIR/key.pem --ssl-certfile $CERT_DIR/cert.pem"; \
    else \
        echo "no TLS certs in $CERT_DIR - serving plain HTTP on port ${BENCH_PORT:-8443} (application mode)" >&2; \
        SSL_OPTS=""; \
    fi; \
    exec uvicorn server:app \
    --host 0.0.0.0 \
    --port "${BENCH_PORT:-8443}" \
    --workers "${BENCH_WORKERS:-$(nproc)}" \
    --no-access-log \
    $SSL_OPTS
