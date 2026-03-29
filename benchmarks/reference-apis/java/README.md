# AletheBench Java Reference API

HTTPS server for AletheBench using JDK 21 built-in APIs only.

## Why JDK Built-in HttpsServer

This implementation deliberately avoids frameworks like Spring Boot, Netty, or
Tomcat. The goal is to measure the Java platform itself:

- **No framework overhead** — `com.sun.net.httpserver.HttpsServer` is part of the
  JDK. There is no classpath scanning, dependency injection, or annotation
  processing at startup. Cold-start time reflects the JVM, not a framework.
- **Minimal footprint** — the compiled JAR is a few kilobytes. Memory usage at
  idle is the JVM baseline, not inflated by connection pools and caches that
  frameworks allocate eagerly.
- **Reproducible** — the only dependency is the JDK itself. No build tool
  (Maven/Gradle) is required, no transitive dependency graph to audit.

## Why Virtual Threads

Virtual Threads (JEP 444, production in Java 21) provide scalable concurrency
without thread pool tuning:

- Each request runs on its own virtual thread. The JVM multiplexes millions of
  virtual threads onto a small pool of carrier (OS) threads.
- Blocking I/O (reading uploads, writing downloads) yields the carrier thread
  automatically — no need for async/reactive APIs.
- No `ThreadPoolExecutor` sizing decisions that would affect benchmark results.

## Endpoints

| Method | Path              | Description                              |
|--------|-------------------|------------------------------------------|
| GET    | `/health`         | `{"status":"ok","language":"java",...}`   |
| GET    | `/download/{size}`| Stream `size` bytes of zeros             |
| POST   | `/upload`         | Read body, return `{"bytes_received": N}`|

All endpoints listen on port 8443 (HTTPS).

## Quick Start

```bash
# Build
./build.sh

# Run locally (needs cert.pem + key.pem in /opt/bench or set BENCH_CERT_DIR)
BENCH_CERT_DIR=../../shared java -jar server.jar

# Docker
docker build -t alethabench-java .
docker run -v /opt/bench:/opt/bench -p 8443:8443 alethabench-java

# Deploy to VM
./deploy.sh user@host
```

## Requirements

- JDK 21+ (for Virtual Threads)
- TLS certificate and key in PEM format at `$BENCH_CERT_DIR` (default `/opt/bench`)
