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
CMD uvicorn server:app \
    --host 0.0.0.0 \
    --port "${BENCH_PORT:-8443}" \
    --workers "${BENCH_WORKERS:-$(nproc)}" \
    --no-access-log \
    --ssl-keyfile "${BENCH_CERT_DIR:-/opt/bench}/key.pem" \
    --ssl-certfile "${BENCH_CERT_DIR:-/opt/bench}/cert.pem"
