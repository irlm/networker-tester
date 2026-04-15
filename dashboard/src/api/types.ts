export interface Agent {
  agent_id: string;
  name: string;
  region: string | null;
  provider: string | null;
  status: string;
  version: string | null;
  os: string | null;
  arch: string | null;
  last_heartbeat: string | null;
  registered_at: string;
  tags: Record<string, string> | null;
  /** FK to `project_tester`. NULL means the tester was deleted and the
   * agent row is orphaned — dashboard UI should hide these unless they
   * happen to be online right now. */
  tester_id: string | null;
}

export interface Job {
  job_id: string;
  definition_id: string | null;
  agent_id: string | null;
  status: string;
  config: JobConfig;
  created_by: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  run_id: string | null;
  tls_profile_run_id: string | null;
  error_message: string | null;
}

export interface JobConfig {
  target: string;
  modes: string[];
  project_id?: string;
  tls_profile_url?: string;
  tls_profile_ip?: string;
  tls_profile_sni?: string;
  tls_profile_target_kind?: 'managed-endpoint' | 'external-url' | 'external-host';
  runs: number;
  concurrency: number;
  timeout_secs: number;
  payload_sizes: string[];
  insecure: boolean;
  dns_enabled: boolean;
  connection_reuse: boolean;
  capture_mode?: 'none' | 'tester' | 'endpoint' | 'both';
}

export interface RunSummary {
  run_id: string;
  started_at: string;
  finished_at: string | null;
  target_url: string;
  target_host: string;
  modes: string;
  total_runs: number;
  success_count: number;
  failure_count: number;
}

export interface BenchmarkRunSummary {
  run_id: string;
  generated_at: string;
  target_url: string;
  target_host: string;
  modes: string[];
  concurrency: number;
  total_runs: number;
  contract_version: string;
  scenario: string;
  primary_phase: string;
  phase_model: string;
  execution_plan_source: string | null;
  server_region: string | null;
  network_type: string | null;
  baseline_rtt_p50_ms: number | null;
  total_cases: number;
  total_samples: number;
  publication_ready: boolean;
  noise_level: string;
  sufficiency: string;
  publication_blocker_count: number;
  warnings: string[];
}

export interface BenchmarkComparePresetFilters {
  targetSearch: string;
  scenario: string;
  phaseModel: string;
  serverRegion: string;
  networkType: string;
}

export interface BenchmarkComparePresetInput {
  id?: string;
  name: string;
  runIds: string[];
  baselineRunId: string | null;
  filters?: BenchmarkComparePresetFilters;
}

export interface BenchmarkComparePreset {
  id: string;
  name: string;
  createdAt: string;
  updatedAt: string;
  runIds: string[];
  baselineRunId: string | null;
  filters?: BenchmarkComparePresetFilters;
}

export interface BenchmarkHostInfo {
  os: string;
  arch: string;
  cpu_cores: number;
  total_memory_mb?: number | null;
  os_version?: string | null;
  hostname?: string | null;
  server_version?: string | null;
  uptime_secs?: number | null;
  region?: string | null;
}

export interface BenchmarkNetworkBaseline {
  samples: number;
  rtt_min_ms: number;
  rtt_avg_ms: number;
  rtt_max_ms: number;
  rtt_p50_ms: number;
  rtt_p95_ms: number;
  network_type: string;
}

export interface BenchmarkEnvironmentCheck {
  attempted_samples: number;
  successful_samples: number;
  failed_samples: number;
  duration_ms: number;
  rtt_min_ms: number;
  rtt_avg_ms: number;
  rtt_max_ms: number;
  rtt_p50_ms: number;
  rtt_p95_ms: number;
  packet_loss_percent: number;
  network_type: string;
}

export interface BenchmarkStabilityCheck extends BenchmarkEnvironmentCheck {
  jitter_ms: number;
}

export interface BenchmarkExecutionPlan {
  source: string;
  min_samples: number;
  max_samples: number;
  min_duration_ms: number;
  target_relative_error?: number | null;
  target_absolute_error?: number | null;
  pilot_sample_count: number;
  pilot_elapsed_ms?: number | null;
}

export interface BenchmarkNoiseThresholds {
  max_packet_loss_percent: number;
  max_jitter_ratio: number;
  max_rtt_spread_ratio: number;
}

export interface BenchmarkMetadata {
  contract_version: string;
  generated_at: string;
  run_id: string;
  source: string;
  target_url: string;
  target_host: string;
  modes: string[];
  total_runs: number;
  concurrency: number;
  timeout_ms: number;
  client_os: string;
  client_version: string;
}

export interface BenchmarkEnvironment {
  client_info: BenchmarkHostInfo | null;
  server_info: BenchmarkHostInfo | null;
  network_baseline: BenchmarkNetworkBaseline | null;
  environment_check?: BenchmarkEnvironmentCheck | null;
  stability_check?: BenchmarkStabilityCheck | null;
  packet_capture_enabled: boolean;
}

export interface BenchmarkMethodology {
  mode: string;
  phase_model: string;
  sample_phase: string;
  scenario: string;
  launch_count: number;
  phases_present: string[];
  retries_recorded: boolean;
  higher_is_better_depends_on_workload: boolean;
  confidence_level?: number;
  outlier_policy?: string;
  uncertainty_method?: string;
  execution_plan?: BenchmarkExecutionPlan | null;
  noise_thresholds?: BenchmarkNoiseThresholds | null;
}

export interface BenchmarkLaunch {
  launch_index: number;
  scenario: string;
  primary_phase: string;
  started_at: string;
  finished_at: string | null;
  phases_present: string[];
  sample_count: number;
  primary_sample_count: number;
  warmup_sample_count: number;
  success_count: number;
  failure_count: number;
}

export interface BenchmarkCase {
  id: string;
  protocol: string;
  payload_bytes: number | null;
  http_stack: string | null;
  metric_name: string;
  metric_unit: string;
  higher_is_better: boolean;
}

export interface BenchmarkSample {
  attempt_id: string;
  case_id: string;
  launch_index: number;
  phase: string;
  iteration_index: number;
  success: boolean;
  retry_count: number;
  inclusion_status: string;
  metric_value: number | null;
  metric_unit: string;
  started_at: string;
  finished_at: string | null;
  total_duration_ms: number | null;
  ttfb_ms: number | null;
}

export interface BenchmarkSummary {
  case_id: string;
  protocol: string;
  payload_bytes: number | null;
  http_stack: string | null;
  metric_name: string;
  metric_unit: string;
  higher_is_better: boolean;
  sample_count: number;
  included_sample_count: number;
  excluded_sample_count: number;
  success_count: number;
  failure_count: number;
  total_requests: number;
  error_count: number;
  bytes_transferred: number;
  wall_time_ms: number;
  rps: number;
  min: number;
  mean: number;
  p5: number;
  p25: number;
  p50: number;
  p75: number;
  p95: number;
  p99: number;
  p999: number;
  max: number;
  stddev: number;
  latency_mean_ms: number | null;
  latency_p50_ms: number | null;
  latency_p99_ms: number | null;
  latency_p999_ms: number | null;
  latency_max_ms: number | null;
}

export interface BenchmarkDataQuality {
  noise_level: string;
  sample_stability_cv: number;
  sufficiency: string;
  warnings: string[];
  publication_ready: boolean;
  confidence_level?: number;
  outlier_policy?: string;
  uncertainty_method?: string;
  relative_margin_of_error?: number;
  quality_tier?: string;
  low_outlier_count?: number;
  high_outlier_count?: number;
  outlier_count?: number;
  publication_blockers?: string[];
}

export interface BenchmarkDiagnostics {
  raw_attempt_count: number;
  raw_success_count: number;
  raw_failure_count: number;
}

export interface BenchmarkArtifact {
  metadata: BenchmarkMetadata;
  environment: BenchmarkEnvironment;
  methodology: BenchmarkMethodology;
  launches: BenchmarkLaunch[];
  cases: BenchmarkCase[];
  samples: BenchmarkSample[];
  summaries: BenchmarkSummary[];
  comparisons: unknown[];
  data_quality: BenchmarkDataQuality;
  diagnostics: BenchmarkDiagnostics;
  summary: BenchmarkSummary;
}

export interface BenchmarkDistributionStats {
  sample_count: number;
  min: number;
  mean: number;
  median: number;
  p95: number;
  p99: number;
  max: number;
  stddev: number;
  cv: number;
  standard_error: number;
  ci95_lower: number;
  ci95_upper: number;
}

export interface ComparedBenchmarkRun {
  run_id: string;
  generated_at: string;
  target_host: string;
  scenario: string;
  primary_phase: string;
  phase_model: string;
  publication_ready: boolean;
  noise_level: string;
  sufficiency: string;
  warning_count: number;
  environment: BenchmarkEnvironmentFingerprintView;
}

export interface BenchmarkCaseRunView {
  run_id: string;
  generated_at: string;
  target_host: string;
  scenario: string;
  primary_phase: string;
  phase_model: string;
  publication_ready: boolean;
  noise_level: string;
  sufficiency: string;
  warning_count: number;
  included_sample_count: number;
  failure_count: number;
  error_count: number;
  rps: number;
  p95: number;
  p99: number;
  environment: BenchmarkEnvironmentFingerprintView;
  distribution: BenchmarkDistributionStats;
}

export interface BenchmarkEnvironmentFingerprintView {
  client_os: string | null;
  client_arch: string | null;
  client_cpu_cores: number | null;
  client_region: string | null;
  server_os: string | null;
  server_arch: string | null;
  server_cpu_cores: number | null;
  server_region: string | null;
  network_type: string | null;
  baseline_rtt_p50_ms: number | null;
  baseline_rtt_p95_ms: number | null;
}

export interface BenchmarkCaseCandidateComparison {
  run: BenchmarkCaseRunView;
  comparable: boolean;
  comparability_notes: string[];
  absolute_delta: number | null;
  percent_delta: number | null;
  ratio: number | null;
  verdict: string;
}

export interface BenchmarkCaseComparison {
  case_id: string;
  protocol: string;
  payload_bytes: number | null;
  http_stack: string | null;
  metric_name: string;
  metric_unit: string;
  higher_is_better: boolean;
  baseline: BenchmarkCaseRunView;
  candidates: BenchmarkCaseCandidateComparison[];
}

export interface BenchmarkComparisonReport {
  baseline_run_id: string;
  comparability_policy: string;
  gated_candidate_count: number;
  runs: ComparedBenchmarkRun[];
  cases: BenchmarkCaseComparison[];
}

export interface Attempt {
  attempt_id: string;
  protocol: string;
  sequence_num: number;
  started_at: string;
  finished_at: string | null;
  success: boolean;
  error_message: string | null;
  retry_count: number;
}

export interface Deployment {
  deployment_id: string;
  name: string;
  status: string;
  config: DeployConfig;
  provider_summary: string | null;
  created_by: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  endpoint_ips: string[] | null;
  agent_id: string | null;
  error_message: string | null;
  log: string | null;
}

export interface DeployConfig {
  endpoints: DeployEndpoint[];
  test?: JobConfig;
}

export interface DeployEndpoint {
  provider: string;
  region?: string;
  zone?: string;
  vm_size?: string;
  instance_type?: string;
  machine_type?: string;
  os?: string;
  resource_group?: string;
  label?: string;
  ip?: string;
  ssh_user?: string;
  ssh_port?: number;
  http_stacks?: string[];
}

export interface CloudStatus {
  azure: ProviderStatus;
  aws: ProviderStatus;
  gcp: ProviderStatus;
  ssh: ProviderStatus;
}

export interface ProviderStatus {
  available: boolean;
  authenticated: boolean;
  account: string | null;
}

export interface ModeInfo {
  id: string;
  name: string;
  desc: string;
  detail: string;
}

export interface ModeGroup {
  label: string;
  detail: string;
  modes: ModeInfo[];
}

export interface PacketCaptureSummary {
  mode: string;
  interface: string;
  total_packets: number;
  capture_status: string;
  capture_confidence: string;
  note: string | null;
  warnings: string[];
  tcp_packets: number;
  udp_packets: number;
  quic_packets: number;
  http_packets: number;
  dns_packets: number;
  retransmissions: number;
  duplicate_acks: number;
  resets: number;
  likely_target_endpoints: string[];
  likely_target_packets: number;
  likely_target_pct_of_total: number;
  dominant_trace_port: number | null;
  transport_shares: { protocol: string; packets: number; pct_of_total: number }[];
  top_endpoints: { endpoint: string; packets: number }[];
  top_ports: { port: number; packets: number }[];
  observed_quic: boolean;
  observed_tcp_only: boolean;
  observed_mixed_transport: boolean;
  capture_may_be_ambiguous: boolean;
}

export interface Schedule {
  schedule_id: string;
  name: string | null;
  definition_id: string | null;
  agent_id: string | null;
  deployment_id: string | null;
  cron_expr: string;
  enabled: boolean;
  config: JobConfig | null;
  auto_start_vm: boolean;
  auto_stop_vm: boolean;
  created_by: string | null;
  created_at: string;
  next_run_at: string | null;
  last_run_at: string | null;
  benchmark_config_id: string | null;
}

export interface DashUser {
  user_id: string;
  email: string;
  role: string;
  status: string;
  auth_provider: string;
  display_name: string | null;
  last_login_at: string | null;
  created_at: string;
}

export interface CloudConnection {
  connection_id: string;
  name: string;
  provider: string;
  config: AzureConfig | AwsConfig | GcpConfig;
  status: string;
  last_validated: string | null;
  validation_error: string | null;
  created_by: string | null;
  created_at: string;
  updated_at: string;
}

export interface AzureConfig {
  subscription_id: string;
  tenant_id?: string;
  resource_groups?: string[];
}

export interface AwsConfig {
  account_id: string;
  role_arn: string;
  external_id?: string;
  regions?: string[];
}

export interface GcpConfig {
  project_id: string;
  workload_identity_pool?: string;
  provider_id?: string;
  regions?: string[];
}

export interface LiveAttempt {
  attempt_id: string;
  run_id: string;
  protocol: string;
  sequence_num: number;
  started_at: string;
  finished_at: string | null;
  success: boolean;
  retry_count: number;
  dns?: { duration_ms: number; query_name: string; resolved_ips: string[] };
  tcp?: { connect_duration_ms: number; remote_addr: string };
  tls?: { handshake_duration_ms: number; protocol_version: string; cipher_suite: string };
  http?: { status_code: number; ttfb_ms: number; total_duration_ms: number; negotiated_version: string; throughput_mbps?: number; goodput_mbps?: number; payload_bytes?: number; body_size_bytes?: number; headers_size_bytes?: number; redirect_count?: number; cpu_time_ms?: number };
  udp?: { rtt_avg_ms: number; rtt_min_ms?: number; rtt_p95_ms?: number; jitter_ms?: number; loss_percent: number; probe_count: number; success_count?: number };
  error?: { category: string; message: string; detail?: string };
  page_load?: { total_ms: number; ttfb_ms?: number; asset_count: number; assets_fetched: number; total_bytes?: number; connections_opened?: number; tls_setup_ms?: number; tls_overhead_ratio?: number; cpu_time_ms?: number; connection_reused?: boolean };
  browser?: { load_ms: number; dom_content_loaded_ms?: number; ttfb_ms?: number; resource_count?: number; transferred_bytes?: number; protocol?: string };
}

// Cloud account types
export interface CloudAccountSummary {
  account_id: string;
  name: string;
  provider: string;
  region_default: string | null;
  personal: boolean;
  status: string;
  last_validated: string | null;
}

export interface ShareLink {
  link_id: string;
  resource_type: string;
  resource_id: string | null;
  label: string | null;
  expires_at: string;
  created_by: string;
  created_by_email: string;
  created_at: string;
  revoked: boolean;
  access_count: number;
  last_accessed: string | null;
}

export interface TlsProfileSummary {
  id: string;
  started_at: string;
  host: string;
  port: number;
  target_kind: string;
  coverage_level: string;
  summary_status: string;
  summary_score: number | null;
}

export interface TlsEndpointProfile {
  target_kind: string;
  coverage_level: string;
  unsupported_checks?: string[];
  limitations?: string[];
  target: {
    host: string;
    port: number;
    requested_ip?: string | null;
    sni?: string | null;
    resolved_ips?: string[];
    source_url?: string | null;
  };
  path_characteristics: {
    connected_ip?: string | null;
    direct_ip_match: boolean;
    proxy_detected: boolean;
    classification: string;
    evidence?: string[];
  };
  connectivity: {
    tcp_connect_ms?: number | null;
    tls_handshake_ms?: number | null;
    negotiated_tls_version?: string | null;
    negotiated_cipher_suite?: string | null;
    negotiated_key_exchange_group?: string | null;
    alpn?: string | null;
  };
  certificate: {
    leaf?: {
      subject: string;
      issuer: string;
      serial_number: string;
      not_before?: string | null;
      not_after?: string | null;
      san_dns?: string[];
      san_ip?: string[];
      key_type: string;
      key_bits?: number | null;
      signature_algorithm: string;
      is_ca: boolean;
      sha256_fingerprint: string;
      spki_sha256: string;
      ocsp_urls?: string[];
      crl_urls?: string[];
      aia_issuers?: string[];
      must_staple: boolean;
      scts_present: boolean;
    } | null;
    chain?: Array<{
      subject: string;
      issuer: string;
      not_after?: string | null;
      sha256_fingerprint: string;
    }>;
  };
  trust: {
    hostname_matches: boolean;
    chain_valid: boolean;
    trusted_by_system_store: boolean;
    verification_performed: boolean;
    chain_presented: boolean;
    verified_chain_depth?: number | null;
    issues?: string[];
    revocation: {
      ocsp_stapled: boolean;
      method: string;
      status: string;
      ocsp_urls?: string[];
      crl_urls?: string[];
      online_check_attempted: boolean;
      notes?: string[];
    };
  };
  resumption: {
    supported: boolean;
    method?: string | null;
    initial_handshake_ms?: number | null;
    resumed_handshake_ms?: number | null;
    resumption_ratio?: number | null;
    resumed_tls_version?: string | null;
    resumed_cipher_suite?: string | null;
    early_data_offered: boolean;
    early_data_accepted?: boolean | null;
    notes?: string[];
  };
  findings: Array<{
    severity: string;
    code: string;
    message: string;
  }>;
  summary: {
    status: string;
    score?: number | null;
  };
}

export interface TlsProfileDetail {
  id: string;
  started_at: string;
  host: string;
  port: number;
  target_kind: string;
  coverage_level: string;
  summary_status: string;
  summary_score: number | null;
  profile: TlsEndpointProfile;
}

// Workspace Invites
export interface WorkspaceInvite {
  invite_id: string;
  project_id: string;
  email: string;
  role: string;
  status: string;
  invited_by: string;
  invited_by_email: string;
  created_at: string;
  expires_at: string;
}

export interface ResolvedInvite {
  invite_id: string;
  project_id: string;
  project_name: string;
  email: string;
  role: string;
  has_account: boolean;
  expires_at: string;
}

// Command Approvals
export interface CommandApproval {
  approval_id: string;
  project_id: string;
  agent_id: string;
  command_type: string;
  command_detail: Record<string, unknown>;
  status: string;
  requested_by: string;
  requested_by_email: string;
  decided_by: string | null;
  decided_by_email: string | null;
  requested_at: string;
  decided_at: string | null;
  expires_at: string;
  reason: string | null;
}

// Project types
export interface ProjectSummary {
  project_id: string;
  name: string;
  slug: string;
  role: string;
  description?: string;
  created_at: string;
}

export interface ProjectDetail extends ProjectSummary {
  settings: Record<string, unknown>;
  created_by: string;
  updated_at: string;
}

export interface ProjectMember {
  project_id: string;
  user_id: string;
  role: string;
  status: string;
  joined_at: string;
  invited_by: string | null;
  invite_sent_at: string | null;
  email: string;
  display_name: string | null;
}

export interface ImportDetail {
  email: string;
  result: string;
  message: string;
}

export interface ImportResult {
  imported: number;
  skipped: number;
  errors: number;
  details: ImportDetail[];
}

export interface SendInviteDetail {
  user_id: string;
  result: string;
  invite_url?: string;
}

export interface SendInviteResult {
  sent: number;
  skipped: number;
  errors: number;
  email_configured: boolean;
  details: SendInviteDetail[];
}

export interface SystemMetrics {
  cpu_usage_percent: number;
  memory_used_bytes: number;
  memory_total_bytes: number;
  disk_used_bytes: number;
  disk_total_bytes: number;
  uptime_seconds: number;
}

export interface DbMetrics {
  active_connections: number;
  max_connections: number;
  database_size_bytes: number;
  oldest_transaction_age_seconds: number | null;
  cache_hit_ratio: number;
}

export interface WorkspaceUsage {
  project_id: string;
  name: string;
  slug: string;
  member_count: number;
  tester_count: number;
  jobs_30d: number;
  runs_30d: number;
  last_activity: string | null;
  deleted_at: string | null;
  delete_protection: boolean;
}

export interface LogEntry {
  ts: string;
  level: number;
  service: string;
  message: string;
  config_id?: string | null;
  project_id?: string | null;
  trace_id?: string | null;
  fields?: Record<string, unknown> | null;
}

export interface LogsResponse {
  entries: LogEntry[];
  total: number;
  truncated: boolean;
}

// Benchmark types
export interface BenchmarkLeaderboardEntry {
  language: string;
  runtime: string;
  metrics: Record<string, number>;
  server_os: string | null;
  client_os: string | null;
  cloud: string | null;
  phase: string | null;
  concurrency: number | null;
}

export interface BenchmarkRun {
  run_id: string;
  name: string;
  config: Record<string, unknown>;
  status: string;
  started_at: string;
  finished_at: string | null;
  tier: string | null;
  created_by: string | null;
  results?: BenchmarkResultRow[];
}

export interface BenchmarkResultRow {
  result_id: string;
  run_id: string;
  language: string;
  runtime: string;
  server_os: string | null;
  client_os: string | null;
  cloud: string | null;
  phase: string | null;
  concurrency: number | null;
  metrics: Record<string, number>;
  started_at: string | null;
  finished_at: string | null;
}

// ── Benchmark Progress (per-mode live stats) ───────────────────────────
export interface BenchmarkModeProgress {
  mode: string;
  completed: number;
  total: number;
  p50_ms: number | null;
  mean_ms: number | null;
  success_count: number;
  fail_count: number;
}

export interface BenchmarkLanguageProgress {
  language: string;
  testbed_id: string | null;
  modes: BenchmarkModeProgress[];
}

export interface BenchmarkProgressResponse {
  progress: BenchmarkLanguageProgress[];
}

// ── Benchmark Creation (wizard) ─────────────────────────────────────────
export interface BenchmarkConfigSummary {
  config_id: string;
  name: string;
  status: string;
  template: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  testbed_count: number;
}

export interface BenchmarkTestbedConfig {
  cloud: string;
  region: string;
  topology: string;
  vm_size: string;
  existing_vm_ip: string | null;
  os: string;
  languages: string[];
  proxies?: string[];
  tester_os?: string;
}

export interface BenchmarkVmCatalogEntry {
  vm_id: string;
  name: string;
  cloud: string;
  region: string;
  ip: string;
  ssh_user: string;
  languages: string[];
  status: string;
  last_health_check: string | null;
}

// ── Benchmark Config Results (cross-testbed comparison) ─────────────────

export interface BenchmarkConfigResultSummary {
  case_id: string;
  protocol: string;
  payload_bytes: number | null;
  http_stack: string | null;
  metric_name: string;
  metric_unit: string;
  higher_is_better: boolean;
  sample_count: number;
  included_sample_count: number;
  mean: number;
  p5: number;
  p25: number;
  p50: number;
  p75: number;
  p95: number;
  p99: number;
  p999: number;
  max: number;
  min: number;
  stddev: number;
  rps: number;
}

export interface ConfigTestbedResult {
  run_id: string;
  testbed_id: string | null;
  language: string;
  status: string;
  started_at: string;
  finished_at: string | null;
  summaries: BenchmarkConfigResultSummary[];
}

export interface BenchmarkTestbedRow {
  testbed_id: string;
  config_id: string;
  cloud: string;
  region: string;
  topology: string;
  endpoint_vm_id: string | null;
  tester_vm_id: string | null;
  endpoint_ip: string | null;
  tester_ip: string | null;
  status: string;
  os: string;
  languages: string[];
  vm_size: string | null;
}

export interface BenchmarkConfigResults {
  config: BenchmarkConfigSummary & {
    project_id: string;
    config_json: Record<string, unknown>;
    error_message: string | null;
    max_duration_secs: number;
    baseline_run_id: string | null;
    created_by: string | null;
    worker_id: string | null;
    last_heartbeat: string | null;
  };
  testbeds: BenchmarkTestbedRow[];
  results: ConfigTestbedResult[];
}

// ── Benchmark Regressions ─────────────────────────────────────────

export interface BenchmarkRegression {
  regression_id: string;
  config_id: string;
  baseline_config_id: string | null;
  language: string;
  metric: string;
  baseline_value: number;
  current_value: number;
  delta_percent: number;
  severity: string;
  detected_at: string;
}

export interface BenchmarkRegressionWithConfig extends BenchmarkRegression {
  config_name: string;
}

export interface GroupedLeaderboardEntry {
  language: string;
  run_count: number;
  p5: number;
  p25: number;
  p50: number;
  p75: number;
  p95: number;
  mean: number;
  rps: number;
}

export interface GroupedLeaderboard {
  groups: string[];
  selected: string;
  languages: GroupedLeaderboardEntry[];
}

export interface BenchTokenInfo {
  name: string;
  config_id: string;
  testbed_id: string;
  created: string | null;
  expires: string | null;
  enabled: boolean;
  user: string | null;
  project_id: string | null;
}

// ── Performance Log ──────────────────────────────────────────────────

export interface PerfLogInput {
  kind: 'api' | 'render';
  timestamp?: number;
  method?: string;
  path?: string;
  status?: number;
  total_ms?: number;
  server_ms?: number;
  network_ms?: number;
  source?: string;
  component?: string;
  trigger?: string;
  render_ms?: number;
  item_count?: number;
}

export interface PerfLogRow {
  id: number;
  logged_at: string;
  user_id: string | null;
  session_id: string | null;
  kind: string;
  method: string | null;
  path: string | null;
  status: number | null;
  total_ms: number | null;
  server_ms: number | null;
  network_ms: number | null;
  source: string | null;
  component: string | null;
  trigger: string | null;
  render_ms: number | null;
  item_count: number | null;
}

export interface PerfLogStats {
  api_count: number;
  render_count: number;
  avg_total_ms: number | null;
  avg_server_ms: number | null;
  avg_render_ms: number | null;
  p95_total_ms: number | null;
  p95_render_ms: number | null;
  slow_api_count: number;
  janky_render_count: number;
}
