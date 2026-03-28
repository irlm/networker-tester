# Multi-stage build for the C++ Boost.Beast reference server.
# Usage:
#   docker build -t alethabench-cpp .
#   docker run -p 8443:8443 -v /path/to/certs:/opt/bench:ro alethabench-cpp

# ── Build stage ──────────────────────────────────────────────────────────────
FROM ubuntu:24.04 AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential cmake libboost-system-dev libboost-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY server.cpp CMakeLists.txt ./

RUN mkdir build && cd build \
    && cmake .. -DCMAKE_BUILD_TYPE=Release \
    && make -j"$(nproc)"

# ── Runtime stage ────────────────────────────────────────────────────────────
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    libboost-system1.83.0 libssl3t64 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/build/server /usr/local/bin/server

ENV BENCH_PORT=8443
ENV BENCH_CERT_PATH=/opt/bench/cert.pem
ENV BENCH_KEY_PATH=/opt/bench/key.pem

EXPOSE 8443

CMD ["server"]
