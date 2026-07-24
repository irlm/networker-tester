// ── Scenario / preset catalog ────────────────────────────────────────────────
//
// The "start from a scenario" launcher: instead of knowing which of the four
// test types to open and hand-picking modes, the user picks an *outcome* and we
// drop them into the right existing flow, pre-filled. Each scenario maps to a
// target page + query params those pages already read (DiagnosticsPage `?preset`,
// NetworkTestPage `?modes`, FullStackPage `?modes`, AppBenchmarkPage `?template`).
//
// This is config-only prefill — it reuses the four tested wizards and the
// capability gate (mode-capabilities.ts); it never provisions on its own. The
// user can tweak everything before launching. Auto-provisioning target+runner
// as part of a scenario is a documented Phase 2.
//
// INVARIANT (guarded by scenarios.test.ts): every scenario's `modes` must be
// runnable against its `flow`'s target — e.g. a `url` scenario may only carry
// `any`-requirement modes, or it would 422 at create (Phase 2 enforcement).

/** Which existing flow a scenario lands in, and the capability context. */
export type ScenarioFlow =
  | 'url' //                 DiagnosticsPage — a raw URL (gate kind 'url')
  | 'endpoint' //            NetworkTestPage — an existing deployed endpoint (gate kind 'endpoint')
  | 'provision-endpoint' //  FullStackPage — provisions an endpoint (gate kind 'endpoint')
  | 'provision-app'; //      AppBenchmarkPage — provisions a language stack (apibench; pending fail-open)

export interface Scenario {
  id: string;
  title: string;
  /** One-line "what you get". */
  summary: string;
  /** Chips: the concrete things this scenario measures. */
  measures: string[];
  flow: ScenarioFlow;
  /** Short label shown as the card's test-type badge. */
  badge: string;
  /** What infrastructure the user needs before this can run. */
  needs: string;
  /** Rough wall-clock estimate. */
  est: string;
  /** The modes this scenario runs — for display and capability validation. */
  modes: string[];
  /**
   * For `url` scenarios: the DiagnosticsPage preset id. For `provision-app`:
   * the RUNTIME_TEMPLATES id. Undefined for the `?modes`-driven flows.
   */
  presetId?: string;
  /** Destination path + query for the given project. */
  href: (projectId: string) => string;
}

const probe = (pid: string, preset: string) =>
  `/projects/${pid}/probe?preset=${preset}`;
const network = (pid: string, modes: string[]) =>
  `/projects/${pid}/tests/new?modes=${modes.join(',')}`;
const fullStack = (pid: string, modes: string[]) =>
  `/projects/${pid}/benchmarks/full-stack/new?modes=${modes.join(',')}`;
const application = (pid: string, template: string) =>
  `/projects/${pid}/benchmarks/application/new?template=${template}`;

// Mode sets mirror DiagnosticsPage.DIAG_PRESETS so the card preview matches what
// the probe page selects. Kept in lockstep by scenarios.test.ts (all `any`).
const URL_QUICK = ['dns', 'tcp', 'tls', 'http2'];
const URL_STANDARD = ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp'];
const URL_FULL = [...URL_STANDARD, 'curl', 'pageload', 'pageload2', 'pageload3', 'browser1', 'browser2', 'browser3'];

export interface ScenarioGroup {
  id: string;
  label: string;
  blurb: string;
  scenarios: Scenario[];
}

export const SCENARIO_GROUPS: ScenarioGroup[] = [
  {
    id: 'diagnose',
    label: 'Diagnose a URL',
    blurb: 'Point at any reachable URL — no infrastructure to set up.',
    scenarios: [
      {
        id: 'url-quick',
        title: 'Quick latency & TLS check',
        summary: 'The fast health read on any host — DNS, connect, handshake, first byte.',
        measures: ['DNS resolve', 'TCP connect', 'TLS handshake', 'HTTP/2 TTFB'],
        flow: 'url',
        badge: 'URL Probe',
        needs: 'Any reachable URL',
        est: '~3s',
        modes: URL_QUICK,
        presetId: 'quick',
        href: (pid) => probe(pid, 'quick'),
      },
      {
        id: 'url-protocols',
        title: 'Protocol & handshake deep-dive',
        summary: 'Every network + HTTP layer isolated — compare protocol versions and TLS behavior.',
        measures: ['HTTP/1·2·3', 'TLS resume', 'Native OS TLS', 'UDP RTT & jitter'],
        flow: 'url',
        badge: 'URL Probe',
        needs: 'Any reachable URL',
        est: '~15s',
        modes: URL_STANDARD,
        presetId: 'standard',
        href: (pid) => probe(pid, 'standard'),
      },
      {
        id: 'url-pageload',
        title: 'Real page-load experience',
        summary: 'What a browser actually experiences — parallel fetch + headless Chrome render.',
        measures: ['H1/H2/H3 page load', 'Chrome DOM + load', 'Bytes transferred'],
        flow: 'url',
        badge: 'URL Probe',
        needs: 'Any reachable URL',
        est: '~60s',
        modes: URL_FULL,
        presetId: 'full',
        href: (pid) => probe(pid, 'full'),
      },
    ],
  },
  {
    id: 'measure',
    label: 'Measure your endpoint',
    blurb: 'Run against a deployed networker-endpoint you already have.',
    scenarios: [
      {
        id: 'endpoint-throughput',
        title: 'Throughput to your endpoint',
        summary: 'Sustained bandwidth both directions against the endpoint’s transfer servers.',
        measures: ['Download Mbps', 'Upload Mbps'],
        flow: 'endpoint',
        badge: 'Network',
        needs: 'A deployed endpoint',
        est: '~30s',
        modes: ['download', 'upload'],
        href: (pid) => network(pid, ['download', 'upload']),
      },
      {
        id: 'endpoint-http-versions',
        title: 'HTTP/1 vs 2 vs 3 head-to-head',
        summary: 'Per-version request timing against the same endpoint — see where QUIC wins.',
        measures: ['H1 TTFB & total', 'H2 multiplexed', 'H3 / QUIC'],
        flow: 'endpoint',
        badge: 'Network',
        needs: 'A deployed endpoint',
        est: '~20s',
        modes: ['http1', 'http2', 'http3'],
        href: (pid) => network(pid, ['http1', 'http2', 'http3']),
      },
    ],
  },
  {
    id: 'benchmark',
    label: 'Benchmark & compare',
    blurb: 'Provision fresh infrastructure and compare across a matrix. Needs a cloud account.',
    scenarios: [
      {
        id: 'full-stack-compare',
        title: 'Full-stack proxy comparison',
        summary: 'Protocol + throughput across proxy stacks on freshly provisioned VMs.',
        measures: ['H1/H2/H3', 'Download & upload', 'Per-proxy matrix'],
        flow: 'provision-endpoint',
        badge: 'Full Stack',
        needs: 'A cloud account (auto-provisioned)',
        est: 'minutes',
        modes: ['http1', 'http2', 'http3', 'download', 'upload'],
        href: (pid) => fullStack(pid, ['http1', 'http2', 'http3', 'download', 'upload']),
      },
      {
        id: 'app-language-stack',
        title: 'Language stack benchmark (Linux)',
        summary: 'HTTP + throughput across language runtimes on one provisioned Linux testbed.',
        measures: ['6 language runtimes', 'H1/H2/H3', 'Download & upload'],
        flow: 'provision-app',
        badge: 'Application',
        needs: 'A cloud account (auto-provisioned)',
        est: 'minutes',
        modes: ['http1', 'http2', 'http3', 'download', 'upload'],
        presetId: 'linux-api-stack',
        href: (pid) => application(pid, 'linux-api-stack'),
      },
      {
        id: 'app-api-compute',
        title: 'API compute benchmark',
        summary: 'Per-request server compute (sort / hash / aggregate / search / compress) across languages.',
        measures: ['apibench workload suite', 'Compute-bound TTFB', 'Per-language matrix'],
        flow: 'provision-app',
        badge: 'Application',
        needs: 'A cloud account (auto-provisioned)',
        est: 'minutes',
        modes: ['http1', 'apibench'],
        presetId: 'api-compute',
        href: (pid) => application(pid, 'api-compute'),
      },
    ],
  },
];

/** Flat list of all scenarios, for lookups + tests. */
export const ALL_SCENARIOS: Scenario[] = SCENARIO_GROUPS.flatMap((g) => g.scenarios);
