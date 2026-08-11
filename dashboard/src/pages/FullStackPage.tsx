import { useState, useEffect, useMemo, useRef } from 'react';
import { useAsyncEffect } from '../hooks/useAsyncEffect';
import { useSearchParams } from 'react-router';
import { api } from '../api/client';
import type { Workload, ModeGroup } from '../api/types';
import { buildComparisonCells as buildCells, countCells } from '../lib/matrix-cells';
import { WizardShell } from '../components/wizard/WizardShell';
import { TestbedMatrix } from '../components/wizard/TestbedMatrix';
import { WorkloadPanel } from '../components/wizard/WorkloadPanel';
import { MethodologyPanel } from '../components/wizard/MethodologyPanel';
import { ReviewStep } from '../components/wizard/ReviewStep';
import { ProvisioningNotice } from '../components/wizard/ProvisioningNotice';
import { useComparisonSubmit } from '../components/wizard/useComparisonSubmit';
import { unsupportedReason } from '../lib/mode-capabilities';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { testersApi } from '../api/testers';
import type { CloudAccountSummary, Methodology } from '../api/types';
import type { TestbedState } from '../components/wizard/testbed-constants';
import {
  methodologyForPreset,
  makeTestbed,
  REGIONS,
} from '../components/wizard/testbed-constants';

// Shared wizard chrome/submit live in components/wizard (WizardShell,
// ReviewStep, useComparisonSubmit) — this page owns only what makes a
// full-stack benchmark different: the Workload step, mode gating for
// endpoint targets, and the ?autoprovision= scenario prefill.

const STEPS = ['Testbeds', 'Workload', 'Methodology', 'Review'];

export function FullStackPage() {
  const { projectId } = useProject();
  usePageTitle('New Full Stack Benchmark');

  const [step, setStep] = useState(0);

  // Step 0: Testbeds
  const [testbeds, setTestbeds] = useState<TestbedState[]>([]);
  const [proxyWarning, setProxyWarning] = useState(false);
  const [runnerMode, setRunnerMode] = useState<'auto' | 'specific'>('auto');
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);

  // Step 1: Workload
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);
  // Seeded from ?modes= on first render instead of being written back by an
  // effect. The effect version rendered the defaults, then replaced them — a
  // visible flicker of the wrong selection — and needed a ref to fire once.
  const [rawSelectedModes, setSelectedModes] = useState<Set<string>>(() => {
    const param = new URLSearchParams(window.location.search).get('modes');
    const fromUrl = (param ?? '').split(',').map(m => m.trim()).filter(Boolean);
    return fromUrl.length > 0
      ? new Set(fromUrl)
      : new Set(['http1', 'http2', 'http3', 'download', 'upload']);
  });
  const [runs, setRuns] = useState(10);
  const [concurrency, setConcurrency] = useState(1);
  const [timeoutMs, setTimeoutMs] = useState(5000);
  const [selectedPayloads, setSelectedPayloads] = useState<Set<string>>(new Set());
  const [insecure, setInsecure] = useState(false);
  const [connectionReuse, setConnectionReuse] = useState(true);
  const [captureMode, setCaptureMode] = useState<'none' | 'tester' | 'endpoint' | 'both'>('none');

  const [searchParams] = useSearchParams();

  // A full-stack run always targets a provisioned networker-endpoint (a proxy
  // stack), so gate the mode picker to that target kind — greys out sdkprobe
  // (needs an SDK endpoint) and apibench (needs the Application Benchmark's
  // reference APIs), which would only ever fail here.
  const modeUnsupported = useMemo(
    () => (id: string) => unsupportedReason(id, { kind: 'endpoint' }),
    [],
  );

  // Defensively drop any unsupported mode (e.g. from a prefilled/saved config)
  // so a launch can never include one. DERIVED, not synced through an effect:
  // the effect version briefly held an invalid selection between render and
  // effect, and cascaded a render whenever support changed. `rawSelectedModes`
  // stays the user's intent so toggling a testbed back restores their picks.
  const selectedModes = useMemo(
    () => new Set([...rawSelectedModes].filter(m => modeUnsupported(m) === null)),
    [rawSelectedModes, modeUnsupported],
  );

  // Step 2: Methodology (always on). Seeded from the 'standard' preset so the
  // Review step always shows exactly what the highlighted preset says (F14).
  const [methodology, setMethodology] = useState<Methodology>(methodologyForPreset('standard') as Methodology);
  const [methodPreset, setMethodPreset] = useState<string>('standard');

  // Step 3: Review
  const [configName, setConfigName] = useState('');
  const [addSchedule, setAddSchedule] = useState(false);
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');

  // Infra for the auto-provisioning path + the review cost/runner notice.
  const [cloudAccounts, setCloudAccounts] = useState<CloudAccountSummary[]>([]);
  const [onlineRunners, setOnlineRunners] = useState(0);

  // ── Data loading ────────────────────────────────────────────────────

  useEffect(() => {
    api.getModes().then(r => setModeGroups(r.groups)).catch(() => {});
    api.getCloudAccounts(projectId).then(setCloudAccounts).catch(() => {});
    testersApi.listTesters(projectId)
      .then(rows => setOnlineRunners(rows.filter(t => t.power_state === 'running' && t.agent_status === 'online').length))
      .catch(() => {});
  }, [projectId]);

  // Auto-provisioning scenario (?autoprovision=1): pre-fill a default testbed
  // from the project's first cloud account (+ scenario ?proxies / ?os) and jump
  // straight to Review — the user reviews the provisioning notice and launches.
  // No-op (falls back to the manual testbed step) when no cloud account exists.
  const autoProvisionedRef = useRef(false);
  useAsyncEffect(() => {
    if (autoProvisionedRef.current) return;
    if (searchParams.get('autoprovision') !== '1') return;
    if (cloudAccounts.length === 0) return; // wait for load / nothing to pick
    autoProvisionedRef.current = true;

    const acct = cloudAccounts[0];
    const os = searchParams.get('os') === 'windows' ? 'windows' : 'linux';
    const proxies = (searchParams.get('proxies') ?? '')
      .split(',').map(p => p.trim()).filter(Boolean);
    const tb = makeTestbed(Date.now(), acct.provider, os, proxies.length ? proxies : ['nginx']);
    tb.cloudAccountId = acct.account_id;
    tb.region = acct.region_default || REGIONS[acct.provider]?.[0] || tb.region;
    setTestbeds([tb]);
    setConfigName(prev => prev || searchParams.get('name') || 'Full-stack comparison');
    setStep(3); // Review
  }, [searchParams, cloudAccounts]);

  // ── Navigation ──────────────────────────────────────────────────────

  const totalProxies = new Set(testbeds.flatMap(tb => tb.proxies)).size;

  const canNext = useMemo(() => {
    if (step === 0) {
      return (
        testbeds.length > 0 &&
        testbeds.every(c => c.proxies.length > 0) &&
        testbeds.every(c => c.cloudAccountId !== '')
      );
    }
    if (step === 1) return selectedModes.size > 0;
    return true;
  }, [step, testbeds, selectedModes.size]);

  // Say WHY Next is disabled — a silently grey button was audit §8.
  const nextHint = useMemo(() => {
    if (canNext || step >= 3) return null;
    if (step === 0) {
      if (testbeds.length === 0) return 'add a testbed to continue';
      if (testbeds.some(tb => tb.cloudAccountId === '')) return 'select a cloud account to continue';
      if (testbeds.some(tb => tb.proxies.length === 0)) return 'select at least one proxy per testbed';
    }
    if (step === 1) return 'select at least one mode';
    return null;
  }, [canNext, step, testbeds]);

  const goNext = () => {
    if (!canNext || step >= 3) return;
    if (step === 0) {
      const missingProxies = testbeds.some(tb => tb.proxies.length === 0);
      if (missingProxies) { setProxyWarning(true); return; }
      setProxyWarning(false);
    }
    setStep(step + 1);
  };

  // ── Submit ──────────────────────────────────────────────────────────

  // Fan out each testbed across its selected proxies (lib/matrix-cells,
  // unit-tested per audit P1-12). Any combination producing more than one
  // cell is a matrix run.
  const buildComparisonCells = () => buildCells(testbeds, selectedTesterId);
  const totalCells = countCells(testbeds);
  const isMatrixRun = totalCells > 1;

  // Name defaults to the placeholder when left blank — requiring a retype of
  // the suggested default left Launch silently disabled (E2E P3-11).
  const namePlaceholder = `Full stack benchmark ${new Date().toISOString().slice(0, 10)}`;
  const effectiveName = () => configName.trim() || namePlaceholder;

  const buildWorkload = (): Workload => {
    const sizeMap: Record<string, number> = { '64k': 65536, '1m': 1048576, '16m': 16777216 };
    const payloadSizes = [...selectedPayloads].map(s => sizeMap[s]).filter(Boolean);
    const captureModeMap: Record<string, string> = { none: 'metrics-only', tester: 'headers-only', endpoint: 'headers-only', both: 'full' };
    return {
      modes: [...selectedModes],
      runs,
      concurrency,
      timeout_ms: timeoutMs,
      payload_sizes: payloadSizes,
      capture_mode: (captureModeMap[captureMode] ?? 'headers-only') as Workload['capture_mode'],
      insecure: insecure || undefined,
      connection_reuse: connectionReuse || undefined,
    };
  };

  const { submitting, handleSubmit } = useComparisonSubmit({
    projectId,
    buildCells: buildComparisonCells,
    buildWorkload,
    methodology,
    effectiveName,
    addSchedule,
    cronExpr,
    selectedTesterId,
    isMatrixRun,
    emptyCellsError: 'At least one testbed with one proxy is required',
  });

  // ── Render ──────────────────────────────────────────────────────────

  return (
    <WizardShell
      breadcrumbLabel="Full Stack"
      breadcrumbTo={`/projects/${projectId}/runs`}
      title="New Full Stack Benchmark"
      subtitle="Test infrastructure stack performance through proxies with statistical methodology."
      steps={STEPS}
      step={step}
      onStepClick={setStep}
      canNext={canNext}
      nextHint={nextHint}
      onNext={goNext}
      onBack={() => step > 0 && setStep(step - 1)}
    >
      {step === 0 && (
        <TestbedMatrix
          projectId={projectId}
          testbeds={testbeds}
          onTestbedsChange={setTestbeds}
          runnerMode={runnerMode}
          onRunnerModeChange={setRunnerMode}
          selectedTesterId={selectedTesterId}
          onTesterIdChange={setSelectedTesterId}
          proxyWarning={proxyWarning}
        />
      )}

      {step === 1 && (
        <WorkloadPanel
          modeGroups={modeGroups}
          selectedModes={selectedModes}
          onModesChange={setSelectedModes}
          unsupported={modeUnsupported}
          runs={runs}
          onRunsChange={setRuns}
          concurrency={concurrency}
          onConcurrencyChange={setConcurrency}
          timeoutMs={timeoutMs}
          onTimeoutChange={setTimeoutMs}
          selectedPayloads={selectedPayloads}
          onPayloadsChange={setSelectedPayloads}
          insecure={insecure}
          onInsecureChange={setInsecure}
          connectionReuse={connectionReuse}
          onConnectionReuseChange={setConnectionReuse}
          captureMode={captureMode}
          onCaptureModeChange={setCaptureMode}
        />
      )}

      {step === 2 && (
        <MethodologyPanel
          alwaysOn
          benchmarkMode
          onBenchmarkModeChange={() => {}}
          methodology={methodology}
          onMethodologyChange={setMethodology}
          methodPreset={methodPreset}
          onMethodPresetChange={setMethodPreset}
        />
      )}

      {step === 3 && (
        <ReviewStep
          configName={configName}
          onConfigNameChange={setConfigName}
          namePlaceholder={namePlaceholder}
          summaryLine={
            <>
              {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''}
              {' / '}{totalProxies} prox{totalProxies !== 1 ? 'ies' : 'y'}
              {' / '}{[...selectedModes].join(', ')}
            </>
          }
          testbeds={testbeds}
          methodology={methodology}
          workloadLine={
            <>{runs} runs x {concurrency} concurrency / {timeoutMs}ms timeout / {[...selectedModes].join(' ')}</>
          }
          matrixNote={isMatrixRun
            ? <>Comparison group: {totalCells} cell{totalCells !== 1 ? 's' : ''} across {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''}</>
            : undefined}
          afterWorkload={
            // Provisioning cost + runner-readiness — the last check before spend
            <ProvisioningNotice
              vmCount={testbeds.length}
              cloud={new Set(testbeds.map(t => t.cloud)).size === 1 ? testbeds[0].cloud : 'multiple'}
              region={new Set(testbeds.map(t => t.region)).size === 1 ? testbeds[0].region : 'multiple'}
              onlineRunners={onlineRunners}
            />
          }
          addSchedule={addSchedule}
          onAddScheduleChange={setAddSchedule}
          cronExpr={cronExpr}
          onCronExprChange={setCronExpr}
          isMatrixRun={isMatrixRun}
          submitting={submitting}
          onSubmit={handleSubmit}
          launchLabel={isMatrixRun ? `Launch ${totalCells} Runs` : 'Launch Now'}
        />
      )}
    </WizardShell>
  );
}
