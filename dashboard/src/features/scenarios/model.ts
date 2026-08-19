import type { Scenario } from '../../lib/scenarios';
import type { ScenarioReadinessResponse } from './api';

export type ScenarioIntentId = 'url' | 'route' | 'endpoint' | 'benchmark';
export type ReadinessTone = 'ready' | 'attention' | 'blocked' | 'unverified';

export interface ScenarioIntent {
  id: ScenarioIntentId;
  label: string;
  description: string;
  scenarioIds: string[];
}

export const SCENARIO_INTENTS: ScenarioIntent[] = [
  {
    id: 'url',
    label: 'URL symptoms',
    description: 'Resolve slow handshakes, protocol negotiation, and real page-load reports.',
    scenarioIds: ['url-quick', 'url-protocols', 'url-pageload'],
  },
  {
    id: 'route',
    label: 'Route & reachability',
    description: 'Find loss, broken hops, IPv4/IPv6 differences, and path MTU problems.',
    scenarioIds: ['url-route', 'url-quick', 'url-protocols'],
  },
  {
    id: 'endpoint',
    label: 'Deployed endpoint',
    description: 'Measure throughput, responsiveness, WebSockets, or HTTP versions.',
    scenarioIds: [
      'endpoint-throughput',
      'endpoint-bufferbloat',
      'endpoint-websocket',
      'endpoint-http-versions',
    ],
  },
  {
    id: 'benchmark',
    label: 'Controlled benchmark',
    description: 'Provision a controlled testbed and compare stacks or language runtimes.',
    scenarioIds: ['full-stack-compare', 'app-language-stack', 'app-api-compute'],
  },
];

const OUTPUTS: Record<string, string> = {
  'url-quick': 'Layer timing with DNS, TCP, TLS, and first-byte evidence',
  'url-protocols': 'Protocol timing matrix and TLS resumption evidence',
  'url-route': 'Hop-by-hop path, loss, dual-stack verdict, and path MTU',
  'url-pageload': 'Browser load timing compared across HTTP versions',
  'endpoint-throughput': 'Sustained download and upload throughput',
  'endpoint-bufferbloat': 'Loaded latency, RPM, and bufferbloat factor',
  'endpoint-websocket': 'Upgrade timing, RTT distribution, jitter, and loss',
  'endpoint-http-versions': 'Same-target HTTP/1, HTTP/2, and HTTP/3 comparison',
  'full-stack-compare': 'Per-proxy protocol and throughput matrix',
  'app-language-stack': 'Per-runtime protocol and throughput matrix',
  'app-api-compute': 'Per-language compute-bound request comparison',
};

export interface ReadinessSummaryItem {
  id: 'runner' | 'endpoint' | 'cloud';
  label: string;
  tone: ReadinessTone;
  value: string;
  detail: string;
  repairLabel: string;
  repairPath: (projectId: string) => string;
}

export interface ScenarioAvailability {
  tone: ReadinessTone;
  label: string;
  detail: string;
  actionLabel: string;
  actionPath: (projectId: string) => string;
  canConfigure: boolean;
}

function onlineAgents(readiness?: ScenarioReadinessResponse): number | null {
  const agents = readiness?.agents.data;
  return agents ? agents.filter((agent) => agent.status === 'online').length : null;
}

function activeEndpoints(readiness?: ScenarioReadinessResponse): number | null {
  const deployments = readiness?.deployments.data;
  return deployments
    ? deployments.filter((deployment) =>
        deployment.status === 'completed'
        && Boolean(deployment.endpoint_ips?.length || deployment.endpoint_hosts?.some(Boolean)),
      ).length
    : null;
}

function activeCloudAccounts(readiness?: ScenarioReadinessResponse): number | null {
  const accounts = readiness?.cloudAccounts.data;
  return accounts ? accounts.filter((account) => account.status === 'active').length : null;
}

export function summarizeReadiness(
  readiness?: ScenarioReadinessResponse,
): ReadinessSummaryItem[] {
  const runnerCount = onlineAgents(readiness);
  const agentTotal = readiness?.agents.data?.length ?? null;
  const endpointCount = activeEndpoints(readiness);
  const deploymentTotal = readiness?.deployments.data?.length ?? null;
  const cloudCount = activeCloudAccounts(readiness);
  const cloudTotal = readiness?.cloudAccounts.data?.length ?? null;

  return [
    runnerCount == null
      ? {
          id: 'runner', label: 'Runner', tone: 'unverified', value: 'UNVERIFIED',
          detail: readiness?.agents.error ?? 'Checking runner inventory…',
          repairLabel: 'View runners', repairPath: (pid) => `/projects/${pid}/vms`,
        }
      : runnerCount > 0
        ? {
            id: 'runner', label: 'Runner', tone: 'ready', value: `${runnerCount} ONLINE`,
            detail: `${runnerCount} of ${agentTotal} runners can accept work.`,
            repairLabel: 'View runners', repairPath: (pid) => `/projects/${pid}/vms`,
          }
        : {
            id: 'runner', label: 'Runner', tone: agentTotal ? 'attention' : 'blocked',
            value: agentTotal ? 'OFFLINE' : 'REQUIRED',
            detail: agentTotal ? `${agentTotal} registered runner${agentTotal === 1 ? ' is' : 's are'} offline.` : 'No runner is registered for this project.',
            repairLabel: agentTotal ? 'View runners' : 'Add runner', repairPath: (pid) => `/projects/${pid}/vms`,
          },
    endpointCount == null
      ? {
          id: 'endpoint', label: 'Endpoint', tone: 'unverified', value: 'UNVERIFIED',
          detail: readiness?.deployments.error ?? 'Checking deployed targets…',
          repairLabel: 'View infrastructure', repairPath: (pid) => `/projects/${pid}/vms`,
        }
      : endpointCount > 0
        ? {
            id: 'endpoint', label: 'Endpoint', tone: 'ready', value: `${endpointCount} ACTIVE`,
            detail: `${endpointCount} completed deployment${endpointCount === 1 ? '' : 's'} expose a target.`,
            repairLabel: 'View endpoints', repairPath: (pid) => `/projects/${pid}/vms`,
          }
        : {
            id: 'endpoint', label: 'Endpoint', tone: 'blocked', value: 'NO TARGET',
            detail: deploymentTotal ? `${deploymentTotal} deployment${deploymentTotal === 1 ? '' : 's'} found, but none expose an active target.` : 'No endpoint has been deployed yet.',
            repairLabel: 'Deploy endpoint', repairPath: (pid) => `/projects/${pid}/vms`,
          },
    cloudCount == null
      ? {
          id: 'cloud', label: 'Cloud', tone: 'unverified', value: 'UNVERIFIED',
          detail: readiness?.cloudAccounts.error ?? 'Checking cloud credentials…',
          repairLabel: 'View cloud accounts', repairPath: (pid) => `/projects/${pid}/cloud-accounts`,
        }
      : cloudCount > 0
        ? {
            id: 'cloud', label: 'Cloud', tone: 'ready', value: `${cloudCount} ACTIVE`,
            detail: `${cloudCount} validated cloud account${cloudCount === 1 ? '' : 's'} can provision a testbed.`,
            repairLabel: 'View cloud accounts', repairPath: (pid) => `/projects/${pid}/cloud-accounts`,
          }
        : {
            id: 'cloud', label: 'Cloud', tone: cloudTotal ? 'attention' : 'blocked',
            value: cloudTotal ? 'NEEDS ATTENTION' : 'REQUIRED',
            detail: cloudTotal ? `${cloudTotal} cloud account${cloudTotal === 1 ? '' : 's'} need validation.` : 'No cloud account is connected.',
            repairLabel: cloudTotal ? 'Fix cloud account' : 'Add cloud account', repairPath: (pid) => `/projects/${pid}/cloud-accounts`,
          },
  ];
}

function configureLabel(scenario: Scenario): string {
  if (scenario.flow === 'url') return 'Configure URL probe';
  if (scenario.flow === 'endpoint') return 'Configure endpoint test';
  return 'Review benchmark testbed';
}

export function scenarioAvailability(
  scenario: Scenario,
  readiness?: ScenarioReadinessResponse,
): ScenarioAvailability {
  const summary = summarizeReadiness(readiness);
  const runner = summary[0];
  const endpoint = summary[1];
  const cloud = summary[2];
  const configure: ScenarioAvailability = {
    tone: 'ready',
    label: 'READY',
    detail: scenario.flow.startsWith('provision-')
      ? 'Cloud credentials are validated for controlled provisioning.'
      : 'Required project infrastructure is available.',
    actionLabel: configureLabel(scenario),
    actionPath: (pid) => scenario.href(pid),
    canConfigure: true,
  };

  const required = scenario.flow === 'url'
    ? [runner]
    : scenario.flow === 'endpoint'
      ? [runner, endpoint]
      : [cloud];
  const unverified = required.find((item) => item.tone === 'unverified');
  if (unverified) {
    return {
      ...configure,
      tone: 'unverified',
      label: 'UNVERIFIED',
      detail: `Could not verify ${unverified.label.toLowerCase()} readiness. Configuration is still available.`,
    };
  }

  const unavailable = required.find((item) => item.tone !== 'ready');
  if (!unavailable) return configure;

  return {
    tone: unavailable.tone,
    label: unavailable.value,
    detail: unavailable.detail,
    actionLabel: unavailable.repairLabel,
    actionPath: unavailable.repairPath,
    canConfigure: false,
  };
}

export function scenarioOutput(scenario: Scenario): string {
  return OUTPUTS[scenario.id] ?? scenario.measures.join(', ');
}

export function scenarioMethod(scenario: Scenario): string {
  const modeList = scenario.modes.join(', ');
  return scenario.flow.startsWith('provision-')
    ? `Provisions an isolated testbed, then runs ${modeList}. You review provider, region, and cost-affecting choices before launch.`
    : `Uses the existing tested builder with ${modeList} selected. Target, repetitions, timeouts, and capture settings remain editable before launch.`;
}

export function rankScenarios(
  scenarios: Scenario[],
  readiness?: ScenarioReadinessResponse,
): Scenario[] {
  const score: Record<ReadinessTone, number> = { ready: 0, unverified: 1, attention: 2, blocked: 3 };
  return scenarios
    .map((scenario, index) => ({ scenario, index, score: score[scenarioAvailability(scenario, readiness).tone] }))
    .sort((a, b) => a.score - b.score || a.index - b.index)
    .map(({ scenario }) => scenario);
}
