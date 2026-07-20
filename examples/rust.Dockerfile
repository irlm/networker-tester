# LagHound Rust sample — axum service nesting laghound::router.
# Build context MUST be the repo root: `docker build -f examples/rust.Dockerfile .`
# Multi-stage: compile with the full toolchain, run on a slim base with just the
# binary (the sample statically links everything it needs).
FROM rust:1-slim AS build
WORKDIR /src

# The example crate is a standalone workspace that depends on the SDK at `..`
# via a path dependency, so both trees are needed to build.
COPY sdk/rust ./rust

WORKDIR /src/rust/example
RUN cargo build --release && \
    cp target/release/laghound-sample /laghound-sample

FROM debian:trixie-slim AS runtime
WORKDIR /app
# wget for the compose healthcheck (slim base ships neither curl nor wget).
RUN apt-get update && apt-get install -y --no-install-recommends wget \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /laghound-sample ./laghound-sample

# Container listens on 8084 internally; compose maps it to the host port.
# LAGHOUND_TOKEN comes from the environment.
ENV PORT=8084
EXPOSE 8084
ENTRYPOINT ["/app/laghound-sample"]
