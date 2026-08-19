import { describe, expect, it } from 'vitest';
import type { CloudAccountSummary } from '../../api/types';
import {
  DEFAULT_METHODOLOGY,
  languageAllowedOnOs,
  INSTANCE_TYPES,
  makeTestbed,
  makeTestbedForAccount,
  METHODOLOGY_PRESETS,
  methodologyForPreset,
  nextTestbedKey,
  pickDefaultAccount,
  providerToCloud,
  REGIONS,
  unhealthyAccountLaunchBlock,
  updateTestbedState,
  windowsProxiesFor,
  WINDOWS_PROXIES_AZURE,
} from './testbed-constants';

describe('methodologyForPreset (audit F14 — Review must match Methodology)', () => {
  it('seeds Standard with the numbers the Standard card advertises (10/50/5%)', () => {
    const std = METHODOLOGY_PRESETS.find(p => p.id === 'standard')!;
    const m = methodologyForPreset('standard');
    expect(m.warmup_runs).toBe(std.warmup);
    expect(m.measured_runs).toBe(std.measured);
    expect(m.target_error_pct).toBe(std.targetError);
    // The regression: wizards defaulted to DEFAULT_METHODOLOGY (5/30/2%)
    // while highlighting Standard (10/50/5%).
    expect(m.warmup_runs).toBe(10);
    expect(m.measured_runs).toBe(50);
    expect(m.target_error_pct).toBe(5);
  });

  it('maps every preset id to its own advertised numbers', () => {
    for (const p of METHODOLOGY_PRESETS) {
      const m = methodologyForPreset(p.id);
      expect(m.warmup_runs).toBe(p.warmup);
      expect(m.measured_runs).toBe(p.measured);
      expect(m.target_error_pct).toBe(p.targetError ?? 0);
    }
  });

  it('preserves the non-preset methodology fields (gates, outlier policy)', () => {
    const m = methodologyForPreset('standard');
    expect(m.cooldown_ms).toBe(DEFAULT_METHODOLOGY.cooldown_ms);
    expect(m.quality_gates).toEqual(DEFAULT_METHODOLOGY.quality_gates);
    expect(m.publication_gates).toEqual(DEFAULT_METHODOLOGY.publication_gates);
    expect(m.outlier_policy).toEqual(DEFAULT_METHODOLOGY.outlier_policy);
  });

  it('falls back to defaults for unknown preset ids', () => {
    expect(methodologyForPreset('nope')).toEqual(DEFAULT_METHODOLOGY);
  });
});

// ── Windows stack support ⇄ server launch gate (v0.28.147) ──────────────────
// The server rejects windows·haproxy and windows·apache at launch
// (ComparisonGroupsEndpoints.UnsupportedComboReason — no native/scriptable
// Windows binary sources, verified 2026-08-04). The UI must not offer what
// the server will refuse. C# pins the same pairs from its side
// (UnsupportedComboTests) — change either list only together.
describe('windows proxy support mirrors the server launch gate', () => {
  it('never offers haproxy or apache on any windows cloud', () => {
    for (const cloud of ['Azure', 'AWS', 'GCP']) {
      const offered = windowsProxiesFor(cloud);
      expect(offered).not.toContain('haproxy');
      expect(offered).not.toContain('apache');
    }
  });

  it('azure windows offers exactly the supported trio', () => {
    expect([...WINDOWS_PROXIES_AZURE].sort()).toEqual(['caddy', 'iis', 'traefik']);
  });
});

describe('nextTestbedKey (duplicate-key regression)', () => {
  it('starts at 0 and always allocates above the current max', () => {
    expect(nextTestbedKey([])).toBe(0);
    const a = makeTestbed(0);
    const b = makeTestbed(nextTestbedKey([a]));
    expect(b.key).toBe(1);
    // Removing a LOW key must not cause reuse of a live key — the old
    // length-derived counter did exactly that after remove + remount,
    // making two rows patch together in updateTestbedState.
    const afterRemove = [b]; // removed key 0, length is 1 again
    expect(nextTestbedKey(afterRemove)).toBe(2);
  });

  it('updateTestbedState patches exactly one row when keys are unique', () => {
    const rows = [makeTestbed(0), makeTestbed(1)];
    const next = updateTestbedState(rows, 1, { os: 'windows' });
    expect(next[0].os).toBe('linux');
    expect(next[1].os).toBe('windows');
  });
});

// ── Autoprovision account/provider helpers (#791 / #793 P1-1, P1-2) ─────────

describe('providerToCloud (wire provider → REGIONS/INSTANCE_TYPES key)', () => {
  it('maps every wire provider to a key that resolves in both tables', () => {
    for (const [wire, cloud] of [
      ['azure', 'Azure'],
      ['aws', 'AWS'],
      ['gcp', 'GCP'],
      ['docker', 'Docker'],
    ] as const) {
      expect(providerToCloud(wire)).toBe(cloud);
      expect(REGIONS[providerToCloud(wire)]?.length).toBeGreaterThan(0);
      expect(INSTANCE_TYPES[providerToCloud(wire)]?.length).toBeGreaterThan(0);
    }
  });

  it('is case-insensitive and falls back to Azure for unknown providers', () => {
    expect(providerToCloud('AWS')).toBe('AWS');
    expect(providerToCloud('Azure')).toBe('Azure');
    expect(providerToCloud('openstack')).toBe('Azure');
  });

  it('the raw wire value NEVER resolves — the #793 P1-2 bug shape', () => {
    // Feeding the lowercase wire provider straight into the tables misses,
    // which is exactly what produced Standard_B2s-on-AWS + empty region.
    expect(REGIONS['aws']).toBeUndefined();
    expect(INSTANCE_TYPES['gcp']).toBeUndefined();
  });
});

function account(over: Partial<CloudAccountSummary> = {}): CloudAccountSummary {
  return {
    account_id: 'acct-1',
    name: 'Acct',
    provider: 'azure',
    region_default: null,
    personal: false,
    status: 'active',
    last_validated: null,
    validation_error: null,
    ...over,
  };
}

describe('pickDefaultAccount (healthy-first default, #791)', () => {
  it('skips an alphabetically-first error account in favor of a healthy one', () => {
    const aws = account({ account_id: 'a', name: 'AWS', provider: 'aws', status: 'error', validation_error: 'Invalid access key ID' });
    const azure = account({ account_id: 'b', name: 'Azure', provider: 'azure', status: 'active' });
    // Server order is by name — AWS first, exactly the incident shape.
    expect(pickDefaultAccount([aws, azure])?.account_id).toBe('b');
  });

  it('prefers provider/name order among multiple active accounts', () => {
    const gcp = account({ account_id: 'g', name: 'GCP', provider: 'gcp' });
    const aws = account({ account_id: 'a', name: 'AWS', provider: 'aws' });
    expect(pickDefaultAccount([gcp, aws])?.account_id).toBe('a');
  });

  it('returns null when every account is unhealthy or the list is empty', () => {
    expect(pickDefaultAccount([])).toBeNull();
    expect(pickDefaultAccount([
      account({ status: 'error' }),
      account({ account_id: 'x', status: 'validating' }),
    ])).toBeNull();
  });

  it('does not mutate the caller-owned list', () => {
    const list = [
      account({ account_id: 'z', name: 'Z', status: 'error' }),
      account({ account_id: 'a', name: 'A' }),
    ];
    pickDefaultAccount(list);
    expect(list[0].account_id).toBe('z');
  });
});

describe('makeTestbedForAccount (the autoprovision prefill path)', () => {
  it('an AWS account yields AWS regions and an AWS SKU — not Standard_B2s', () => {
    const tb = makeTestbedForAccount(1, account({ provider: 'aws' }), 'linux', ['nginx']);
    expect(tb.cloud).toBe('AWS');
    expect(tb.region).toBe('us-east-1');
    expect(tb.vmSize).toBe('t3.small');
    expect(tb.vmSize).not.toMatch(/^Standard_/);
    expect(tb.region).not.toBe('');
  });

  it('honors region_default only when valid for the provider', () => {
    const good = makeTestbedForAccount(1, account({ provider: 'gcp', region_default: 'europe-west1' }));
    expect(good.region).toBe('europe-west1');
    // An Azure-region default on a GCP account must not leak through.
    const bad = makeTestbedForAccount(1, account({ provider: 'gcp', region_default: 'eastus' }));
    expect(bad.region).toBe(REGIONS.GCP[0]);
  });

  it('carries the account id, os and proxies onto the testbed', () => {
    const tb = makeTestbedForAccount(7, account({ account_id: 'acc-42', provider: 'azure' }), 'windows', ['iis']);
    expect(tb.key).toBe(7);
    expect(tb.cloudAccountId).toBe('acc-42');
    expect(tb.os).toBe('windows');
    expect(tb.proxies).toEqual(['iis']);
  });
});

describe('unhealthyAccountLaunchBlock (Review launch gate, #793 P2-4)', () => {
  const err = account({ account_id: 'bad', name: 'AWS prod', status: 'error', validation_error: 'Invalid access key ID' });
  const ok = account({ account_id: 'good', name: 'Azure' });

  it('blocks with the account name, status and validation error', () => {
    const tb = makeTestbed(0, 'AWS', 'linux', ['nginx']);
    tb.cloudAccountId = 'bad';
    const reason = unhealthyAccountLaunchBlock([tb], [err, ok]);
    expect(reason).toContain("cloud account 'AWS prod' is in error state");
    expect(reason).toContain('Invalid access key ID');
  });

  it('passes when every selected account is active', () => {
    const tb = makeTestbed(0, 'Azure', 'linux', ['nginx']);
    tb.cloudAccountId = 'good';
    expect(unhealthyAccountLaunchBlock([tb], [err, ok])).toBeNull();
  });

  it('ignores docker/unselected testbeds (empty cloudAccountId)', () => {
    const tb = makeTestbed(0, 'Docker', 'linux', ['nginx']);
    expect(unhealthyAccountLaunchBlock([tb], [err])).toBeNull();
  });

  it('omits the detail suffix when there is no validation error', () => {
    const tb = makeTestbed(0, 'AWS', 'linux', ['nginx']);
    tb.cloudAccountId = 'v';
    const reason = unhealthyAccountLaunchBlock([tb], [account({ account_id: 'v', name: 'V', status: 'validating' })]);
    expect(reason).toContain("cloud account 'V' is in validating state — fix credentials");
  });
});

describe('languageAllowedOnOs (net48-on-linux regression)', () => {
  it('never allows Windows-only runtimes on a Linux testbed', () => {
    expect(languageAllowedOnOs('csharp-net48', 'linux')).toBe(false);
    expect(languageAllowedOnOs('csharp-net48', 'windows')).toBe(true);
  });
  it('allows cross-platform runtimes everywhere, and the empty language', () => {
    expect(languageAllowedOnOs('csharp-net8', 'linux')).toBe(true);
    expect(languageAllowedOnOs('go', 'windows')).toBe(true);
    expect(languageAllowedOnOs('', 'linux')).toBe(true);
  });
});
