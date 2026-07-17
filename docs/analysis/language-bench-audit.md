# Language Benchmark Test Suite Audit

Date: 2026-07-16 · Scope: `benchmarks/reference-apis`, `benchmarks/validate`, `benchmarks/orchestrator`, `crates/networker-endpoint`, product surface (dashboard + control planes) · Method: full source read of all 17 implementations + validator + CI history. READ-ONLY audit; findings verified against code (runtime-confirmation items are marked).

## Executive summary

The suite is **not fit for cross-language comparison today**. The 17 implementations split into **three incompatible contract families** — the same `/api/*` endpoint returns different JSON shapes and performs different work depending on language, so any language ranking compares different workloads. The **Rust baseline itself fails the orchestrator's own contract** (`/download?bytes=` vs `/download/{size}`, `/health` missing `runtime`), Java fails it too (`/health` missing `version`), and the only committed results file contains **zero results**. The shared dataset (`bench-data.json`) that install.sh ships to every VM **breaks four languages at runtime** (Python 500s on `/api/aggregate`, Node emits NaN garbage, Go zeroes its data, C# silently falls back to PRNG — i.e., different input data per language). The validation workflow has **failed all 14 runs since its creation (2026-04-06, never green)** — a single Ruby `bundle install` failure aborts the whole matrix — and even when it runs it only checks liveness, not shape or content. At least three Docker images appear unbuildable/unbootable (Python runs `uvicorn` that is never installed; Java's jar omits the API handler classes; Go's Dockerfile never copies `go.sum`). Worker counts are inconsistent and undocumented (PHP pinned to 4, Node/Python/Ruby single-process, compiled languages all-cores), and the one workload the weekly CI actually measures (`/health`) does per-language different work (Java serves a precomputed static string; Rust formats a timestamp per request). Finally, nothing in the product ever measures the `/api/*` JSON workloads — the Application Benchmark UI only drives http1/2/3 + download/upload.

**P0: 7 · P1: 14 · P2: 9**

---

## 1. Endpoint × language parity matrix

### The de-facto contracts

There is no written endpoint spec under `benchmarks/shared/`. Three contracts exist in practice:

- **Orchestrator contract** (`benchmarks/orchestrator/src/validator.rs:70-72,138-199`, `deployer.rs:680-706`, `runner.rs:384`): `GET /health` → `{status:"ok", runtime, version}`; `GET /download/{size}` → exactly N bytes; `POST /upload`.
- **Shared-dataset contract** (`benchmarks/reference-apis/generate-bench-data.py`): users are `{id,name,email,score:float,created_at}` (no `age`/`active`/`department`); `timeseries` entries are objects `{ts,value,category}`; `expected_checksums` keys `users_page1|aggregate_summary|search_network_top10|transform_input0`. Notably its `transform_input0` checksum (`{hashed_fields, reversed_values}`, lines 124-137) matches **only the Rust endpoint's** transform contract — no other language implements it.
- **The Rust endpoint** (`crates/networker-endpoint/src/routes.rs`) — nominally the reference — disagrees with both on `/download` and `/health`.

### Shape families

| Family | Members | `/api/users` | `/api/transform` | `/api/aggregate` | `/api/search` | `/api/validate` |
|---|---|---|---|---|---|---|
| **A** | Go (`go/main.go`), C# net6–net10 ± AOT (`csharp-*/Program.cs`), C# net48, Java (mostly) | bare array `[{id,name,email,age,score:int,active,created_at}]` | `{key: sha256hex \| reversed-array \| passthrough}` | `{count,mean,p50,p95,max,categories:{alpha:{count,sum,mean}…}}` (buckets by `i%5`) | bare array `[{index,text,score}]`; missing `q` → 400 | Go: flat `{seed,…}`; C#: raw `expected_checksums` with no `seed` wrapper (Program.cs:513-522); net48: `{seed,version,checksums}` (Server.cs:668) |
| **B** | Node (`nodejs/server.js`), Python (`python/server.py`), PHP (`php/server.php`), Ruby (`ruby/config.ru`), C++ (`cpp/server.cpp`) | wrapper `{page,page_size,total,sort,order,users:[{id,name,email,age,department,score:float}]}` | `{original_fields, transformed:{key:{original_reversed,sha256}}}` | `{range:{start,end},total_points,stats:{mean,p50,p95,max},groups:{alpha:{count,mean,p50,max}}}` (PRNG category assignment) | `{query,total_matches,limit,results:[{id,text,score}]}`; missing `q` → default `"test"` | `{seed,checksums:{users_page1,aggregate_start0,search_corpus}}` (16-char truncated) |
| **C** | Rust `networker-endpoint` (routes.rs:1183-1615) and its C# port `src/Networker.Endpoint/Endpoints.cs` | bare array of 20 `{id,name,email,score:float,created_at}` | `{seed,hashed_fields,reversed_values}` (requires `{seed,fields,values}` body) | `{total_points,mean,p50,p95,max,categories:[q1..q5 quintiles]}` | `{query,total_matches,returned,results:[{rank,item,match_position}]}`; raw-regex, case-sensitive | `{seed,checksums:{users,aggregate,transform,search}}` |

Java straddles A and B: A-shaped payloads but B-style `checksums.users_page1` in validate (Server.java:1112), and a hand-written JSON scanner (Server.java:302-348, 728-830) that mis-parses escaped quotes.

### Per-endpoint drift highlights

| Endpoint | Drift (file:line) |
|---|---|
| `GET /health` | **Java** returns `{status,language,runtime}` — no `version` → fails validator.rs:169 (Server.java:418-421). **Rust** returns `{status,timestamp,service,version}` — no `runtime` → fails validator.rs:166 (routes.rs:737-744). nginx hardcodes `"version":"latest"` (nginx.conf:49). All others conform. |
| `GET /download/{size}` | **Rust + C# endpoint port use `?bytes=` query** (routes.rs:786-795; Endpoints.cs:41,170) → `GET /download/1024` is a 404. **csharp-net48** routes exact-match `case "/download"` (Server.cs:279) — path-style likely 404 (needs runtime confirm). Fill byte: 0x42 everywhere except **Java (zeros, 64 KiB chunks**, Server.java:434-435) and Rust (zeros, 64 KiB). Chunk size 8 KiB elsewhere — chunking is part of the measured download workload. |
| `POST /api/transform` | Three disjoint request/response contracts (see families). The canonical checksum in generate-bench-data.py:124-137 matches only family C. |
| `GET /api/aggregate` | `range` required (400) in Go/C#/Java (main.go:427-436; Program.cs:322-331; Server.java:846-859); defaulted in family B and Rust. Category bucketing: `i%5` (A) vs per-request PRNG draw ×10,000 (Node server.js:432-437; Python server.py:261-262) vs dataset field (PHP/Ruby/Java/C++) vs quintiles (Rust) — **different CPU work per language**. |
| `GET /api/search` | `total_matches` counted **after truncation** in Python (server.py:329-334), Ruby (config.ru:313-319), PHP (server.php:417-424); before truncation in Node (server.js:518-523) and Go — same field, different value. `limit` unclamped in Python (server.py:304). Rust treats `q` as raw case-sensitive regex (routes.rs:1409); everyone else escapes + IgnoreCase. |
| `POST /api/upload/process` | zlib (Rust/Node/Python/PHP/Ruby) vs **raw deflate** (Go `flate` main.go:590; C# `DeflateStream` Program.cs:466) → different `compressed_size` values and different CPU cost; C# uses `CompressionLevel.Optimal` vs Go default — level mismatch. `compression_ratio` field only in family B. |
| `GET /api/delayed` | Default `ms`: Go/C# 1, Rust 10, Node 50, Python/PHP/Ruby 100. **Python has no clamp** (server.py:372-375) — `?ms=600000` sleeps 10 min (also a DoS). `work=light` implemented in B, `work=heavy` in Go/C#, ignored by Rust (routes.rs:1493-1494). |
| `GET /api/validate` | Five different output shapes; every implementation just **echoes the file's checksums** — none recompute from their own output, so validate proves nothing about cross-language correctness. |

### Shared-dataset correctness (bench-data.json deployed by `install.sh:10546-10549` to every VM)

| Language | Behavior with the real dataset |
|---|---|
| Python | `/api/aggregate` → `sum(dicts)` **TypeError → HTTP 500** (server.py:253-265) |
| Node | `/api/aggregate` → objects summed → NaN/garbage stats (server.js:420-443) |
| Go | timeseries objects unmarshal-into-float silently fails → **all-zero values** (main.go:441-444); users `score:4.57` into `int` field errors, error ignored → corrupted users (main.go:331-338) |
| C# | `GetProperty("age")` / `GetDouble()` throw → **silent PRNG fallback** (Program.cs:226-242, 335-343) — C# benchmarks different data than PHP/Ruby |
| PHP/Ruby/Java/C++ | read `value`/`category` correctly |

So under production deployment the same request produces 500s, garbage, or different input data depending on language. **Nothing catches this**: the validate compose mounts only certs into `/opt/bench` (docker-compose.yml:22-24) and Dockerfiles can't COPY `../shared` (outside build context), so validation only ever exercises PRNG fallback.

---

## 2. Fairness findings

| # | Sev | Finding | Evidence | Fix |
|---|-----|---------|----------|-----|
| F1 | **P0** | Three contract families = different workloads per language (shapes, required params, regex semantics, category bucketing, compression algo). Rankings compare apples to oranges. | Section 1 | Write `benchmarks/shared/API-SPEC.md` as the single contract (pick family B or the Rust shapes), port all implementations, verify with checksum-recomputing validation. |
| F2 | **P0** | Shared dataset breaks 4 languages at runtime (500 / NaN / zeros / silent PRNG). | main.go:331-344,441-444; Program.cs:226-242,335-343; server.js:420-443; server.py:253-265 | Fix parsers to the generate-bench-data.py schema; make fallback loud (fail startup if `BENCH_DATA_PATH` set but unparsable). |
| F3 | **P0** | Worker/process counts inconsistent and undocumented: PHP Swoole pinned `worker_num=4` (server.php:49), Node 1 process (server.js:797-805), Python uvicorn 1 worker (python/Dockerfile:14, deploy.sh:56-62), Ruby `workers 0; threads 4,16` (puma.rb:8-11, comment claims "fair… single-process" while compiled langs use all cores), Go/C++/Java/C#/Rust = all cores. | files cited | Pick a policy (1 process per language OR N=cores everywhere OR "idiomatic production default") and document it in the methodology; make PHP consistent with the policy. |
| F4 | **P0** | The one workload CI measures weekly (`/health` via `benchmarks/ci/run-language.sh:131-136`) does different work per language: Java serves a **precomputed static string** (Server.java:418-421); Rust formats an RFC3339 timestamp per request (routes.rs:740); Go/C# allocate + serialize per request. | files cited | Measure a real workload; define `/health` as constant-work (static body everywhere) and exclude it from rankings. |
| F5 | P1 | JSON handling asymmetry: C# and Java hand-roll serialization with StringBuilder and (Java) hand-roll *parsing* with a custom scanner (Program.cs:258-268,439-449; Server.java:598-644,737-830), while Go/Node/Python/Ruby/PHP/Rust use standard serializers. Java's scanner breaks on escaped quotes. | files cited | Policy decision: idiomatic serializer per language (recommended) — replace C#/Java string building with System.Text.Json / a real JSON lib. |
| F6 | P1 | Per-request overhead asymmetries: C# rebuilds the 256-entry CRC32 table per call (Program.cs:569-584); Go re-unmarshals the entire users/corpus arrays per request (main.go:331-338, 527-532) and C# re-walks the JsonDocument, while Node/Python/PHP/Ruby/C++ use pre-parsed in-memory data and Rust slices a cached copy (routes.rs:1090-1112). | files cited | Cache parsed dataset at startup in every language (it's static). |
| F7 | P1 | Blocking sleeps on event-loop/worker threads: C++ `std::this_thread::sleep_for` on io_context threads (server.cpp:968), PHP `usleep` in 4 workers (server.php:460), Ruby thread pool — vs async timers in Go/Node/Python/C#/Rust. `/api/delayed` under concurrency measures thread starvation, not language performance. | files cited | Use async timers (Swoole coroutine sleep, Beast timers) or document `/api/delayed` as a concurrency-model probe. |
| F8 | P1 | Application-mode (plain HTTP behind proxy) supported by Go (main.go:163-197), Node (server.js:91,798-805), Java (Server.java:353-374) only. C# (all variants — `CreateFromPemFile` at startup, Program.cs:41), Python (uvicorn `--ssl-*` unconditional), PHP (`SWOOLE_SSL` constructor, server.php:39-44), Ruby (`bind "ssl://…"`, puma.rb:5) **cannot start without certs** → the documented proxy topology (docs/superpowers/specs/2026-04-03-application-benchmark-mode-design.md) is unrunnable for half the languages. | files cited | Add cert-missing → plain-HTTP fallback to C#, Python, PHP, Ruby, C++ mirroring Go's pattern. |
| F9 | P1 | TLS termination inequality even in direct mode: languages self-terminate TLS with different stacks (OpenSSL/Swoole, rustls, SChannel-less Kestrel, Java SSLEngine) — acceptable *if documented*; currently nothing in docs/testing.md covers it. | docs/testing.md | Document; for app-mode rely on proxy termination (which requires F8). |
| F10 | P1 | Download workload drift: Java streams zeros in 64 KiB chunks (Server.java:434-435) vs 0x42 in 8 KiB chunks elsewhere; nginx serves pre-generated files via sendfile (nginx.conf:52-59, by design as baseline). `browser-benchmark.json` measures `/download/1048576`, so chunking differences are in the measured path. | files cited | Pin fill byte + chunk size in the spec (8 KiB / 0x42). |
| F11 | P1 | Logging asymmetry in production deploys: uvicorn access-log defaults ON via `python/deploy.sh:56-62` (per-request stderr I/O, redirected to `/var/log/python-bench.log`), while nginx sets `access_log off` (nginx.conf:26) and Go/Node/C#/Java log nothing per request. CI disables it (`--log-level error`, ci/run-language.sh:78-81) — so CI and prod measure different Python servers. | files cited | `--no-access-log` in deploy.sh and Dockerfile. |
| F12 | P1 | Python runtime identity confusion: server.py:1 says "hypercorn (HTTP/3 QUIC)", requirements.txt pins `hypercorn[h3]`, but every runner invokes **uvicorn** (Dockerfile:14, deploy.sh:57, ci/run-language.sh:78) which has no HTTP/3 — while AltSvcMiddleware still advertises `h3=":8443"` (server.py:471-490), inviting doomed QUIC upgrades in browser tests. | files cited | Choose hypercorn (h3) or uvicorn (document no-h3 + drop Alt-Svc); align all three run paths. |
| F13 | P1 | C# runtime-ladder variants aren't code-identical, defeating the "only the runtime differs" premise: net8 alone has LOG_FORMAT=json support (net8/Program.cs:49-54); net10-aot hardcodes `/opt/bench` certs ignoring `BENCH_CERT_DIR` (net10-aot diff); net6 has no H3/Alt-Svc (expected) — plus AOT variants use `CreateSlimBuilder` + JSON source-gen for health/upload only. | diffs recorded in audit session | Regenerate all variants from one template with version-gated deltas only. |
| F14 | P2 | Docker base images not normalized: `golang:1.22-alpine`→scratch, `ubuntu:24.04` (cpp), `node:22-slim`, `python:3.12-slim`, `ruby:3.3-slim`, `php:8.3-cli` (debian full), temurin-21. No CPU/memory limits, no healthchecks anywhere in validate compose. | Dockerfiles; docker-compose.yml | Document image policy; add compose healthchecks + optional cpuset pinning for validation-grade runs. |

Positive: bearer-token auth, Server-Timing `app;dur=`, cache-control headers, and the `BENCH_DATA_PATH → /opt/bench → ../shared` lookup order are consistently implemented across all languages, and the orchestrator's statistics layer (bootstrap CIs, Tukey outlier policy, anti-cherry-picking, environment fingerprints — `orchestrator/src/reporter.rs`, `types.rs:146-166`) is genuinely strong.

---

## 3. Validation-suite findings

| # | Sev | Finding | Evidence | Fix |
|---|-----|---------|----------|-----|
| V1 | **P0** | The workflow has **never passed**: 14/14 failures from 2026-04-06 through 2026-07-06 (weekly cron). Latest failure: `target ruby: failed to solve: "bundle install --quiet" exit code 5` — puma's native extension can't build on `ruby:3.3-slim` (no build-essential, no Gemfile.lock; ruby/Dockerfile:6). Because `run-validation.sh` is `set -e` and `docker compose up --build` is all-or-nothing, one broken image aborts validation of **all** languages. | `gh run list --workflow=validate-bench-apis.yml` (14 failures); run 28775632589 log | Fix ruby image (`apt-get install build-essential` or `puma` pure-ruby alternative + commit Gemfile.lock); build images independently and validate the ones that built; alert on failure. |
| V2 | **P0** | Even green, a "pass" proves liveness only: HTTP <400 + parseable JSON + presence of *one* field on 3 of 7 endpoints + 4 headers (run-validation.sh:52-108,164-206). No shape checks, no cross-language equality, no `expected_checksums` verification, `/download` and `/upload` not tested at all, determinism is warn-only (line 140-146), auth only tested when `--token` passed. | run-validation.sh | Add JSON-schema check per endpoint + byte-exact `/download/{size}` check + cross-language diff of `/api/validate` output against `expected_checksums`. |
| V3 | P1 | The one field assertion is wrong for the majority shape: `check … "mean"` asserts top-level `mean` (line 172, 93-102), but family B nests it under `stats` → Node/Python/PHP/Ruby/C++ `/api/aggregate` would FAIL once the images build. The suite has never run far enough to notice. | run-validation.sh:93-103,172 | Fix after F1 defines the canonical shape. |
| V4 | P1 | Coverage 8/17: only go, python, nodejs, java, ruby, php, cpp, csharp-net8 (docker-compose.yml; run-validation.sh:254-268). Rust is validated only via manual `--rust-only`; csharp-net6/7/9/9-aot/10/10-aot/48 and nginx never validated. | files cited | Add remaining images (or delete dead variants, C4). |
| V5 | P1 | Not per-PR: schedule (Mon 6am) + manual only (validate-bench-apis.yml:6-9); no path filter exists, so reference-api changes merge unvalidated. | workflow | Add `pull_request: paths: [benchmarks/reference-apis/**, benchmarks/validate/**]` trigger. |
| V6 | P1 | bench-data.json never reaches validation containers (compose mounts certs volume over `/opt/bench`; build contexts exclude `../shared`) → the shared-data code paths (the ones that break, F2) are never exercised. | docker-compose.yml:18-106 | Mount `../reference-apis/shared/bench-data.json:/opt/bench/bench-data.json:ro`. |
| V7 | P1 | Broken images beyond ruby (need runtime confirm, static evidence strong): **Python** CMD runs uvicorn, never installed (Dockerfile:14 vs requirements.txt:1); **Java** jar packs only Health/Download/Upload handler classes — `Server$APIUsersHandler` etc. missing → NoClassDefFoundError in `main` (java/Dockerfile:4-8); **Go** `COPY go.mod main.go` without go.sum → `go build` fails with quic-go dependency (go/Dockerfile:3). Experiment: `docker compose -f benchmarks/validate/docker-compose.yml build`. | files cited | Add uvicorn to requirements (or switch CMD to hypercorn); `jar cfe server.jar Server *.class`; COPY go.sum. |
| V8 | P2 | Flakiness traps: fixed host ports 8501-8508, fixed `sleep 10` readiness (run-validation.sh:251), `--wait` without healthchecks, `declare -A` (bash 4+; macOS default bash can't run it locally). | run-validation.sh:239-268 | Healthcheck-based wait + retry loop; dynamic ports; bash-3.2-safe arrays. |
| V9 | P2 | Nothing validates the orchestrator's measurement methodology or metrics-agent (no golden tests for warmup discard, percentile math beyond unit level; Chrome harness has `golden-run-invariants.json` but no CI hook). Chrome harness defaults are publication-thin (10 measured cycles, concurrency hardcoded 10 — chrome-harness/runner.js:58-60). | orchestrator/reporter.rs; chrome-harness/runner.js | Add reporter/percentile golden tests to `cargo test`; raise Chrome defaults or gate publication on config. |

---

## 4. Coverage vs product-promise findings

| # | Sev | Finding | Evidence | Fix |
|---|-----|---------|----------|-----|
| C1 | **P0** | **No runner measures the `/api/*` JSON workloads at all.** The orchestrator validates/measures `/health` + `/download/{size}` (validator.rs, runner.rs:384); weekly `benchmark.yml` targets `/health` (ci/run-language.sh:131); Application Benchmark UI presets are `http1,http2,http3,download,upload` (dashboard AppBenchmarkPage); the `Mode` enum (crates/networker-common/src/test_config.rs:169-188) has no API-workload mode; grep shows no client anywhere hits `/api/users`. The differentiating "per-request computation" promise of the app-benchmark spec (docs/superpowers/specs/2026-04-03…:57-67) is unmeasured — all 7 endpoints × 17 languages are dead weight for results. | cited | Add an `apibench` mode (or workload param) to networker-tester + orchestrator + UI preset, driving the canonical `/api/*` suite. |
| C2 | **P0** | The default baseline language cannot complete a run: Rust (= networker-endpoint) 404s `deployer::validate_api`'s `/download/1024` (deployer.rs:692 vs routes.rs:786-795) and fails `check_health` (`runtime` missing); Java fails `check_health` (`version` missing). benchmark.yml defaults include rust. The committed results file `benchmarks/results-78cc2352….json` has `results: []`. Experiment: `alethabench validate --ip <endpoint-vm> --language rust`. | cited | Add `/download/{size}` route + `runtime` field to networker-endpoint (and the C# port `src/Networker.Endpoint/Endpoints.cs:41`); add `version` to Java health. |
| C3 | P1 | C# control-plane language detection is a stub: `BenchmarkCatalogEndpoints.cs:135-169` returns 202 with a TODO (log: "SSH probe is not yet implemented"), vs the working Rust probe (`crates/networker-dashboard/src/api/benchmark_catalog.rs:227-292`) that detects all `/opt/bench/*` installs including `csharp-net*` variants. Post-cutover (phase3 port complete per project state), the Application Benchmark wizard's language catalog stops updating. | cited | Port `ssh_detect_languages` before the user-facing flip. |
| C4 | P1 | Dead implementations: `csharp-net6` and `csharp-net7` (both EOL runtimes) exist in the repo and are detected by the Rust SSH probe, but are **absent from the UI catalog** (`dashboard/src/components/wizard/testbed-constants.ts:169-206` lists net48/net8/net8-aot/net9/net9-aot/net10/net10-aot only) and absent from validation. They can appear in a VM's detected-language list yet be unselectable. | cited | Delete net6/net7 (keep on a tag) or add them to the UI + validation. |
| C5 | P1 | UI language list (17) vs contract reality: nginx (always-included baseline) serves only `/health`, `/download`, `/upload` (nginx.conf) — fine for download modes, but any future `/api` workload (C1 fix) 404s on it; Java is HTTP/1.1-only (`com.sun.net.httpserver`), so direct-mode `http2`/`http3` selections against Java can't measure what the UI implies (proxy-fronted is fine). No capability matrix exists to gate mode × language combos in the wizard. | cited | Add per-language capability metadata (h2/h3/api-suite) consumed by the wizard and the orchestrator skip logic. |
| C6 | P2 | `benchmarks/sample-benchmark.json` is rot: references `implementations/rust-axum`, `go-gin`, ports 3001+, endpoint `/api/health` — none exist in the repo. | sample-benchmark.json | Regenerate against reference-apis or delete. |
| C7 | P2 | Committed build artifacts pollute the estate: `csharp-net10/publish/**` (full self-contained runtime, hundreds of files), `obj/`/`bin/` trees, `python/__pycache__`, per-language `output/run-*.json` reports inside reference dirs. | file listing | .gitignore + purge. |
| C8 | P2 | csharp-net48 has no Dockerfile/README (Windows-only HttpListener, deploy.sh only) and its `/download` route is exact-match `case "/download"` (Server.cs:279) — path-style `/download/{size}` support needs runtime confirmation on a Windows VM. | Server.cs:276-285 | Verify on Windows testbed; document Windows-only status in reference-apis README. |
| C9 | P2 | `/api/validate` never validates: every language echoes the pre-computed file checksums instead of recomputing from its own output (e.g. Program.cs:513-522 returns the raw JSON block). A language could return garbage from `/api/users` and still "pass" validate. | cited | Recompute checksums from live endpoint output server-side, or have the validator fetch `/api/users?sort=name` etc. and hash client-side against `expected_checksums`. |

---

## 5. Prioritized TODO

### P0 — comparison numbers are misleading today
1. **Freeze a written API contract** (`benchmarks/shared/API-SPEC.md`): one shape family, param defaults/clamps, error codes, fill byte + chunk size, compression algo+level, `/health` fields. Port all 17 implementations to it. (F1)
2. **Fix shared-dataset parsing** in Go, C#, Node, Python (users schema + timeseries objects); make dataset-load failures fatal, not silent fallback. (F2)
3. **Make the Rust baseline pass its own orchestrator**: add `/download/{size}` and `runtime` in `/health` to networker-endpoint + C# endpoint port; add `version` to Java health. Re-run `alethabench validate` for all languages. (C2)
4. **Revive validation**: fix ruby/python/java/go images, per-image build isolation, PR path-trigger, mount bench-data.json, and make the workflow's 14-week failure streak impossible to ignore (required check or alert). (V1, V5, V6, V7)
5. **Normalize and document worker policy** (PHP=4 vs single-process scripting langs vs all-core compiled). (F3)
6. **Stop ranking on `/health`**; define it constant-work and point CI at a real workload. (F4)
7. **Wire the `/api/*` workloads into an actual runner** (apibench mode) or descope the app-benchmark compute promise. (C1)

### P1 — trust and reproducibility
8. Content-verifying validation: JSON schema per endpoint, byte-exact downloads, cross-language checksum diff (V2, V3, C9).
9. Plain-HTTP fallback for C#, Python, PHP, Ruby, C++ (application mode). (F8)
10. Resolve Python uvicorn/hypercorn identity; kill false Alt-Svc h3; disable access logs in deploy. (F11, F12)
11. Idiomatic JSON serializers in C#/Java; cache parsed datasets; hoist C# CRC table. (F5, F6)
12. Async sleeps in C++/PHP or document `/api/delayed` semantics; clamp Python `ms` and `limit`. (F7, and drift table)
13. Port `ssh_detect_languages` to the C# control plane before cutover. (C3)
14. Language-capability matrix consumed by the wizard (h2/h3/api-suite; Java h1-only; nginx no-api). (C5)
15. Regenerate C# runtime-ladder variants from one template; validate all variants weekly. (F13, V4)

### P2 — polish
16. Delete or resurrect csharp-net6/net7; document net48 Windows-only; verify its `/download` path handling. (C4, C8)
17. Purge committed build artifacts + stale outputs; fix `sample-benchmark.json`. (C6, C7)
18. Compose healthchecks, dynamic ports, bash-3.2-safe validate script; add reporter/percentile golden tests; raise Chrome-harness sample defaults. (V8, V9)
19. Pin/document Docker image policy and optional resource limits. (F14)

### Runtime-confirmation experiments
- `docker compose -f benchmarks/validate/docker-compose.yml build` — confirms V7 (python/java/go/ruby image breakage).
- Deploy bench-data.json + `curl /api/aggregate?range=1,100` per language — confirms F2 (Python 500, Node NaN, Go zeros, C# fallback).
- `alethabench validate --ip <vm> --language rust|java` — confirms C2.
- Windows testbed: `curl https://…/download/1024` against csharp-net48 — confirms C8.
