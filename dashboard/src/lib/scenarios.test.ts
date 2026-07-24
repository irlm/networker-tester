// Guards the scenario catalog against the capability model: a scenario must
// never carry a mode that can only fail against the target its flow lands on —
// which, since v0.28.72, the server would 422 at config-create. This keeps the
// launcher honest as modes/presets evolve.

import { describe, it, expect } from 'vitest';
import { ALL_SCENARIOS, SCENARIO_GROUPS } from './scenarios';
import { requirementOf, isModeSupported } from './mode-capabilities';
import { RUNTIME_TEMPLATES } from '../components/wizard/testbed-constants';

const templateIds = new Set(RUNTIME_TEMPLATES.map(t => t.id));

describe('scenario catalog integrity', () => {
  it('has unique scenario ids', () => {
    const ids = ALL_SCENARIOS.map(s => s.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('every scenario is fully populated', () => {
    for (const s of ALL_SCENARIOS) {
      expect(s.title, s.id).toBeTruthy();
      expect(s.summary, s.id).toBeTruthy();
      expect(s.badge, s.id).toBeTruthy();
      expect(s.needs, s.id).toBeTruthy();
      expect(s.est, s.id).toBeTruthy();
      expect(s.modes.length, `${s.id} modes`).toBeGreaterThan(0);
      expect(s.measures.length, `${s.id} measures`).toBeGreaterThan(0);
    }
  });

  it('every scenario belongs to exactly one group', () => {
    const grouped = SCENARIO_GROUPS.flatMap(g => g.scenarios);
    expect(grouped.length).toBe(ALL_SCENARIOS.length);
  });
});

describe('scenario ⇄ capability model', () => {
  it('URL scenarios carry only any-target modes (a raw URL cannot run the rest)', () => {
    for (const s of ALL_SCENARIOS.filter(s => s.flow === 'url')) {
      for (const m of s.modes) {
        expect(requirementOf(m), `${s.id}: mode ${m}`).toBe('any');
      }
    }
  });

  it('endpoint scenarios carry only endpoint-supported modes (no sdkprobe/apibench)', () => {
    const endpointFlows = ['endpoint', 'provision-endpoint'];
    for (const s of ALL_SCENARIOS.filter(s => endpointFlows.includes(s.flow))) {
      for (const m of s.modes) {
        expect(isModeSupported(m, { kind: 'endpoint' }), `${s.id}: mode ${m}`).toBe(true);
      }
    }
  });

  it('application scenarios reference a real RuntimeTemplate', () => {
    for (const s of ALL_SCENARIOS.filter(s => s.flow === 'provision-app')) {
      expect(s.presetId, `${s.id} presetId`).toBeDefined();
      expect(templateIds.has(s.presetId!), `${s.id} template ${s.presetId}`).toBe(true);
    }
  });
});

describe('scenario href routing', () => {
  const pid = 'proj-123';

  it('URL scenarios route to /probe with the preset param', () => {
    for (const s of ALL_SCENARIOS.filter(s => s.flow === 'url')) {
      const href = s.href(pid);
      expect(href, s.id).toContain(`/projects/${pid}/probe`);
      expect(href, s.id).toContain(`preset=${s.presetId}`);
    }
  });

  it('endpoint scenarios route to /tests/new with the modes param', () => {
    for (const s of ALL_SCENARIOS.filter(s => s.flow === 'endpoint')) {
      const href = s.href(pid);
      expect(href, s.id).toContain(`/projects/${pid}/tests/new`);
      expect(href, s.id).toContain(`modes=${s.modes.join(',')}`);
    }
  });

  it('full-stack scenarios route to the full-stack wizard with modes', () => {
    for (const s of ALL_SCENARIOS.filter(s => s.flow === 'provision-endpoint')) {
      const href = s.href(pid);
      expect(href, s.id).toContain(`/projects/${pid}/benchmarks/full-stack/new`);
      expect(href, s.id).toContain('modes=');
    }
  });

  it('application scenarios route to the application wizard with the template', () => {
    for (const s of ALL_SCENARIOS.filter(s => s.flow === 'provision-app')) {
      const href = s.href(pid);
      expect(href, s.id).toContain(`/projects/${pid}/benchmarks/application/new`);
      expect(href, s.id).toContain(`template=${s.presetId}`);
    }
  });
});

describe('auto-provisioning scenarios (Phase 2)', () => {
  const pid = 'proj-123';
  const autoProvision = ALL_SCENARIOS.filter(s => s.autoProvision);

  it('exist', () => {
    expect(autoProvision.length).toBeGreaterThan(0);
  });

  it('are only ever provisioning flows (never url/endpoint)', () => {
    for (const s of autoProvision) {
      expect(['provision-endpoint', 'provision-app'], s.id).toContain(s.flow);
    }
  });

  it('full-stack auto-provision scenarios carry proxies and the autoprovision flag', () => {
    for (const s of autoProvision.filter(s => s.flow === 'provision-endpoint')) {
      expect(s.proxies?.length, `${s.id} proxies`).toBeGreaterThan(0);
      const href = s.href(pid);
      expect(href, s.id).toContain('autoprovision=1');
      expect(href, s.id).toContain('proxies=');
    }
  });
});
