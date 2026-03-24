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
  joined_at: string;
  invited_by: string | null;
  email: string;
  display_name: string | null;
}
