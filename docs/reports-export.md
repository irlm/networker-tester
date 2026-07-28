# Report export (documents)

Turns test results into branded, downloadable documents. Lives in
`src/Networker.ControlPlane/Reports/Documents/`.

## Shape

A format-agnostic **`ReportDocument`** (title, subtitle, header meta, sections of
typed blocks) is the seam. Two sides plug into it and never know about each
other:

- **Builders** map a domain report → `ReportDocument`:
  - `RunReportDocument` — one test run: KPI summary, a per-protocol latency
    **distribution box-plot** (`CandleBlock`) + table, median phase timings
    (DNS/TCP/TLS/TTFB/server), and a capped per-attempt table.
  - `AppNetworkReportDocument` / `PerfPerCostReportDocument` — the existing
    analysis reports (they add no new statistics; they render the wire report).
- **Exporters** (`IReportExporter`) render the document → bytes:
  - `MarkdownReportExporter` — GitHub-flavoured Markdown (unicode block/box-plot
    charts).
  - `HtmlReportExporter` — one self-contained file: dark brand header with the
    inline SVG wordmark, light data-dense body, inline SVG charts, no external
    requests.
  - `DocxReportExporter` — editable Word `.docx` via DocumentFormat.OpenXml
    (pure-managed, no native dependency).
  - PDF ships next, behind its own package (QuestPDF); it reuses the same model.

Adding a new source report or a new output format is a one-file change.

## Blocks

`ProseBlock`, `CalloutBlock` (toned verdict), `MetricsBlock` (KPI strip),
`TableBlock`, `ChartBlock` (single-value bars) and `CandleBlock` (a distribution
box-plot: min→max whisker, p25→p75 box, median tick, dashed p95 tail marker —
the right way to show latency, where the tail is what you diagnose). Charts have
no image pipeline: HTML draws SVG, Markdown/DOCX draw a monospace box-plot
(`CandleAscii`).

Brand tone comes from `ReportBranding` / `ReportTone`; the purple is
wordmark-only (never a status colour, audit F12). Logo assets:
`assets/brand/laghound-{glyph,wordmark}.svg`, embedded into the assembly.

## HTTP

`?format=` selects the document; omit it (or `?format=json`) for the existing
JSON. RBAC and the JSON wire shapes are unchanged.

| Route | Formats |
|-------|---------|
| `GET /api/projects/{projectId}/reports/app-network?format=` | `json` (default), `md`, `html`, `docx` |
| `GET /api/projects/{projectId}/reports/perf-per-cost?format=` | `json` (default), `md`, `html`, `docx` |
| `GET /api/v2/test-runs/{id}/report?format=` | `html` (default), `md`, `docx` — no JSON (use `/attempts`) |

An unknown/unavailable format returns `400` naming the supported document
formats. Documents download as `Content-Disposition: attachment`.
