/**
 * "Open in infra-scorecard": hand an application-benchmark comparison to
 * infra-scorecard, which prices what each language/runtime would cost to run
 * a real estate on. The run travels in the URL fragment
 * (`#laghound=<base64url JSON>`), so it never reaches a server: the
 * scorecard is a static page and needs no access to this control plane.
 *
 * The payload is the subset of the orchestrator's BenchmarkRun the scorecard
 * reads (language, scenario, server cores, rps, p50/p99). The dashboard's
 * artifacts carry no CPU share or peak memory, so the scorecard compares
 * throughput per server core and leaves memory out.
 */
import { normalizeHttpUrl } from '../../lib/url';
import type { BenchmarkSummary } from '../../api/types';
import type { CellResult } from './compare';

/** Where infra-scorecard is deployed (build-time `VITE_INFRA_SCORECARD_URL`); undefined hides the button. */
export function scorecardBaseUrl(): string | undefined {
  return normalizeHttpUrl(import.meta.env.VITE_INFRA_SCORECARD_URL);
}

export interface ScorecardResult {
  language: string;
  scenario?: string;
  environment: { server_cpu_cores?: number };
  network: { rps: number; latency_p50_ms?: number; latency_p99_ms?: number };
}

export interface ScorecardRun {
  id: string;
  started_at?: string;
  results: ScorecardResult[];
}

/** The busiest summary of an artifact: the case the scorecard sizes servers on. */
function busiest(summaries: readonly BenchmarkSummary[] | undefined): BenchmarkSummary | undefined {
  let best: BenchmarkSummary | undefined;
  for (const s of summaries ?? []) if (s.rps > 0 && (!best || s.rps > best.rps)) best = s;
  return best;
}

/**
 * One result per language cell that finished with an artifact. Undefined
 * when fewer than two languages have one: a comparison needs two.
 */
export function scorecardRun(cells: readonly CellResult[], id: string): ScorecardRun | undefined {
  const results: ScorecardResult[] = [];
  let startedAt: string | undefined;
  for (const cell of cells) {
    const language = cell.meta.language;
    const art = cell.artifact;
    if (!language || !art) continue;
    const best = busiest(art.summaries);
    if (!best) continue;
    // The server may omit fields the type declares; read them defensively.
    const cores = art.environment?.server_info?.cpu_cores;
    const scenario = art.methodology?.scenario;
    results.push({
      language,
      ...(scenario ? { scenario } : {}),
      environment: cores ? { server_cpu_cores: cores } : {},
      network: {
        rps: best.rps,
        ...(best.latency_p50_ms != null ? { latency_p50_ms: best.latency_p50_ms } : {}),
        ...(best.latency_p99_ms != null ? { latency_p99_ms: best.latency_p99_ms } : {}),
      },
    });
    startedAt ??= cell.run.started_at ?? undefined;
  }
  if (new Set(results.map((r) => r.language)).size < 2) return undefined;
  return { id, ...(startedAt ? { started_at: startedAt } : {}), results };
}

/** base64url (no padding) of the UTF-8 JSON, as infra-scorecard reads it. */
export function encodeRun(run: ScorecardRun): string {
  const bytes = new TextEncoder().encode(JSON.stringify(run));
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function scorecardHref(base: string, run: ScorecardRun): string {
  return `${base.replace(/#.*$/, '')}#laghound=${encodeRun(run)}`;
}
