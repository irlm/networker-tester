FROM golang:1.22-alpine AS builder
WORKDIR /build
COPY go.mod main.go ./
RUN CGO_ENABLED=0 go build -o server .

FROM scratch
COPY --from=builder /build/server /server
# TLS certs must be mounted at runtime: -v /opt/bench:/opt/bench
EXPOSE 8443
ENTRYPOINT ["/server"]
