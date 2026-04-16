import { useState, useEffect, useRef } from 'react';
import { useNavigate, Link } from 'react-router-dom';
import { api } from '../api/client';
import type { EndpointRef, TestConfigCreate, Workload } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';

type ProbePreset = 'quick' | 'standard' | 'full';

const PROBE_PRESETS: Record<ProbePreset, string[]> = {
  quick: ['dns', 'tcp', 'tls', 'http2'],
  standard: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp'],
  full: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp', 'curl', 'pageload', 'pageload2', 'pageload3', 'browser1', 'browser2', 'browser3'],
};

const PROBE_PRESET_LABELS: Record<ProbePreset, { time: string; desc: string }> = {
  quick: { time: '~3s', desc: 'dns, tcp, tls, http2' },
  standard: { time: '~15s', desc: '+ http1, http3, tls-resume, native-tls, udp' },
  full: { time: '~60s', desc: '+ pageload, browser' },
};

function extractHost(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return '';
  try {
    if (trimmed.includes('://')) {
      return new URL(trimmed).hostname;
    }
    const candidate = new URL(`https://${trimmed}`);
    return candidate.hostname;
  } catch {
    return trimmed;
  }
}

export function ProbePage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('Quick Probe');

  const inputRef = useRef<HTMLInputElement>(null);
  const [url, setUrl] = useState('');
  const [preset, setPreset] = useState<ProbePreset>('quick');
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleProbe = async () => {
    const host = extractHost(url);
    if (!host) {
      addToast('error', 'Enter a URL or hostname to probe');
      return;
    }

    setSubmitting(true);
    try {
      const presetLabel = preset.charAt(0).toUpperCase() + preset.slice(1);
      const configName = `Probe: ${host} (${presetLabel})`;

      const endpoint: EndpointRef = { kind: 'network', host };

      const workload: Workload = {
        modes: PROBE_PRESETS[preset],
        runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        payload_sizes: [],
        capture_mode: 'headers-only',
      };

      const config: TestConfigCreate = {
        name: configName,
        endpoint,
        workload,
      };

      const created = await api.createTestConfig(projectId, config);
      const run = await api.launchTestConfig(created.id);
      addToast('success', `Probe ${run.id.slice(0, 8)} launched`);
      navigate(`/projects/${projectId}/runs/${run.id}`);
    } catch (e) {
      addToast('error', `Probe failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="p-4 md:p-6 flex flex-col items-center">
      <div className="w-full max-w-2xl">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'Quick Probe' }]} />

        <div className="mt-10 mb-8 text-center">
          <h2 className="text-2xl font-bold text-gray-100 mb-2">Quick Probe</h2>
          <p className="text-sm text-gray-500">Test any URL in seconds. No target deployment needed.</p>
        </div>

        {/* URL input */}
        <div className="mb-6">
          <label htmlFor="probe-url" className="text-xs text-gray-500 mb-1.5 block">URL or hostname</label>
          <input
            ref={inputRef}
            id="probe-url"
            type="text"
            value={url}
            onChange={e => setUrl(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter' && url.trim()) handleProbe(); }}
            placeholder="e.g. www.cloudflare.com"
            className="bg-[var(--bg-base)] border border-gray-700 rounded px-4 py-3 text-sm font-mono text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />
        </div>

        {/* Preset cards */}
        <div className="mb-6">
          <label className="text-xs text-gray-500 mb-2 block">Probe depth</label>
          <div className="grid grid-cols-3 gap-3">
            {(['quick', 'standard', 'full'] as ProbePreset[]).map(p => (
              <button
                key={p}
                onClick={() => setPreset(p)}
                className={`border rounded p-4 text-left transition-colors ${
                  preset === p
                    ? 'border-cyan-500 bg-cyan-500/5'
                    : 'border-gray-800 hover:border-gray-600'
                }`}
              >
                <div className="text-sm font-medium text-gray-100 mb-1">
                  {p.charAt(0).toUpperCase() + p.slice(1)}
                </div>
                <div className="text-xs text-gray-500">{PROBE_PRESET_LABELS[p].desc}</div>
                <div className="text-xs text-cyan-400 mt-2 font-mono">{PROBE_PRESET_LABELS[p].time}</div>
              </button>
            ))}
          </div>
        </div>

        {/* Run button */}
        <button
          onClick={handleProbe}
          disabled={submitting || !url.trim()}
          className="w-full bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-5 py-3 rounded text-sm font-medium transition-colors"
        >
          {submitting ? 'Launching...' : 'Run Probe'}
        </button>

        {/* Link to configured run */}
        <div className="mt-8 pt-6 border-t border-gray-800 text-center">
          <p className="text-xs text-gray-600">
            Need a full configured run?{' '}
            <Link
              to={`/projects/${projectId}/runs/new`}
              className="text-cyan-400 hover:text-cyan-300 transition-colors"
            >
              Go to New Run
            </Link>
          </p>
        </div>
      </div>
    </div>
  );
}
