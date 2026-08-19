import { describe, expect, it } from 'vitest';
import type { TesterRow } from '../../api/testers';
import type { CloudAccountSummary, Deployment } from '../../api/types';
import { ALL_SCENARIOS } from '../../lib/scenarios';
import type { ScenarioReadinessResponse } from './api';
import { rankScenarios, SCENARIO_INTENTS, scenarioAvailability, summarizeReadiness } from './model';

function readiness({
  runners = [],
  deployments = [],
  cloudAccounts = [],
  runnerError = null,
}: {
  runners?: TesterRow[];
  deployments?: Deployment[];
  cloudAccounts?: CloudAccountSummary[];
  runnerError?: string | null;
} = {}): ScenarioReadinessResponse {
  return {
    runners: { data: runnerError ? null : runners, error: runnerError },
    deployments: { data: deployments, error: null },
    cloudAccounts: { data: cloudAccounts, error: null },
  };
}

const onlineRunner = { tester_id: 't-1', name: 'runner', power_state: 'running', agent_status: 'online' } as TesterRow;
const activeDeployment = {
  deployment_id: 'd-1', name: 'target', status: 'completed', endpoint_ips: ['192.0.2.1'],
} as Deployment;
const activeCloud = { account_id: 'c-1', name: 'cloud', provider: 'aws', status: 'active' } as CloudAccountSummary;
const scenario = (id: string) => ALL_SCENARIOS.find((item) => item.id === id)!;

describe('scenario readiness model', () => {
  it('reports each independently verified project resource', () => {
    const summary = summarizeReadiness(readiness({
      runners: [onlineRunner], deployments: [activeDeployment], cloudAccounts: [activeCloud],
    }));

    expect(summary.map((item) => [item.id, item.tone, item.value])).toEqual([
      ['runner', 'ready', '1 ONLINE'],
      ['endpoint', 'ready', '1 ACTIVE'],
      ['cloud', 'ready', '1 ACTIVE'],
    ]);
  });

  it('does not call a powered-on VM ready when its agent is disconnected', () => {
    const disconnected = { ...onlineRunner, agent_status: 'offline' };
    const runner = summarizeReadiness(readiness({ runners: [disconnected] }))
      .find((item) => item.id === 'runner');

    expect(runner).toMatchObject({ tone: 'attention', value: 'OFFLINE' });
  });

  it('routes endpoint tests to infrastructure repair when no target exists', () => {
    const availability = scenarioAvailability(
      scenario('endpoint-throughput'),
      readiness({ runners: [onlineRunner] }),
    );

    expect(availability.canConfigure).toBe(false);
    expect(availability.actionLabel).toBe('Deploy endpoint');
    expect(availability.actionPath('p-1')).toBe('/projects/p-1/vms');
  });

  it('allows configuration while honestly marking an unavailable readiness API', () => {
    const availability = scenarioAvailability(
      scenario('url-quick'),
      readiness({ runnerError: 'Service unavailable' }),
    );

    expect(availability.tone).toBe('unverified');
    expect(availability.canConfigure).toBe(true);
    expect(availability.actionPath('p-1')).toBe('/projects/p-1/probe?preset=quick');
  });

  it('ranks a runnable alternative ahead of a blocked scenario without changing catalog order', () => {
    const scenarios = [scenario('endpoint-throughput'), scenario('url-quick')];
    const ranked = rankScenarios(scenarios, readiness({ runners: [onlineRunner] }));

    expect(ranked.map((item) => item.id)).toEqual(['url-quick', 'endpoint-throughput']);
  });

  it('keeps intent ids and catalog ids exhaustive in both directions', () => {
    const intentIds = new Set(SCENARIO_INTENTS.flatMap((intent) => intent.scenarioIds));
    expect([...intentIds].sort()).toEqual(ALL_SCENARIOS.map((item) => item.id).sort());
  });
});
