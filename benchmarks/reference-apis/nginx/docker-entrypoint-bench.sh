#!/bin/sh
# docker-entrypoint-bench.sh — apply the API-SPEC.md env knobs (§1, §3) to the
# checked-in nginx.conf, verify the config, then start nginx.
#
# Knobs (all optional):
#   BENCH_WORKERS   → worker_processes   (default: auto = all logical CPUs)
#   BENCH_PORT      → listen port + Alt-Svc port (default: 8443)
#   BENCH_CERT_DIR  → ssl_certificate/_key directory (default: /opt/bench)
#   BENCH_API_TOKEN → bearer-token auth on every route except /health
set -eu

CONF=/etc/nginx/nginx.conf

# §3: BENCH_WORKERS → worker_processes
if [ -n "${BENCH_WORKERS:-}" ]; then
    sed -i "s/^worker_processes .*/worker_processes ${BENCH_WORKERS};/" "$CONF"
fi

# §1: BENCH_PORT → listen + Alt-Svc port
if [ -n "${BENCH_PORT:-}" ] && [ "${BENCH_PORT}" != "8443" ]; then
    sed -i "s/listen 8443/listen ${BENCH_PORT}/g; s/:8443\"/:${BENCH_PORT}\"/g" "$CONF"
fi

# §1: BENCH_CERT_DIR → cert paths (default /opt/bench)
if [ -n "${BENCH_CERT_DIR:-}" ] && [ "${BENCH_CERT_DIR}" != "/opt/bench" ]; then
    sed -i "s|/opt/bench/cert.pem|${BENCH_CERT_DIR}/cert.pem|; s|/opt/bench/key.pem|${BENCH_CERT_DIR}/key.pem|" "$CONF"
fi

# §1: BENCH_API_TOKEN → bearer auth map (see the $bench_auth_bad map in
# nginx.conf; exact "Bearer <token>" match passes, catch-all regex rejects).
# The token must not contain double quotes.
if [ -n "${BENCH_API_TOKEN:-}" ]; then
    printf '"Bearer %s" 0;\n~^ 1;\n' "${BENCH_API_TOKEN}" > /etc/nginx/bench-auth-token.conf
fi

nginx -t
exec "$@"
