# LagHound Go sample — net/http service mounting the LagHound endpoint.
# Build context MUST be the repo root: `docker build -f examples/go.Dockerfile .`
# Multi-stage: compile with the Go toolchain, run the static binary on a distroless-ish base.
FROM golang:1.26 AS build
WORKDIR /src

# The go.mod (module github.com/irlm/networker-tester/sdk/go/laghound) roots at
# sdk/go; the example package lives under it at example/. Copy the whole tree.
COPY sdk/go ./go

WORKDIR /src/go/example
# CGO off -> a fully static binary that runs on a scratch/slim base.
RUN CGO_ENABLED=0 go build -o /laghound-sample .

FROM debian:trixie-slim AS runtime
WORKDIR /app
# wget for the compose healthcheck (slim base ships neither curl nor wget).
RUN apt-get update && apt-get install -y --no-install-recommends wget \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /laghound-sample ./laghound-sample

# Container listens on 8085 internally; compose maps it to the host port.
# LAGHOUND_TOKEN comes from the environment.
ENV PORT=8085
EXPOSE 8085
ENTRYPOINT ["/app/laghound-sample"]
