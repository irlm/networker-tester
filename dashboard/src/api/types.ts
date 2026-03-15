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
  http?: { status_code: number; ttfb_ms: number; total_duration_ms: number; negotiated_version: string; throughput_mbps?: number; payload_bytes?: number };
  udp?: { rtt_avg_ms: number; loss_percent: number; probe_count: number };
  error?: { category: string; message: string; detail?: string };
}
