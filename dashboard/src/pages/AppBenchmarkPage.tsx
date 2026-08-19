import { useState, useCallback, useMemo, useEffect, useRef } from 'react';
import { useAsyncEffect } from '../hooks/useAsyncEffect';
import { useSearchParams } from 'react-router';
import { api } from '../api/client';
import type { Workload, Methodology, ComparisonCell, LanguageCapability, CloudAccountSummary } from '../api/types';
import { WizardShell } from '../components/wizard/WizardShell';
import { TestbedMatrix } from '../components/wizard/TestbedMatrix';
import { MethodologyPanel } from '../components/wizard/MethodologyPanel';
import { LanguageSelector } from '../components/wizard/LanguageSelector';
import { ReviewStep } from '../components/wizard/ReviewStep';
import { ProvisioningNotice } from '../components/wizard/ProvisioningNotice';
import { useComparisonSubmit } from '../components/wizard/useComparisonSubmit';
import { h3DropsPerCell } from '../lib/matrix-cells';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { testersApi } from '../api/testers';
import { isOnlineTester } from '../lib/tester-readiness';
import type { TestbedState } from '../components/wizard/testbed-constants';
import {
  methodologyForPreset,
  RUNTIME_TEMPLATES,
  LANGUAGE_GROUPS,
  WINDOWS_PROXIES,
  languageAllowedOnOs,
  requiresWindows,
  makeTestbed,
  resolveVmSize,
  resolveTopology,
  unhealthyAccountLaunchBlock,
  type RuntimeTemplate,
} from '../components/wizard/testbed-constants';

// Shared wizard chrome/submit live in components/wizard (WizardShell,
// ReviewStep, useComparisonSubmit) — this page owns only what makes an
// application benchmark different: the Template and Languages steps, the
// language-capability gating, and the language×testbed×proxy cell fan-out.

const STEPS = ['Template', 'Testbeds', 'Languages', 'Methodology', 'Review'];

export function AppBenchmarkPage() {
  const { projectId } = useProject();
  usePageTitle('New Application Benchmark');

  const [searchParams] = useSearchParams();
  const [step, setStep] = useState(0);

  // Step 0: Template
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);

  // Step 1: Testbeds
  const [testbeds, setTestbeds] = useState<TestbedState[]>([]);
  const [proxyWarning, setProxyWarning] = useState(false);
  const [runnerMode, setRunnerMode] = useState<'auto' | 'specific'>('auto');
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);

  // Step 2: Languages
  const [rawSelectedLangs, setSelectedLangs] = useState<Set<string>>(new Set(['nginx']));

  // Workload (configured by template, not a separate step)
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['http1', 'http2', 'http3', 'download', 'upload']));
  const runs = 10;
  const concurrency = 1;
  const timeoutMs = 5000;

  // Cloud accounts — the Review step shows each testbed's account name +
  // status and blocks Launch on a non-active account (#793 P2-4).
  const [cloudAccounts, setCloudAccounts] = useState<CloudAccountSummary[]>([]);
  // Online runners (STRICT: running VM + connected agent) for the Review
  // step's provisioning notice — this wizard has the largest VM fan-out
  // (languages × testbeds × proxies) and had no cost/runner notice (#793 P2-2).
  const [onlineRunners, setOnlineRunners] = useState(0);
  useEffect(() => {
    api.getCloudAccounts(projectId).then(setCloudAccounts).catch(() => {});
    testersApi.listTesters(projectId)
      .then(rows => setOnlineRunners(rows.filter(isOnlineTester).length))
      .catch(() => {});
  }, [projectId]);

  // Language capability matrix (GET /api/modes → language_capabilities).
  // undefined = not loaded / unsupported control plane → no gating.
  const [capabilities, setCapabilities] = useState<LanguageCapability[] | undefined>(undefined);
  useEffect(() => {
    let cancelled = false;
    api.getModes()
      .then(res => { if (!cancelled) setCapabilities(res.language_capabilities); })
      .catch(() => { /* degrade open — selector shows no capability tags */ });
    return () => { cancelled = true; };
  }, []);

  // apibench × language gating: languages without the /api/* suite (nginx)
  // cannot appear in an apibench selection (audit C5).
  // DERIVED rather than synced through an effect: the effect version held an
  // invalid selection between render and effect and cascaded a render whenever
  // capabilities loaded. `rawSelectedLangs` keeps the user's intent, so a
  // language that becomes supported again reappears instead of being lost.
  const selectedLangs = useMemo(() => {
    if (!capabilities || !selectedModes.has('apibench')) return rawSelectedLangs;
    const unsupported = new Set(capabilities.filter(c => !c.apibench).map(c => c.language));
    const pruned = [...rawSelectedLangs].filter(l => !unsupported.has(l));
    return pruned.length === rawSelectedLangs.size ? rawSelectedLangs : new Set(pruned);
  }, [capabilities, selectedModes, rawSelectedLangs]);

  // Step 3: Methodology (always on). Seeded from the 'standard' preset so the
  // Review step always matches the highlighted preset (audit F14).
  const [methodology, setMethodology] = useState<Methodology>(methodologyForPreset('standard') as Methodology);
  const [methodPreset, setMethodPreset] = useState<string>('standard');

  // Step 4: Review
  const [configName, setConfigName] = useState('');
  const [addSchedule, setAddSchedule] = useState(false);
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');

  // ── Template application ────────────────────────────────────────────

  const applyTemplate = useCallback((tmpl: RuntimeTemplate) => {
    setSelectedTemplate(tmpl.id);
    setSelectedLangs(new Set(tmpl.defaultLanguages));
    setSelectedModes(new Set(tmpl.defaultModes));

    // Pre-fill testbeds
    const newTestbeds: TestbedState[] = [];
    if (tmpl.id !== 'custom' && tmpl.defaultTestbedCount > 0) {
      // Wholesale replace — key 0 is always free in the new array; subsequent
      // "+ add testbed" rows get max+1 via nextTestbedKey.
      newTestbeds.push(makeTestbed(0, 'Azure', tmpl.defaultOs ?? 'linux', tmpl.defaultProxies));
    }
    setTestbeds(newTestbeds);

    // Methodology preset
    const presetMap: Record<string, { warmup: number; measured: number; targetError: number | null }> = {
      quick: { warmup: 5, measured: 10, targetError: null },
      standard: { warmup: 10, measured: 50, targetError: 5 },
      rigorous: { warmup: 10, measured: 200, targetError: 2 },
    };
    const p = presetMap[tmpl.methodology];
    if (p) {
      setMethodPreset(tmpl.methodology);
      setMethodology(m => ({
        ...m,
        warmup_runs: p.warmup,
        measured_runs: p.measured,
        target_error_pct: p.targetError ?? 0,
      }));
    }

    setProxyWarning(false);
    setStep(1);
  }, []);

  // Prefill from ?template= (scenario launcher): apply the named RuntimeTemplate
  // once on mount, which seeds langs/modes/testbeds/methodology and advances to
  // step 1. No-op if the param is missing or unknown.
  const prefilledRef = useRef(false);
  useAsyncEffect(() => {
    if (prefilledRef.current) return;
    const templateId = searchParams.get('template');
    if (!templateId) return;
    const tmpl = RUNTIME_TEMPLATES.find(t => t.id === templateId);
    if (tmpl) {
      prefilledRef.current = true;
      applyTemplate(tmpl);
    }
  }, [searchParams, applyTemplate]);

  // ── Navigation ──────────────────────────────────────────────────────

  const totalProxies = new Set(testbeds.flatMap(tb => tb.proxies)).size;

  const canNext = useMemo(() => {
    if (step === 0) return selectedTemplate !== null;
    if (step === 1) {
      return (
        testbeds.length > 0 &&
        testbeds.every(c => c.proxies.length > 0) &&
        testbeds.every(c => c.cloudAccountId !== '')
      );
    }
    if (step === 2) return selectedLangs.size > 0;
    return true;
  }, [step, selectedTemplate, testbeds, selectedLangs.size]);

  const nextHint = useMemo(() => {
    if (canNext || step >= 4) return null;
    if (step === 0) return 'pick a template to continue';
    if (step === 1) {
      if (testbeds.length === 0) return 'add a testbed to continue';
      if (testbeds.some(tb => tb.cloudAccountId === '')) return 'select a cloud account to continue';
      if (testbeds.some(tb => tb.proxies.length === 0)) return 'select at least one proxy per testbed';
    }
    if (step === 2) return 'select at least one language';
    return null;
  }, [canNext, step, testbeds]);

  const goNext = () => {
    if (!canNext || step >= 4) return;
    if (step === 1) {
      const missingProxies = testbeds.some(tb => tb.proxies.length === 0);
      if (missingProxies) { setProxyWarning(true); return; }
      setProxyWarning(false);
    }
    // Auto-switch single Linux testbed to Windows when .NET 4.8 is selected
    if (step === 2 && requiresWindows(selectedLangs)) {
      setTestbeds(prev => prev.map(tb => {
        if (tb.os === 'linux' && prev.length === 1) {
          const validProxies = (WINDOWS_PROXIES as readonly string[]);
          return { ...tb, os: 'windows' as const, proxies: tb.proxies.filter(p => validProxies.includes(p)) };
        }
        return tb;
      }));
    }
    setStep(step + 1);
  };

  // ── Submit ──────────────────────────────────────────────────────────

  // Fan out across (testbed × proxy × language). EVERY cell provisions its
  // own VM: the orchestrator creates one deployment per launched run
  // (ProvisioningOrchestrator.KickOneAsync) — there is no dedup by
  // (cloud_account_id, region, vm_size, os). An earlier comment here claimed
  // the orchestrator dedups and stacks languages/proxies onto shared VMs;
  // that code never existed and the claim leaked into the UI as a cost
  // undercount (#793 P2-1).
  const buildComparisonCells = (): ComparisonCell[] => {
    const cells: ComparisonCell[] = [];
    const langs = selectedLangs.size > 0 ? [...selectedLangs] : [''];
    for (const lang of langs) {
      for (const tb of testbeds) {
        // Windows-only runtimes (e.g. .NET Framework 4.8) cannot run on a
        // Linux testbed — the Languages step promises "Linux testbeds will
        // skip .NET 4.8 automatically", but until now the promise lived only
        // in the copy: the cell was created anyway and provisioning failed
        // (user-caught: csharp-net48 @ linux cell burned a doomed launch).
        if (!languageAllowedOnOs(lang, tb.os)) continue;
        const vmSize = resolveVmSize(tb.cloud, tb.vmSize);
        const topology = resolveTopology(tb.topology);
        for (const proxy of tb.proxies) {
          const label = [lang, `${tb.cloud}/${tb.region}`, tb.os, proxy]
            .filter(Boolean)
            .join(' @ ');
          cells.push({
            label,
            endpoint: {
              kind: 'pending',
              cloud_account_id: tb.cloudAccountId,
              region: tb.region,
              vm_size: vmSize,
              os: tb.os,
              proxy_stack: proxy,
              topology,
              ...(lang ? { language: lang } : {}),
            },
            ...(selectedTesterId ? { runner_id: selectedTesterId } : {}),
          });
        }
      }
    }
    return cells;
  };

  const totalCells = buildComparisonCells().length;
  const isMatrixRun = totalCells > 1;
  // Cells whose proxy stack has no QUIC will not run the h3 modes — the
  // comparison-group launch drops them per cell (shared/http-stacks.json);
  // say so on Review instead of surprising the user with N/A columns.
  const h3Drops = useMemo(() => h3DropsPerCell(testbeds, selectedModes), [testbeds, selectedModes]);

  // Name defaults to the placeholder when left blank — requiring a retype
  // left Launch silently disabled with no disabled styling.
  const namePlaceholder = `Application benchmark ${new Date().toISOString().slice(0, 10)}`;
  const effectiveName = () => configName.trim() || namePlaceholder;

  const buildWorkload = (): Workload => ({
    modes: [...selectedModes],
    runs,
    concurrency,
    timeout_ms: timeoutMs,
    payload_sizes: [],
    capture_mode: 'headers-only',
  });

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
    emptyCellsError: 'At least one testbed, proxy, and language are required',
  });

  // ── Render ──────────────────────────────────────────────────────────

  return (
    <WizardShell
      breadcrumbLabel="Application"
      breadcrumbTo={`/projects/${projectId}/runs`}
      title="New Application Benchmark"
      subtitle="Compare language and framework performance with statistical methodology."
      steps={STEPS}
      step={step}
      onStepClick={setStep}
      hideNextOn={[0]} // template cards advance the wizard themselves
      canNext={canNext}
      nextHint={nextHint}
      onNext={goNext}
      onBack={() => step > 0 && setStep(step - 1)}
    >
      {/* ── Step 0: Template ── */}
      {step === 0 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Choose a template</h3>
          <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
            {RUNTIME_TEMPLATES.map(tmpl => (
              <button
                key={tmpl.id}
                onClick={() => applyTemplate(tmpl)}
                className={`text-left border px-3 py-2.5 transition-colors ${
                  selectedTemplate === tmpl.id
                    ? 'border-cyan-500/50 bg-cyan-500/5'
                    : 'border-gray-800 hover:border-gray-600'
                }`}
              >
                <div className="text-sm font-medium text-gray-100">{tmpl.name}</div>
                <div className="text-xs text-gray-400 mt-0.5">{tmpl.description}</div>
                {tmpl.defaultTestbedCount > 0 && (
                  <div className="text-xs text-faint mt-1.5">
                    {tmpl.defaultTestbedCount} testbed / {tmpl.defaultLanguages.length} lang
                  </div>
                )}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* ── Step 1: Testbeds ── */}
      {step === 1 && (
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

      {/* ── Step 2: Languages ── */}
      {step === 2 && (
        <LanguageSelector
          selectedLangs={selectedLangs}
          onLangsChange={setSelectedLangs}
          testbeds={testbeds}
          selectedModes={selectedModes}
          capabilities={capabilities}
        />
      )}

      {/* ── Step 3: Methodology ── */}
      {step === 3 && (
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

      {/* ── Step 4: Review ── */}
      {step === 4 && (
        <ReviewStep
          configName={configName}
          onConfigNameChange={setConfigName}
          namePlaceholder={namePlaceholder}
          summaryLine={
            <>
              {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''}
              {' / '}{selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''}
              {' / '}{totalProxies} prox{totalProxies !== 1 ? 'ies' : 'y'}
              {' / '}{[...selectedModes].join(', ')}
            </>
          }
          extraSections={
            <div className="mb-4">
              <div className="text-xs uppercase tracking-wider text-faint mb-1.5">Template</div>
              <div className="text-xs text-gray-400">
                {RUNTIME_TEMPLATES.find(t => t.id === selectedTemplate)?.name ?? selectedTemplate}
              </div>
            </div>
          }
          testbeds={testbeds}
          methodology={methodology}
          workloadLine={
            <>{runs} runs x {concurrency} concurrency / {timeoutMs}ms timeout / {[...selectedModes].join(' ')}</>
          }
          matrixNote={isMatrixRun || h3Drops.length > 0
            ? (
              <>
                {isMatrixRun && <>Comparison group: {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''} x {selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''} = {totalCells} runs</>}
                {h3Drops.map(d => (
                  <div key={d.label} data-testid="h3-drop-note" className="text-amber-400/90">
                    {d.label}: {d.dropped.join(', ')} skipped — {d.stack} has no HTTP/3 (see shared/http-stacks.json)
                  </div>
                ))}
              </>
            )
            : undefined}
          afterWorkload={
            <>
              <div className="mb-4">
                <div className="text-xs uppercase tracking-wider text-faint mb-1.5">Languages</div>
                <div className="text-xs text-gray-400">
                  {[...selectedLangs].sort().map(lang => {
                    const entry = LANGUAGE_GROUPS.flatMap(g => g.entries).find(e => e.id === lang);
                    return entry?.label ?? lang;
                  }).join(', ')}
                </div>
              </div>
              {/* Provisioning cost + runner-readiness — one VM per comparison
                  cell (language × testbed × proxy); the widest fan-out wizard
                  had NO notice at all (#793 P2-2). */}
              <ProvisioningNotice
                vmCount={totalCells}
                cloud={new Set(testbeds.map(t => t.cloud)).size === 1 ? testbeds[0].cloud : 'multiple'}
                region={new Set(testbeds.map(t => t.region)).size === 1 ? testbeds[0].region : 'multiple'}
                onlineRunners={onlineRunners}
              />
            </>
          }
          addSchedule={addSchedule}
          onAddScheduleChange={setAddSchedule}
          cronExpr={cronExpr}
          onCronExprChange={setCronExpr}
          isMatrixRun={isMatrixRun}
          submitting={submitting}
          onSubmit={handleSubmit}
          launchLabel={isMatrixRun ? `Launch ${totalCells} Runs` : 'Launch Now'}
          cloudAccounts={cloudAccounts}
          launchBlockedReason={unhealthyAccountLaunchBlock(testbeds, cloudAccounts)}
        />
      )}
    </WizardShell>
  );
}
