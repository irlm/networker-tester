import { describe, expect, it } from 'vitest';
import type { Agent, CloudAccountSummary, Deployment } from '../../api/types';
import { ALL_SCENARIOS } from '../../lib/scenarios';
import type { ScenarioReadinessResponse } from './api';
import { rankScenarios, scenarioAvailability, summarizeReadiness } from './model';

function readiness({
  agents = [],
  deployments = [],
  cloudAccounts = [],
  agentError = null,
}: {
  agents?: Agent[];
  deployments?: Deployment[];
  cloudAccounts?: CloudAccountSummary[];
  agentError?: string | null;
} = {}): ScenarioReadinessResponse {
  return {
    agents: { data: agentError ? null : agents, error: agentError },
    deployments: { data: deployments, error: null },
    cloudAccounts: { data: cloudAccounts, error: null },
  };
}

const onlineAgent = { agent_id: 'a-1', name: 'runner', status: 'online' } as Agent;
const activeDeployment = {
  deployment_id: 'd-1', name: 'target', status: 'completed', endpoint_ips: ['192.0.2.1'],
} as Deployment;
const activeCloud = { account_id: 'c-1', name: 'cloud', provider: 'aws', status: 'active' } as CloudAccountSummary;
const scenario = (id: string) => ALL_SCENARIOS.find((item) => item.id === id)!;

describe('scenario readiness model', () => {
  it('reports each independently verified project resource', () => {
    const summary = summarizeReadiness(readiness({
      agents: [onlineAgent], deployments: [activeDeployment], cloudAccounts: [activeCloud],
    }));

    expect(summary.map((item) => [item.id, item.tone, item.value])).toEqual([
      ['runner', 'ready', '1 ONLINE'],
      ['endpoint', 'ready', '1 ACTIVE'],
      ['cloud', 'ready', '1 ACTIVE'],
    ]);
  });

  it('routes endpoint tests to infrastructure repair when no target exists', () => {
    const availability = scenarioAvailability(
      scenario('endpoint-throughput'),
      readiness({ agents: [onlineAgent] }),
    );

    expect(availability.canConfigure).toBe(false);
    expect(availability.actionLabel).toBe('Deploy endpoint');
    expect(availability.actionPath('p-1')).toBe('/projects/p-1/vms');
  });

  it('allows configuration while honestly marking an unavailable readiness API', () => {
    const availability = scenarioAvailability(
      scenario('url-quick'),
      readiness({ agentError: 'Service unavailable' }),
    );

    expect(availability.tone).toBe('unverified');
    expect(availability.canConfigure).toBe(true);
    expect(availability.actionPath('p-1')).toBe('/projects/p-1/probe?preset=quick');
  });

  it('ranks a runnable alternative ahead of a blocked scenario without changing catalog order', () => {
    const scenarios = [scenario('endpoint-throughput'), scenario('url-quick')];
    const ranked = rankScenarios(scenarios, readiness({ agents: [onlineAgent] }));

    expect(ranked.map((item) => item.id)).toEqual(['url-quick', 'endpoint-throughput']);
  });
});
