import { useCallback, useMemo } from 'react';
import type { LanguageCapability } from '../../api/types';
import {
  LANGUAGE_GROUPS,
  ALL_LANGUAGE_IDS,
  TOP_5_IDS,
  SYSTEMS_IDS,
  requiresWindows,
} from './testbed-constants';
import type { TestbedState } from './testbed-constants';

// Modes that imply a direct h2/h3 negotiation with the language's own server.
const H2_MODE_IDS = new Set(['http2', 'pageload2', 'browser2', 'download2', 'upload2']);
const H3_MODE_IDS = new Set(['http3', 'pageload3', 'browser3', 'download3', 'upload3']);

export interface LanguageSelectorProps {
  selectedLangs: Set<string>;
  onLangsChange: (langs: Set<string>) => void;
  testbeds: TestbedState[];
  /** Workload modes chosen by the template — used to gate incompatible languages. */
  selectedModes?: Set<string>;
  /** Capability matrix from GET /api/modes; undefined = no gating (degrade open). */
  capabilities?: LanguageCapability[];
}

export function LanguageSelector({ selectedLangs, onLangsChange, testbeds, selectedModes, capabilities }: LanguageSelectorProps) {
  const capabilityById = useMemo(() => {
    const map = new Map<string, LanguageCapability>();
    for (const cap of capabilities ?? []) map.set(cap.language, cap);
    return map;
  }, [capabilities]);

  const wantsApibench = selectedModes?.has('apibench') ?? false;
  const wantsDirectH2 = [...(selectedModes ?? [])].some(m => H2_MODE_IDS.has(m));
  const wantsDirectH3 = [...(selectedModes ?? [])].some(m => H3_MODE_IDS.has(m));

  /** apibench selected but the language has no /api/* suite (nginx) — hard incompat. */
  const lacksApibench = useCallback((id: string): boolean => {
    if (!wantsApibench) return false;
    const cap = capabilityById.get(id);
    return cap !== undefined && !cap.apibench;
  }, [wantsApibench, capabilityById]);

  /** h2/h3 modes selected but the language self-serves HTTP/1.1 only (direct mode). */
  const directH1Only = useCallback((id: string): boolean => {
    const cap = capabilityById.get(id);
    if (!cap) return false;
    return (wantsDirectH2 && !cap.http2) || (wantsDirectH3 && !cap.http3);
  }, [capabilityById, wantsDirectH2, wantsDirectH3]);

  const hasH1OnlyAnnotations = ALL_LANGUAGE_IDS.some(id => directH1Only(id));

  const toggleLang = useCallback((id: string) => {
    const next = new Set(selectedLangs);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onLangsChange(next);
  }, [selectedLangs, onLangsChange]);

  const setShortcut = (ids: string[]) => {
    onLangsChange(new Set(ids.filter(id => !lacksApibench(id))));
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-sm font-semibold text-gray-200">Select Languages</h3>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setShortcut(ALL_LANGUAGE_IDS)}
            className="px-2 py-1 border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
          >
            Select All
          </button>
          <button
            type="button"
            onClick={() => setShortcut(TOP_5_IDS)}
            className="px-2 py-1 border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
          >
            Top 5
          </button>
          <button
            type="button"
            onClick={() => setShortcut(SYSTEMS_IDS)}
            className="px-2 py-1 border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
          >
            Systems Only
          </button>
        </div>
      </div>

      <div className="space-y-5">
        {LANGUAGE_GROUPS.map(group => (
          <div key={group.label}>
            <div className="text-[10px] uppercase tracking-wider font-mono text-gray-600 mb-2">{group.label}</div>
            <div className="flex flex-wrap gap-1.5">
              {group.entries.map(entry => {
                const checked = selectedLangs.has(entry.id);
                const isNginx = entry.id === 'nginx';
                const noApi = lacksApibench(entry.id);
                const h1Direct = directH1Only(entry.id);
                return (
                  <label
                    key={entry.id}
                    className={`flex items-center gap-2 px-3 py-2 border transition-colors text-xs ${
                      noApi
                        ? 'border-gray-800/60 text-gray-600 cursor-not-allowed'
                        : checked
                          ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-200 cursor-pointer'
                          : 'border-gray-800 text-gray-400 hover:border-gray-600 cursor-pointer'
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={checked && !noApi}
                      onChange={() => toggleLang(entry.id)}
                      disabled={isNginx || noApi}
                      className="accent-cyan-400"
                    />
                    <span>{entry.label}</span>
                    {noApi ? (
                      <span className="text-[10px] uppercase tracking-wider text-gray-600 border border-gray-700 px-1 py-0.5">
                        no /api/*
                      </span>
                    ) : isNginx ? (
                      <span className="text-[10px] uppercase tracking-wider text-cyan-500/70 border border-cyan-500/30 px-1 py-0.5">
                        baseline
                      </span>
                    ) : h1Direct ? (
                      <span className="text-[10px] uppercase tracking-wider text-gray-500 border border-gray-700 px-1 py-0.5">
                        h1 direct
                      </span>
                    ) : null}
                  </label>
                );
              })}
            </div>
          </div>
        ))}
      </div>

      <p className="text-xs text-gray-600 mt-4">
        {selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''} selected.
        {selectedLangs.has('nginx') && !wantsApibench && ' nginx is included as the static baseline.'}
      </p>

      {wantsApibench && (
        <p className="text-xs text-gray-600 mt-1">
          apibench selected: languages tagged <span className="font-mono">no /api/*</span> serve no measured API suite and are excluded.
        </p>
      )}

      {hasH1OnlyAnnotations && (
        <p className="text-xs text-gray-600 mt-1">
          Languages tagged <span className="font-mono">h1 direct</span> self-serve HTTP/1.1 only — h2/h3 modes measure the proxy in front of them, not the language runtime.
        </p>
      )}

      {requiresWindows(selectedLangs) && testbeds.length === 1 && testbeds[0].os === 'linux' && (
        <div className="mt-3 border border-yellow-500/30 bg-yellow-500/5 p-3">
          <p className="text-xs text-yellow-300">
            C# .NET 4.8 requires Windows Server. When you proceed, your testbed will be switched to Windows automatically.
          </p>
        </div>
      )}

      {requiresWindows(selectedLangs) && testbeds.length > 1 && testbeds.some(tb => tb.os === 'linux') && (
        <div className="mt-3 border border-yellow-500/30 bg-yellow-500/5 p-3">
          <p className="text-xs text-yellow-300">
            C# .NET 4.8 requires Windows Server. It will only run on testbeds configured with Windows OS.
            Linux testbeds will skip .NET 4.8 automatically.
          </p>
        </div>
      )}
    </div>
  );
}
