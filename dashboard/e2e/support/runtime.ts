import { expect, type Page } from '@playwright/test';

export const PID = 'proj-e2e-001';

export const authenticatedRoutes = [
  ['projects', '/projects'],
  ['dashboard', `/projects/${PID}`],
  ['runs', `/projects/${PID}/runs`],
  ['new run', `/projects/${PID}/runs/new`],
  ['scenarios', `/projects/${PID}/scenarios`],
  ['network test', `/projects/${PID}/tests/new`],
  ['full-stack benchmark', `/projects/${PID}/benchmarks/full-stack/new`],
  ['application benchmark', `/projects/${PID}/benchmarks/application/new`],
  ['URL probe', `/projects/${PID}/probe`],
  ['endpoint runs', `/projects/${PID}/network/endpoint-e2e`],
  ['run comparison', `/projects/${PID}/runs/compare?ids=run-e2e-1,run-e2e-2`],
  ['run detail', `/projects/${PID}/runs/run-e2e-1`],
  ['infrastructure', `/projects/${PID}/vms`],
  ['VM history', `/projects/${PID}/vms/history`],
  ['deployment detail', `/projects/${PID}/deploy/deploy-e2e`],
  ['TLS profiles', `/projects/${PID}/tls-profiles`],
  ['TLS profile detail', `/projects/${PID}/tls-profiles/run-e2e-1`],
  ['schedules', `/projects/${PID}/schedules`],
  ['alerts', `/projects/${PID}/alerts`],
  ['settings', `/projects/${PID}/settings`],
  ['members', `/projects/${PID}/members`],
  ['cloud accounts', `/projects/${PID}/cloud-accounts`],
  ['share links', `/projects/${PID}/share-links`],
  ['command approvals', `/projects/${PID}/approvals`],
  ['benchmark catalog', `/projects/${PID}/benchmark-catalog`],
  ['benchmark results', `/projects/${PID}/benchmark-configs/config-e2e/results`],
  ['benchmark regressions', `/projects/${PID}/benchmark-regressions`],
  ['value report', `/projects/${PID}/reports/value`],
  ['SDK endpoints', `/projects/${PID}/sdk-endpoints`],
  ['application network report', `/projects/${PID}/reports/app-network`],
  ['leaderboard', '/leaderboard'],
  ['system dashboard', '/admin/system'],
  ['run-execution canary', '/admin/canary'],
  ['performance log', '/admin/perf-log'],
  ['benchmark tokens', '/bench-tokens'],
  ['users', '/users'],
  ['change password', '/change-password'],
  ['pending approval', '/pending'],
] as const;

export const publicRoutes = [
  ['login', '/login'],
  ['forgot password', '/forgot-password'],
  ['reset password', '/reset-password?token=e2e-token'],
  ['share view', '/share/e2e-token'],
  ['invite acceptance', '/invite/e2e-token'],
] as const;

function project() {
  return {
    project_id: PID,
    name: 'E2E Project',
    slug: 'e2e-project',
    description: 'browser fixture',
    created_at: new Date(0).toISOString(),
    updated_at: new Date(0).toISOString(),
    role: 'admin',
  };
}

/** Shared deterministic browser backend for route, responsive, and a11y suites. */
export async function stubRuntime(page: Page) {
  await page.routeWebSocket('**/ws/**', socket => socket.close({ code: 1000, reason: 'browser test' }));
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({
      status,
      contentType: 'application/json',
      headers: { 'X-Process-Time-Ms': '1.0' },
      body: JSON.stringify(body),
    });

    if (path.endsWith('/api/auth/profile')) {
      return json({
        user_id: '11111111-1111-4111-8111-111111111111',
        email: 'e2e@example.com',
        role: 'admin',
        status: 'active',
        is_platform_admin: true,
        must_change_password: false,
      });
    }
    if (path.endsWith('/api/auth/sso/providers')) return json({ providers: [] });
    if (path.endsWith('/api/projects')) return json({ projects: [project()] });
    if (path.endsWith(`/api/v2/projects/${PID}/test-runs`) && url.searchParams.get('limit') === '200') {
      const common = {
        project_id: PID,
        status: 'completed',
        started_at: '2026-08-18T12:00:00Z',
        finished_at: '2026-08-18T12:00:05Z',
        success_count: 2,
        failure_count: 0,
        error_message: null,
        artifact_id: null,
        tester_id: null,
        worker_id: null,
        last_heartbeat: null,
        created_at: '2026-08-18T12:00:00Z',
      };
      return json([
        { ...common, id: 'run-list-network', test_config_id: 'config-network', config_name: 'Checkout connectivity', endpoint_kind: 'proxy', test_kind: 'network', modes: ['tcp'] },
        { ...common, id: 'run-list-url', test_config_id: 'config-url', config_name: 'Diag: api.example.com (Quick)', endpoint_kind: 'network', test_kind: 'url_probe', modes: ['http2'] },
        { ...common, id: 'run-list-sdk', test_config_id: 'config-sdk', config_name: 'Payments SDK', endpoint_kind: 'network', test_kind: 'sdk_probe', modes: ['sdkprobe'] },
        { ...common, id: 'run-list-benchmark', test_config_id: 'config-benchmark', config_name: 'Runtime throughput', endpoint_kind: 'runtime', test_kind: 'benchmark', modes: ['apibench', 'download'], artifact_id: 'artifact-1' },
      ]);
    }
    if (path.endsWith(`/api/projects/${PID}`)) return json(project());
    if (path.endsWith('/api/me/pending-projects')) return json({ pending: [] });
    if (path.includes('/vm-history')) return json({ events: [], has_more: false });
    if (path.endsWith('/api/logs')) return json({ entries: [], total: 0 });
    if (path.endsWith('/api/modes')) return json({ groups: [], language_capabilities: [] });
    if (path.endsWith('/api/summary')) {
      return json({ agents_online: 0, jobs_running: 0, runs_24h: 0, jobs_pending: 0 });
    }
    if (path.endsWith('/api/version')) {
      return json({ dashboard_version: 'e2e', tester_version: null, latest_release: null, update_available: false, endpoints: [] });
    }
    if (path.endsWith('/api/system/health')) {
      return json({
        live: { core_db: true, logs_db: true },
        checks: [],
      });
    }
    if (path.endsWith('/api/admin/canary')) {
      return json({
        configured: false,
        owner: 'irlm',
        repo: 'networker-tester',
        workflow: 'soak-canary.yml',
        actions_url: 'https://github.com/irlm/networker-tester/actions',
      });
    }
    if (path.endsWith(`/api/projects/${PID}/members`)) return json({ members: [] });
    if (path.endsWith(`/api/projects/${PID}/invites`)) return json([]);
    if (path.endsWith(`/api/projects/${PID}/command-approvals/count`)) return json({ count: 0 });
    if (path.endsWith(`/api/projects/${PID}/command-approvals`)) return json({ approvals: [] });
    if (path.endsWith('/api/perf-log/stats')) {
      return json({
        api_count: 0,
        render_count: 0,
        avg_total_ms: null,
        avg_server_ms: null,
        avg_render_ms: null,
        p95_total_ms: null,
        p95_render_ms: null,
        slow_api_count: 0,
        janky_render_count: 0,
      });
    }
    if (path.endsWith(`/api/projects/${PID}/reports/perf-per-cost`)) {
      return json({
        generated_at: new Date(0).toISOString(),
        cost_table: {
          as_of: '2026-08-18',
          disclaimer: 'E2E fixture prices.',
          source: 'fixture',
        },
        formulas: {
          latency_cost_index: 'p95 × hourly cost',
          mbps_per_dollar_hour: 'throughput ÷ hourly cost',
        },
        completed_runs: 0,
        providers_with_data: 0,
        groups: [],
        missing_cost_skus: [],
      });
    }
    if (path.endsWith(`/api/projects/${PID}/reports/app-network`)) {
      return json({
        generated_at: new Date(0).toISOString(),
        formulas: {
          server_ms: 'server duration',
          network_ms: 'wall duration - server duration',
          split: 'network + server',
          split_anomaly: 'server duration > wall duration',
        },
        mode: 'sdkprobe',
        attempt_count: 0,
        split_anomaly_count: 0,
        overall_verdict: 'no_data',
        overall_main_issue: 'No data yet',
        overall_median_server_ms: null,
        overall_median_network_ms: null,
        overall_median_wall_ms: null,
        overall_server_ratio: null,
        groups: [],
      });
    }
    if (path.endsWith(`/api/projects/${PID}/benchmark-configs/config-e2e/results`)) {
      return json({
        config: {
          config_id: 'config-e2e',
          project_id: PID,
          name: 'Browser fixture benchmark',
          status: 'completed',
          template: null,
          created_at: new Date(0).toISOString(),
          started_at: new Date(0).toISOString(),
          finished_at: new Date(0).toISOString(),
          testbed_count: 0,
          config_json: {},
          error_message: null,
          max_duration_secs: 0,
          baseline_run_id: null,
          created_by: null,
          worker_id: null,
          last_heartbeat: null,
        },
        testbeds: [],
        results: [],
      });
    }
    if (path.endsWith(`/api/projects/${PID}/tls-profiles/run-e2e-1`)) {
      return json({
        id: 'run-e2e-1',
        started_at: new Date(0).toISOString(),
        host: 'example.com',
        port: 443,
        target_kind: 'external-host',
        coverage_level: 'standard',
        summary_status: 'good',
        summary_score: 100,
        profile: {
          target_kind: 'external-host',
          coverage_level: 'standard',
          unsupported_checks: [],
          limitations: [],
          target: { host: 'example.com', port: 443, resolved_ips: [] },
          path_characteristics: {
            connected_ip: null,
            direct_ip_match: true,
            proxy_detected: false,
            classification: 'direct',
            evidence: [],
          },
          connectivity: {
            tcp_connect_ms: 10,
            tls_handshake_ms: 20,
            negotiated_tls_version: 'TLS 1.3',
            negotiated_cipher_suite: 'TLS_AES_128_GCM_SHA256',
            negotiated_key_exchange_group: 'X25519',
            alpn: 'h2',
          },
          certificate: { leaf: null, chain: [] },
          trust: {
            hostname_matches: true,
            chain_valid: true,
            trusted_by_system_store: true,
            verification_performed: true,
            chain_presented: true,
            verified_chain_depth: 2,
            issues: [],
            revocation: {
              ocsp_stapled: false,
              method: 'not-required',
              status: 'not-checked',
              online_check_attempted: false,
            },
          },
          resumption: {
            supported: true,
            method: 'ticket',
            early_data_offered: false,
            early_data_accepted: false,
            notes: [],
          },
          findings: [],
          summary: { status: 'good', score: 100 },
        },
      });
    }
    if (/\/api\/v2\/test-runs\/run-e2e-\d+\/attempts$/.test(path)) return json({ attempts: [] });
    if (/\/api\/v2\/test-runs\/run-e2e-\d+\/infra$/.test(path)) return json({}, 404);
    if (/\/api\/v2\/test-runs\/run-e2e-\d+$/.test(path)) {
      return json({
        id: path.split('/').at(-1),
        project_id: PID,
        test_config_id: 'config-e2e',
        config_name: 'Browser fixture run',
        status: 'completed',
        modes: ['tcp'],
        success_count: 0,
        failure_count: 0,
        artifact_id: null,
        created_at: new Date(0).toISOString(),
        started_at: new Date(0).toISOString(),
        finished_at: new Date(0).toISOString(),
      });
    }
    if (path.endsWith('/api/v2/test-runs/compare')) {
      return json({ run_ids: ['run-e2e-1', 'run-e2e-2'], cases: [] });
    }
    if (path.includes('/api/share/e2e-token')) {
      return json({ project_name: 'E2E Project', report_type: 'run', data: {}, expires_at: null });
    }
    if (path.endsWith('/api/invite/e2e-token')) {
      return json({ project_id: PID, project_name: 'E2E Project', email: 'e2e@example.com', role: 'viewer', expires_at: new Date(Date.now() + 86_400_000).toISOString() });
    }
    return json([]);
  });
}

export async function seedSession(page: Page) {
  await page.addInitScript(() => {
    localStorage.setItem('token', 'e2e-fake-token');
    localStorage.setItem('email', 'e2e@example.com');
    localStorage.setItem('role', 'admin');
    localStorage.setItem('status', 'active');
    localStorage.setItem('isPlatformAdmin', 'true');
  });
}

export function watchForFatalErrors(page: Page): () => string[] {
  const failures: string[] = [];
  page.on('pageerror', error => failures.push(`pageerror: ${error.message}`));
  page.on('response', response => {
    if (response.status() >= 400 && /\.(js|css)(\?|$)/.test(response.url())) {
      failures.push(`asset ${response.status()}: ${response.url()}`);
    }
  });
  return () => failures;
}

export async function expectRouteToRender(page: Page, path: string, failures: () => string[]) {
  const response = await page.goto(path, { waitUntil: 'domcontentloaded' });
  expect(response?.status(), `${path} did not return a document`).toBeLessThan(400);
  await expect(page.locator('#root')).not.toBeEmpty({ timeout: 15_000 });
  await page.waitForTimeout(150);
  await expect(page.getByText(/Something (?:went wrong|broke)/i)).toHaveCount(0);
  expect(failures(), `${path} reported ${failures().join('\n')}`).toEqual([]);
}
