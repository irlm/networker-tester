# csharp-template — one template, nine variants

Single source of truth for the nine-variant C# runtime ladder
(`benchmarks/shared/API-SPEC.md` §8). A change to the API **cannot** be made
in one variant only: every variant directory is generated from here, and
`generate-variants.py --check` fails when any committed copy drifts.

## Layout

| File | Role |
|---|---|
| `Program.cs` | The modern template. Copied **byte-identical** into all eight `csharp-net6` … `csharp-net10-aot` directories. Version-gated deltas live in `#if NET*_OR_GREATER` blocks (HTTP/3 on net7+, `CreateSlimBuilder` on net8+, `X509CertificateLoader` on net9+). The `/health` runtime id comes from the per-variant `<AssemblyName>`, so the source needs no per-variant edits. |
| `Server.cs` | The net48 template (copied into `csharp-net48/`). .NET Framework has no Kestrel/minimal APIs, no System.Text.Json, no ZLibStream — so net48 is the **one** documented divergent implementation: HttpListener + JavaScriptSerializer + hand-framed RFC 1950 zlib, C# 5, Windows-only. |
| `generate-variants.py` | Emits, per variant: `Program.cs`/`Server.cs` copy, `.csproj` (TFM + AssemblyName + `PublishAot` only), `Dockerfile`, `build.sh`, `deploy.sh`, `README.md`, `.gitignore`. `--check` regenerates into a temp dir and diffs against the tree. |

## The ladder

| Variant | TFM | Compilation | HTTP/3 |
|---|---|---|---|
| csharp-net6 | net6.0 | JIT | no (no stable Kestrel H3 on net6) |
| csharp-net7 | net7.0 | JIT | yes |
| csharp-net8 | net8.0 | JIT | yes |
| csharp-net8-aot | net8.0 | Native AOT | yes |
| csharp-net9 | net9.0 | JIT | yes |
| csharp-net9-aot | net9.0 | Native AOT | yes |
| csharp-net10 | net10.0 | JIT | yes |
| csharp-net10-aot | net10.0 | Native AOT | yes |
| csharp-net48 | .NET Framework 4.8 | JIT (Windows) | no (HTTP/1.1 only) |

## Rules

- **Never edit a `csharp-*` directory by hand.** Edit the template, run
  `./generate-variants.py`, commit the regenerated tree.
- `Program.cs` must stay **C# 10 compatible** — the net6 SDK Docker image
  carries the oldest Roslyn in the ladder.
- `Program.cs` must stay **Native AOT safe** — System.Text.Json source
  generation only (`BenchJson`), no reflection serialization.
- Integral doubles are serialized as `N.0` (`PyFloatConverter` / net48
  `Json.Double`): the §7 canonical-JSON validators round-trip responses
  through Python, which distinguishes int `39` from float `39.0` — and the
  frozen dataset's aggregate response really contains an exact `39.0`.
- Dataset load failures are fatal (§2). There is no PRNG fallback anywhere.

## Validating a variant

```bash
python3 generate-variants.py --check
docker build -t bench-csharp-net8 ../csharp-net8
# then boot with certs + bench-data.json mounted at /opt/bench and run:
../../validate/run-validation.sh --url=https://localhost:8443 --name=csharp-net8
```
