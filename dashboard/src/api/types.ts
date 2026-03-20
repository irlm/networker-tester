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
  error_message: string | null;
}

export interface JobConfig {
  target: string;
  modes: string[];
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
