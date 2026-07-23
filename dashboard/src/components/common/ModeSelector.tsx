import type { ModeGroup } from '../../api/types';

interface ModeSelectorProps {
  modeGroups: ModeGroup[];
  selectedModes: Set<string>;
  onToggle: (id: string) => void;
  onToggleGroup: (ids: string[], allSelected: boolean) => void;
  /**
   * Capability gate: return a human-readable reason a mode can't run against the
   * chosen target, or `null`/`undefined` when it can. Unsupported modes render
   * disabled + greyed with the reason as a tooltip, and are excluded from
   * per-mode and group toggles (so a run can't be launched with a mode that only
   * fails). Omit to allow everything.
   */
  unsupported?: (id: string) => string | null | undefined;
}

export function ModeSelector({
  modeGroups,
  selectedModes,
  onToggle,
  onToggleGroup,
  unsupported,
}: ModeSelectorProps) {
  const reasonFor = (id: string): string | null => (unsupported ? unsupported(id) ?? null : null);

  return (
    <div className="grid grid-cols-2 gap-3">
      {modeGroups.map((group) => {
        // Group toggle operates only over the SUPPORTED modes in the group.
        const selectableIds = group.modes.map(m => m.id).filter(id => reasonFor(id) === null);
        const selectedCount = selectableIds.filter(id => selectedModes.has(id)).length;
        const allSelected = selectableIds.length > 0 && selectedCount === selectableIds.length;
        const someSelected = selectedCount > 0 && !allSelected;
        return (
          <div
            key={group.label}
            className={`bg-[var(--bg-base)] border border-gray-800 rounded p-3 ${
              group.label === 'Throughput' ? 'col-span-2' : ''
            }`}
          >
            <label className="flex items-center gap-2 mb-2 cursor-pointer">
              <input
                type="checkbox"
                checked={allSelected}
                disabled={selectableIds.length === 0}
                ref={el => { if (el) el.indeterminate = someSelected; }}
                onChange={() => onToggleGroup(selectableIds, allSelected)}
                className="accent-cyan-500"
              />
              <span className="text-xs text-gray-500 font-medium">{group.label}</span>
              {someSelected && (
                <span className="text-[11px] text-gray-600">{selectedCount}/{selectableIds.length}</span>
              )}
              {group.detail && (
                <span className="text-gray-600 hover:text-gray-400 cursor-help ml-1 text-xs" title={group.detail}>&#9432;</span>
              )}
            </label>
            <div className={`pl-5 ${group.label === 'Throughput' ? 'grid grid-cols-2 gap-x-4' : ''}`}>
              {group.modes.map((mode) => {
                const reason = reasonFor(mode.id);
                const disabled = reason !== null;
                return (
                  <label
                    key={mode.id}
                    title={reason ?? mode.detail}
                    className={`flex items-center gap-2 text-sm py-0.5 ${
                      disabled
                        ? 'text-gray-600 cursor-not-allowed'
                        : 'text-gray-300 cursor-pointer hover:text-gray-100'
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={!disabled && selectedModes.has(mode.id)}
                      disabled={disabled}
                      onChange={() => { if (!disabled) onToggle(mode.id); }}
                      className="accent-cyan-500"
                    />
                    <span className={disabled ? 'line-through decoration-gray-700' : ''}>{mode.name}</span>
                    <span className="text-xs text-gray-600 ml-auto">
                      {disabled ? 'unsupported' : mode.desc}
                    </span>
                  </label>
                );
              })}
            </div>
          </div>
        );
      })}
    </div>
  );
}
