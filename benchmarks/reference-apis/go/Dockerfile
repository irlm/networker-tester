# go.mod declares `go 1.24` (required by quic-go) — the builder image must
# match or `go build` fails with "go.mod requires go >= 1.24".
FROM golang:1.24-alpine AS builder
WORKDIR /build
COPY go.mod go.sum main.go ./
RUN CGO_ENABLED=0 go build -o server .

FROM scratch
COPY --from=builder /build/server /server
# TLS certs + bench-data.json must be mounted at runtime: -v /opt/bench:/opt/bench
# (dataset load failure is fatal per benchmarks/shared/API-SPEC.md §2).
EXPOSE 8443
ENTRYPOINT ["/server"]
