# AletheBench — Ruby Reference API

Ruby Rack application served by Puma.

## Why Puma (direct Rack, no Rails/Sinatra)?

Puma is the most widely deployed Ruby application server in production. It is the
default server for Rails, powers Heroku's Ruby platform, and is the standard
choice for any serious Ruby HTTP workload.

This implementation uses a **direct Rack application** instead of Rails or Sinatra
to eliminate framework overhead from the benchmark. Rack is the standard Ruby
web server interface (analogous to Python's WSGI/ASGI or Java's Servlet API).
Every Ruby web framework is built on Rack, so this represents the fastest path
through the Ruby HTTP stack without dropping to raw sockets.

The combination of Puma + direct Rack gives Ruby its best realistic performance
ceiling, which is the goal of AletheBench: compare runtimes under their idiomatic,
production-grade configurations.

## Endpoints

| Method | Path              | Description                               |
|--------|-------------------|-------------------------------------------|
| GET    | /health           | JSON health check with Ruby version       |
| GET    | /download/{size}  | Stream `size` bytes (0x42, 8 KiB chunks)  |
| POST   | /upload           | Consume body, return bytes received       |

All endpoints served over TLS on port 8443.

## Local development

```bash
bundle install

# Without TLS (development) — override bind in environment
BENCH_CERT_DIR=. puma -b tcp://0.0.0.0:8080 config.ru

# With TLS
puma -C puma.rb config.ru
```

## Docker

```bash
docker build -t alethabench-ruby .
docker run -p 8443:8443 \
    -v /path/to/certs:/opt/bench:ro \
    alethabench-ruby
```

## Deploy to VM

```bash
./deploy.sh user@host --cert-dir /path/to/local/certs
```
