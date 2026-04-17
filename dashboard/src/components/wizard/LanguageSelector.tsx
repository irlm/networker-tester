import { useCallback } from 'react';
import {
  LANGUAGE_GROUPS,
  ALL_LANGUAGE_IDS,
  TOP_5_IDS,
  SYSTEMS_IDS,
  requiresWindows,
} from './testbed-constants';
import type { TestbedState } from './testbed-constants';

export interface LanguageSelectorProps {
  selectedLangs: Set<string>;
  onLangsChange: (langs: Set<string>) => void;
  testbeds: TestbedState[];
}

export function LanguageSelector({ selectedLangs, onLangsChange, testbeds }: LanguageSelectorProps) {
  const toggleLang = useCallback((id: string) => {
    const next = new Set(selectedLangs);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onLangsChange(next);
  }, [selectedLangs, onLangsChange]);

  const setShortcut = (ids: string[]) => {
    onLangsChange(new Set(ids));
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-sm font-semibold text-gray-200">Select Languages</h3>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setShortcut(ALL_LANGUAGE_IDS)}
            className="px-2 py-1 border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
          >
            Select All
          </button>
          <button
            onClick={() => setShortcut(TOP_5_IDS)}
            className="px-2 py-1 border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
          >
            Top 5
          </button>
          <button
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
                return (
                  <label
                    key={entry.id}
                    className={`flex items-center gap-2 px-3 py-2 border cursor-pointer transition-colors text-xs ${
                      checked
                        ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-200'
                        : 'border-gray-800 text-gray-400 hover:border-gray-600'
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggleLang(entry.id)}
                      disabled={isNginx}
                      className="accent-cyan-400"
                    />
                    <span>{entry.label}</span>
                    {isNginx && (
                      <span className="text-[10px] uppercase tracking-wider text-cyan-500/70 border border-cyan-500/30 px-1 py-0.5">
                        baseline
                      </span>
                    )}
                  </label>
                );
              })}
            </div>
          </div>
        ))}
      </div>

      <p className="text-xs text-gray-600 mt-4">
        {selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''} selected.
        {selectedLangs.has('nginx') && ' nginx is included as the static baseline.'}
      </p>

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
