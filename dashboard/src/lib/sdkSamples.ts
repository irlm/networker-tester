import type {
  SdkSampleAction,
  SdkSampleCostPreview,
  SdkSampleState,
  SdkSampleStatus,
} from '../api/types';

/**
 * Pure decision helpers for the SDK-sample create flow — kept out of the page
 * so the interesting half is testable without rendering anything
 * (`sdkSamples.test.ts`).
 *
 * The server decides each language's state and recommended action
 * (`SdkSamplePlan` in the control plane); this module never re-derives them. It
 * only answers the questions the *form* has: what does the current selection
 * cost, how many servers does each shape need, and which languages will be
 * reused rather than provisioned.
 */

/** How the create should lay servers out. */
export type SdkSampleShape = 'consolidated' | 'separated';

export const SAMPLE_STATE_CHIP: Record<SdkSampleState, { label: string; className: string }> = {
  current: { label: 'deployed · current', className: 'text-emerald-400 border-emerald-500/30 bg-emerald-500/10' },
  outdated: { label: 'deployed · outdated', className: 'text-yellow-400 border-yellow-500/30 bg-yellow-500/10' },
  unknown_version: { label: 'deployed · version unknown', className: 'text-cyan-400 border-cyan-500/30 bg-cyan-500/10' },
  unhealthy: { label: 'unreachable', className: 'text-red-400 border-red-500/30 bg-red-500/10' },
  failed: { label: 'deploy failed', className: 'text-red-400 border-red-500/30 bg-red-500/10' },
  deploying: { label: 'deploying', className: 'text-cyan-400 border-cyan-500/30 bg-cyan-500/10' },
  none: { label: 'not deployed', className: 'text-gray-500 border-gray-700 bg-transparent' },
};

export const SAMPLE_ACTION_LABEL: Record<SdkSampleAction, string> = {
  reuse: 'Reuse',
  update: 'Update',
  redeploy: 'Redeploy',
  create: 'Deploy',
  wait: 'Deploying…',
};

/**
 * Version line for a row: "1.0.0" when deployed matches the catalog, and the
 * explicit "0.9.0 → 1.0.0" only when they differ. Never fabricates a deployed
 * version — an unreadable one renders as "?".
 */
export function versionLine(s: Pick<SdkSampleStatus, 'current_version' | 'deployed_version' | 'state'>): string {
  if (s.state === 'none' || s.state === 'deploying') return s.current_version;
  if (s.deployed_version === null) return `? → ${s.current_version}`;
  if (s.deployed_version === s.current_version) return s.current_version;
  return `${s.deployed_version} → ${s.current_version}`;
}

/** The plan a create request would execute, from the current form state. */
export interface SamplePlan {
  /** Selected languages whose existing server will be registered/kept. */
  reuse: string[];
  /** Selected languages that need a server provisioned. */
  provision: string[];
  /** Selected languages blocked by an in-flight deploy — nothing to do yet. */
  waiting: string[];
  /** Servers the create will start: 1 for consolidated, N for separated. */
  serverCount: number;
  /** True when the request would do nothing at all. */
  empty: boolean;
}

/**
 * Split the selection the way the server will (`SdkSamplePlan.Split`), so the
 * form's summary and the server's behaviour cannot disagree. `reuseExisting`
 * false is the explicit "provision anyway" escape hatch; it never silently
 * reuses.
 */
export function planFor(
  samples: readonly SdkSampleStatus[],
  selected: readonly string[],
  shape: SdkSampleShape,
  reuseExisting: boolean,
): SamplePlan {
  const byId = new Map(samples.map((s) => [s.language, s]));
  const reuse: string[] = [];
  const provision: string[] = [];
  const waiting: string[] = [];

  for (const lang of selected) {
    const s = byId.get(lang);
    if (s?.state === 'deploying') {
      waiting.push(lang);
    } else if (reuseExisting && s?.reusable) {
      reuse.push(lang);
    } else {
      provision.push(lang);
    }
  }

  const serverCount = provision.length === 0 ? 0 : shape === 'consolidated' ? 1 : provision.length;
  return { reuse, provision, waiting, serverCount, empty: reuse.length === 0 && provision.length === 0 };
}

/** Cost of a plan, or null when nothing priced the server. */
export interface SampleCost {
  hourlyUsd: number;
  monthlyUsd: number;
  /** What the reused languages avoid, priced as separated servers — the
   * honest ceiling of the saving, since reuse also avoids the consolidated
   * server when nothing else needs provisioning. */
  avoidedMonthlyUsd: number;
}

export function costFor(plan: SamplePlan, price: SdkSampleCostPreview | null): SampleCost | null {
  if (!price) return null;
  return {
    hourlyUsd: plan.serverCount * price.hourly_usd,
    monthlyUsd: plan.serverCount * price.monthly_usd,
    avoidedMonthlyUsd: plan.reuse.length * price.monthly_usd,
  };
}

/** `$0.096/h` — trailing zeros kept so a column of prices lines up. */
export function usdPerHour(value: number): string {
  return `$${value.toFixed(3)}/h`;
}

/** `$69.12/mo`. */
export function usdPerMonth(value: number): string {
  return `$${value.toFixed(2)}/mo`;
}

/**
 * What the create button should say for the current plan — reuse-only requests
 * provision nothing, and saying "Deploy 0 servers" would be a lie.
 */
export function submitLabel(plan: SamplePlan): string {
  if (plan.empty) return 'Select a language';
  if (plan.serverCount === 0) return `Reuse ${plan.reuse.length} existing`;
  const servers = `${plan.serverCount} server${plan.serverCount === 1 ? '' : 's'}`;
  return plan.reuse.length > 0
    ? `Deploy ${servers} · reuse ${plan.reuse.length}`
    : `Deploy ${servers}`;
}

/**
 * The default selection: every language that is not already current. A user
 * who opens the dialog wants the gaps filled; pre-ticking a language that is
 * already serving the current sample just invites an accidental redeploy.
 * Falls back to every language when nothing is missing, so the dialog is never
 * empty.
 */
export function defaultSelection(samples: readonly SdkSampleStatus[]): string[] {
  const missing = samples.filter((s) => s.state !== 'current').map((s) => s.language);
  return missing.length > 0 ? missing : samples.map((s) => s.language);
}

/** Languages the page should offer an in-place update for. */
export function outdated(samples: readonly SdkSampleStatus[]): SdkSampleStatus[] {
  return samples.filter((s) => s.state === 'outdated');
}

/** True when the project has at least one usable sample already. */
export function anyReusable(samples: readonly SdkSampleStatus[]): boolean {
  return samples.some((s) => s.reusable);
}

/**
 * Languages that are deployed and healthy but have no SDK endpoint row yet —
 * the "your server is up, one click to make it probeable" case that a create
 * leaves behind while its deployment finishes.
 */
export function unregistered(samples: readonly SdkSampleStatus[]): SdkSampleStatus[] {
  return samples.filter((s) => s.reusable && s.sdk_endpoint_id === null);
}
