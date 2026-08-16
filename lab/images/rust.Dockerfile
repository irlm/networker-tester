# syntax=docker/dockerfile:1.7
# ─── Rust build image: networker-tester + networker-endpoint ──────────────────
# Built ONCE per `lab.sh build` and consumed by runner/target images via
# `COPY --from=nwk-lab/rustbin`. Builds for the container's native arch, so it
# works identically on Apple Silicon (linux/arm64) and x86_64 hosts. Uses
# BuildKit cache mounts so incremental rebuilds only recompile changed crates.
#
# Feature set mirrors the release build (default features incl. http3 + browser
# + db-mssql); the binaries are dynamically linked against glibc (bookworm) and
# run on the ubuntu:24.04 runtime images below (glibc 2.39 >= 2.36).
ARG RUST_IMAGE=rust:1-bookworm
FROM ${RUST_IMAGE} AS builder

# aws-lc-sys (pulled by axum-server's rustls default provider in
# networker-endpoint) needs cmake + a C compiler; pkg-config for the rest.
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      cmake clang pkg-config libclang-dev perl \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY shared ./shared
COPY sql ./sql

# --locked: the lab must build exactly what CI/release builds (Cargo.lock is
# committed). Cache mounts keep the registry + incremental artifacts across
# builds; the final `cp` moves the binaries out of the cache mount.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p networker-tester -p networker-endpoint \
 && mkdir -p /out \
 && cp target/release/networker-tester target/release/networker-endpoint /out/ \
 && /out/networker-tester --version && /out/networker-endpoint --version

# ─── Export stage: just the two binaries ─────────────────────────────────────
FROM scratch AS rustbin
COPY --from=builder /out/networker-tester /networker-tester
COPY --from=builder /out/networker-endpoint /networker-endpoint
