import { useMemo, useState } from 'react';
import { api, errorMessage } from '../api/client';
import type { CloudAccountSummary, SdkSampleCostPreview, SdkSampleStatus } from '../api/types';
import { useAsyncEffect } from '../hooks/useAsyncEffect';
import { DOCKER_CLOUD, DOCKER_LABEL, DOCKER_REGION, useDockerProvider } from '../hooks/useDockerProvider';
import { useToast } from '../hooks/useToast';
import { cloudProviderText } from '../lib/provider';
import {
  costFor,
  defaultSelection,
  planFor,
  SAMPLE_STATE_CHIP,
  submitLabel,
  usdPerHour,
  usdPerMonth,
  versionLine,
  type SdkSampleShape,
} from '../lib/sdkSamples';
import { INSTANCE_TYPES, REGIONS, defaultInstanceType } from './wizard/testbed-constants';
import { Button } from './common/Button';
import { FormField, Select } from './common/FormControls';
import { Modal } from './common/Modal';

interface CreateSdkSamplesDialogProps {
  projectId: string;
  samples: SdkSampleStatus[];
  onClose: () => void;
  onCreated: () => void;
}

/** Provider → wizard cloud key for REGIONS / INSTANCE_TYPES. */
const CLOUD_KEY: Record<string, string> = { azure: 'Azure', aws: 'AWS', gcp: 'GCP' };

/**
 * Provision the LagHound SDK samples.
 *
 * Two shapes, one selection, one honest price:
 *
 *  - **Consolidated** — one server hosting every selected language (each on its
 *    catalog port). One VM, one bill. The default, because it is the cheap one.
 *  - **Separated** — one server per language: isolation, and per-language
 *    infrastructure numbers, at N× the cost.
 *
 * Whatever is already deployed and usable is REUSED rather than provisioned
 * again — the summary says how many servers that avoids and what they would
 * have cost. Reuse can be turned off explicitly, never silently.
 */
export function CreateSdkSamplesDialog({
  projectId,
  samples,
  onClose,
  onCreated,
}: CreateSdkSamplesDialogProps) {
  const dockerAvailable = useDockerProvider();
  const addToast = useToast();

  const [shape, setShape] = useState<SdkSampleShape>('consolidated');
  const [selected, setSelected] = useState<string[]>(() => defaultSelection(samples));
  const [reuseExisting, setReuseExisting] = useState(true);
  const [provider, setProvider] = useState('azure');
  const [region, setRegion] = useState('eastus');
  const [vmSize, setVmSize] = useState(() => defaultInstanceType('Azure'));
  const [accounts, setAccounts] = useState<CloudAccountSummary[]>([]);
  const [accountId, setAccountId] = useState('');
  const [price, setPrice] = useState<SdkSampleCostPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useAsyncEffect(async (cancelled) => {
    try {
      const rows = await api.getCloudAccounts(projectId);
      if (!cancelled()) setAccounts(rows);
    } catch {
      // Cloud accounts are optional here (docker needs none); the submit
      // surfaces a real error if the provider actually requires one.
    }
  }, [projectId]);

  // Price ONE server for the current provider/region/size. Same table the
  // deployment cost endpoint uses — never a second price list in the client.
  useAsyncEffect(async (cancelled) => {
    try {
      const res = await api.getSdkSamples(projectId, { provider, region, vmSize });
      if (!cancelled()) setPrice(res.cost_preview);
    } catch {
      if (!cancelled()) setPrice(null);
    }
  }, [projectId, provider, region, vmSize]);

  const isDocker = provider === DOCKER_CLOUD;
  const cloudKey = CLOUD_KEY[provider] ?? 'Azure';
  const plan = useMemo(
    () => planFor(samples, selected, shape, reuseExisting),
    [samples, selected, shape, reuseExisting],
  );
  const cost = costFor(plan, price);
  const eligibleAccounts = accounts.filter((a) => a.provider === provider);

  const onProviderChange = (next: string) => {
    setProvider(next);
    setAccountId('');
    if (next === DOCKER_CLOUD) {
      setRegion(DOCKER_REGION);
      setVmSize('container');
      return;
    }
    const key = CLOUD_KEY[next] ?? 'Azure';
    setRegion(REGIONS[key]?.[0] ?? '');
    setVmSize(defaultInstanceType(key));
  };

  const toggle = (language: string) => {
    setSelected((prev) =>
      prev.includes(language) ? prev.filter((l) => l !== language) : [...prev, language],
    );
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (plan.empty) {
      setError('Select at least one language.');
      return;
    }
    if (plan.serverCount > 0 && !isDocker && eligibleAccounts.length === 0) {
      setError(`No ${provider} cloud account in this project — add one before provisioning a server.`);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const result = await api.createSdkSamples(projectId, {
        shape,
        languages: selected,
        reuse_existing: reuseExisting,
        ...(plan.serverCount > 0
          ? {
              provider,
              region: isDocker ? DOCKER_REGION : region,
              vm_size: isDocker ? 'container' : vmSize,
              ...(accountId ? { cloud_account_id: accountId } : {}),
            }
          : {}),
      });
      const parts: string[] = [];
      if (result.reused.length > 0) parts.push(`reused ${result.reused.length}`);
      if (result.servers_provisioned > 0) {
        parts.push(`${result.servers_provisioned} server${result.servers_provisioned === 1 ? '' : 's'} deploying`);
      }
      addToast('success', `SDK samples: ${parts.join(', ') || 'nothing to do'}`);
      onCreated();
      onClose();
    } catch (err) {
      const msg = errorMessage(err);
      setError(msg);
      addToast('error', msg);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Modal
      onClose={onClose}
      labelledBy="create-sdk-samples-title"
      variant="slide-over"
      panelClassName="md:!w-[640px] md:!max-w-[95vw]"
    >
      <form onSubmit={handleSubmit} className="p-4 md:p-6" noValidate>
        <div className="mb-2 flex items-center justify-between">
          <h3 id="create-sdk-samples-title" className="text-lg font-bold text-gray-100">
            Deploy SDK samples
          </h3>
          <Button variant="ghost" size="xs" onClick={onClose} aria-label="Close">&#x2715;</Button>
        </div>
        <p className="mb-6 text-xs text-gray-400">
          Provision the LagHound reference apps, then register each as an SDK endpoint so{' '}
          <span className="text-cyan-400">sdkprobe</span> can split its latency into network versus
          application time.
        </p>

        {error && <div className="alert alert-error mb-4 text-sm text-red-300" role="alert">{error}</div>}

        {/* ── Shape ─────────────────────────────────────────────────── */}
        <fieldset className="mb-5">
          <legend className="field-label mb-2">Layout</legend>
          <div className="grid gap-2 sm:grid-cols-2">
            <ShapeCard
              id="consolidated"
              title="Consolidated"
              detail="One server hosts every selected language, each on its own port. Cheapest."
              selected={shape === 'consolidated'}
              onSelect={() => setShape('consolidated')}
            />
            <ShapeCard
              id="separated"
              title="Separated"
              detail="One server per language. Isolation and per-language infrastructure numbers, at N× the cost."
              selected={shape === 'separated'}
              onSelect={() => setShape('separated')}
            />
          </div>
        </fieldset>

        {/* ── Languages ─────────────────────────────────────────────── */}
        <fieldset className="mb-5">
          <legend className="field-label mb-2">Languages</legend>
          <ul className="divide-y divide-gray-800 rounded border border-gray-800">
            {samples.map((s) => {
              const chip = SAMPLE_STATE_CHIP[s.state];
              const checked = selected.includes(s.language);
              return (
                <li key={s.language} className="flex items-start gap-3 px-3 py-2">
                  <input
                    type="checkbox"
                    id={`sdk-sample-${s.language}`}
                    checked={checked}
                    onChange={() => toggle(s.language)}
                    className="mt-1"
                  />
                  <label htmlFor={`sdk-sample-${s.language}`} className="min-w-0 flex-1 cursor-pointer">
                    <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      <span className="text-sm text-gray-200">{s.label}</span>
                      <span className="text-xs text-faint">{s.runtime}</span>
                      <span className={`rounded border px-1.5 py-0.5 text-xs ${chip.className}`}>
                        {chip.label}
                      </span>
                      <span className="text-xs text-faint">SDK {versionLine(s)}</span>
                    </span>
                    <span className="mt-0.5 block text-xs text-gray-500">
                      :{s.port} · {s.reason}
                    </span>
                  </label>
                </li>
              );
            })}
          </ul>
          <label className="mt-2 flex items-center gap-2 text-xs text-gray-400">
            <input
              type="checkbox"
              checked={reuseExisting}
              onChange={(e) => setReuseExisting(e.target.checked)}
            />
            Reuse servers that already run a usable sample (recommended — avoids paying twice)
          </label>
        </fieldset>

        {/* ── Where ─────────────────────────────────────────────────── */}
        {plan.serverCount > 0 && (
          <fieldset className="mb-5 space-y-4">
            <legend className="field-label mb-2">Where to provision</legend>

            <FormField label="Provider" htmlFor="sdk-sample-provider">
              <Select value={provider} onChange={(e) => onProviderChange(e.target.value)}>
                <option value="azure">Azure</option>
                <option value="aws">AWS</option>
                <option value="gcp">GCP</option>
                {dockerAvailable && <option value={DOCKER_CLOUD}>{DOCKER_LABEL}</option>}
              </Select>
            </FormField>

            {!isDocker && (
              <>
                <FormField
                  label="Cloud account"
                  htmlFor="sdk-sample-account"
                  hint={
                    eligibleAccounts.length === 0
                      ? `No ${provider} account in this project yet.`
                      : undefined
                  }
                >
                  <Select value={accountId} onChange={(e) => setAccountId(e.target.value)}>
                    <option value="">Default for this project</option>
                    {eligibleAccounts.map((a) => (
                      <option key={a.account_id} value={a.account_id}>
                        {a.name} ({a.status})
                      </option>
                    ))}
                  </Select>
                </FormField>

                <FormField label="Region" htmlFor="sdk-sample-region">
                  <Select value={region} onChange={(e) => setRegion(e.target.value)}>
                    {(REGIONS[cloudKey] ?? []).map((r) => (
                      <option key={r} value={r}>{r}</option>
                    ))}
                  </Select>
                </FormField>

                <FormField label="Instance size" htmlFor="sdk-sample-size">
                  <Select value={vmSize} onChange={(e) => setVmSize(e.target.value)}>
                    {(INSTANCE_TYPES[cloudKey] ?? []).map((t) => (
                      <option key={t.id} value={t.id}>{t.id} · {t.hint}</option>
                    ))}
                  </Select>
                </FormField>
              </>
            )}
          </fieldset>
        )}

        {/* ── What this will do ─────────────────────────────────────── */}
        <div className="mb-6 rounded border border-gray-800 bg-[var(--bg-surface)] p-3 text-xs">
          <div className="mb-1.5 text-gray-400">This will</div>
          <ul className="space-y-1 text-gray-300">
            <li>
              <span className="text-gray-500">provision</span>{' '}
              {plan.serverCount === 0
                ? 'nothing — every selected language already has a usable server'
                : `${plan.serverCount} server${plan.serverCount === 1 ? '' : 's'} for ${plan.provision.join(', ')}`}
              {plan.serverCount > 0 && (
                <span className={`ml-2 ${cloudProviderText(provider)}`}>
                  {provider}
                  {!isDocker && ` ${region} · ${vmSize}`}
                </span>
              )}
            </li>
            {plan.reuse.length > 0 && (
              <li>
                <span className="text-gray-500">reuse</span>{' '}
                <span className="text-emerald-400">{plan.reuse.join(', ')}</span>
                {cost && cost.avoidedMonthlyUsd > 0 && (
                  <span className="text-gray-500"> — avoids {usdPerMonth(cost.avoidedMonthlyUsd)}</span>
                )}
              </li>
            )}
            {plan.waiting.length > 0 && (
              <li>
                <span className="text-gray-500">skip</span>{' '}
                <span className="text-cyan-400">{plan.waiting.join(', ')}</span>
                <span className="text-gray-500"> — a deployment is already running</span>
              </li>
            )}
            {cost && plan.serverCount > 0 && (
              <li className="pt-1 text-gray-400">
                <span className="text-gray-500">cost</span>{' '}
                <span className="text-cyan-400">
                  {usdPerHour(cost.hourlyUsd)} · {usdPerMonth(cost.monthlyUsd)}
                </span>
                <span className="text-gray-600"> ({price?.note})</span>
              </li>
            )}
          </ul>
        </div>

        <div className="flex justify-end gap-3 border-t border-gray-800/50 pt-4">
          <Button onClick={onClose}>Cancel</Button>
          <Button
            type="submit"
            variant="primary"
            disabled={plan.empty || loading}
            loading={loading}
            loadingLabel="Starting…"
          >
            {submitLabel(plan)}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function ShapeCard({
  id,
  title,
  detail,
  selected,
  onSelect,
}: {
  id: string;
  title: string;
  detail: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={selected}
      onClick={onSelect}
      className={`rounded border p-3 text-left transition-colors ${
        selected
          ? 'border-cyan-500/60 bg-cyan-500/10'
          : 'border-gray-800 bg-[var(--bg-surface)] hover:border-gray-700'
      }`}
    >
      <div className={`text-sm font-semibold ${selected ? 'text-cyan-300' : 'text-gray-200'}`} data-shape={id}>
        {title}
      </div>
      <div className="mt-1 text-xs text-gray-400">{detail}</div>
    </button>
  );
}
