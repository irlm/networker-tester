// The create flow's arithmetic: which languages get reused vs provisioned,
// how many servers each shape needs, and what that costs. The owner's brief
// was explicit that reuse is the default *because it is cheaper*, so the
// money-facing branches are pinned here rather than left to the page.

import { describe, expect, it } from 'vitest';
import type { SdkSampleCostPreview, SdkSampleStatus } from '../api/types';
import {
  anyReusable,
  costFor,
  defaultSelection,
  planFor,
  submitLabel,
  unregistered,
  usdPerHour,
  usdPerMonth,
  versionLine,
} from './sdkSamples';

function sample(over: Partial<SdkSampleStatus> & { language: string }): SdkSampleStatus {
  return {
    label: over.language,
    runtime: 'runtime',
    description: 'desc',
    port: 8101,
    url: null,
    route: '/laghound/echo',
    state: 'none',
    recommended_action: 'create',
    reason: 'reason',
    reusable: false,
    current_version: '1.0.0',
    deployed_version: null,
    deployment_id: null,
    deployment_name: null,
    deployment_status: null,
    host: null,
    provider: null,
    region: null,
    vm_size: null,
    consolidated: null,
    samples_on_host: null,
    sdk_endpoint_id: null,
    ...over,
  };
}

const current = sample({
  language: 'go',
  state: 'current',
  recommended_action: 'reuse',
  reusable: true,
  deployed_version: '1.0.0',
  deployment_id: 'd-1',
  sdk_endpoint_id: 'sdk-1',
});
const outdatedGo = sample({
  language: 'rust',
  state: 'outdated',
  recommended_action: 'update',
  reusable: true,
  deployed_version: '0.9.0',
  deployment_id: 'd-2',
});
const missing = sample({ language: 'python' });
const dead = sample({
  language: 'js',
  state: 'unhealthy',
  recommended_action: 'redeploy',
  reusable: false,
  deployment_id: 'd-3',
});
const inFlight = sample({
  language: 'csharp',
  state: 'deploying',
  recommended_action: 'wait',
  reusable: false,
  deployment_id: 'd-4',
});

const price: SdkSampleCostPreview = {
  provider: 'azure',
  region: 'eastus',
  vm_size: 'Standard_B2s',
  hourly_usd: 0.1,
  monthly_usd: 72,
  note: 'per server, always-on',
};

const all = [current, outdatedGo, missing, dead, inFlight];

describe('planFor', () => {
  it('reuses everything usable and provisions only the gaps', () => {
    const plan = planFor(all, ['go', 'rust', 'python', 'js'], 'consolidated', true);
    expect(plan.reuse).toEqual(['go', 'rust']);
    expect(plan.provision).toEqual(['python', 'js']);
    expect(plan.serverCount).toBe(1);
  });

  it('separated needs one server per provisioned language, not per selected one', () => {
    const plan = planFor(all, ['go', 'rust', 'python', 'js'], 'separated', true);
    // go + rust are reused, so only python + js cost anything.
    expect(plan.serverCount).toBe(2);
  });

  it('provisions nothing when every selection is reusable', () => {
    const plan = planFor(all, ['go', 'rust'], 'consolidated', true);
    expect(plan.provision).toEqual([]);
    expect(plan.serverCount).toBe(0);
    expect(plan.empty).toBe(false);
  });

  it('reuse_existing=false is the explicit "provision anyway" escape hatch', () => {
    const plan = planFor(all, ['go', 'rust'], 'separated', false);
    expect(plan.reuse).toEqual([]);
    expect(plan.provision).toEqual(['go', 'rust']);
    expect(plan.serverCount).toBe(2);
  });

  it('an in-flight deploy is waited on, never double-provisioned', () => {
    const plan = planFor(all, ['csharp'], 'separated', true);
    expect(plan.waiting).toEqual(['csharp']);
    expect(plan.provision).toEqual([]);
    expect(plan.serverCount).toBe(0);
  });

  it('an unhealthy sample is provisioned, never reused', () => {
    const plan = planFor(all, ['js'], 'consolidated', true);
    expect(plan.reuse).toEqual([]);
    expect(plan.provision).toEqual(['js']);
  });

  it('an empty selection is empty', () => {
    expect(planFor(all, [], 'consolidated', true).empty).toBe(true);
  });
});

describe('costFor', () => {
  it('prices the servers the plan actually starts', () => {
    const plan = planFor(all, ['go', 'rust', 'python', 'js'], 'separated', true);
    const cost = costFor(plan, price);
    expect(cost?.hourlyUsd).toBeCloseTo(0.2);
    expect(cost?.monthlyUsd).toBeCloseTo(144);
  });

  it('consolidated is one server no matter how many languages it carries', () => {
    const plan = planFor(all, ['python', 'js'], 'consolidated', true);
    expect(costFor(plan, price)?.monthlyUsd).toBeCloseTo(72);
  });

  it('reports what reuse avoids', () => {
    const plan = planFor(all, ['go', 'rust', 'python'], 'consolidated', true);
    expect(costFor(plan, price)?.avoidedMonthlyUsd).toBeCloseTo(144);
  });

  it('is null when nothing priced the server rather than guessing zero', () => {
    expect(costFor(planFor(all, ['python'], 'consolidated', true), null)).toBeNull();
  });
});

describe('versionLine', () => {
  it('shows one version when the deployed sample is current', () => {
    expect(versionLine(current)).toBe('1.0.0');
  });

  it('shows both versions when the deployed sample is behind', () => {
    expect(versionLine(outdatedGo)).toBe('0.9.0 → 1.0.0');
  });

  it('never invents a deployed version it could not read', () => {
    expect(versionLine({ ...dead, state: 'unknown_version', deployed_version: null }))
      .toBe('? → 1.0.0');
  });

  it('shows the catalog version alone when nothing is deployed', () => {
    expect(versionLine(missing)).toBe('1.0.0');
  });
});

describe('submitLabel', () => {
  it('never claims to deploy servers a reuse-only plan will not start', () => {
    expect(submitLabel(planFor(all, ['go'], 'consolidated', true))).toBe('Reuse 1 existing');
  });

  it('names the server count and the reuse count', () => {
    expect(submitLabel(planFor(all, ['go', 'python', 'js'], 'separated', true)))
      .toBe('Deploy 2 servers · reuse 1');
    expect(submitLabel(planFor(all, ['python'], 'consolidated', true))).toBe('Deploy 1 server');
  });

  it('asks for a selection when there is none', () => {
    expect(submitLabel(planFor(all, [], 'consolidated', true))).toBe('Select a language');
  });
});

describe('selection helpers', () => {
  it('pre-selects only what is not already current', () => {
    expect(defaultSelection(all)).toEqual(['rust', 'python', 'js', 'csharp']);
  });

  it('falls back to everything when nothing is missing', () => {
    expect(defaultSelection([current])).toEqual(['go']);
  });

  it('anyReusable sees the reuse-first opportunity', () => {
    expect(anyReusable(all)).toBe(true);
    expect(anyReusable([missing, dead])).toBe(false);
  });

  it('unregistered finds usable samples with no SDK endpoint row yet', () => {
    expect(unregistered(all).map((s) => s.language)).toEqual(['rust']);
  });
});

describe('money formatting', () => {
  it('renders hourly to 3 dp and monthly to 2', () => {
    expect(usdPerHour(0.0964)).toBe('$0.096/h');
    expect(usdPerMonth(69.408)).toBe('$69.41/mo');
  });
});
