import type { ModeGroup } from '../../api/types';

interface ModeSelectorProps {
  modeGroups: ModeGroup[];
  selectedModes: Set<string>;
  onToggle: (id: string) => void;
  onToggleGroup: (ids: string[], allSelected: boolean) => void;
}

export function ModeSelector({ modeGroups, selectedModes, onToggle, onToggleGroup }: ModeSelectorProps) {
  return (
    <div className="grid grid-cols-2 gap-3">
      {modeGroups.map((group) => {
        const ids = group.modes.map(m => m.id);
        const selectedCount = ids.filter(id => selectedModes.has(id)).length;
        const allSelected = selectedCount === ids.length;
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
                ref={el => { if (el) el.indeterminate = someSelected; }}
                onChange={() => onToggleGroup(ids, allSelected)}
                className="accent-cyan-500"
              />
              <span className="text-xs text-gray-500 font-medium">{group.label}</span>
              {someSelected && (
                <span className="text-[11px] text-gray-600">{selectedCount}/{ids.length}</span>
              )}
              {group.detail && (
                <span className="text-gray-600 hover:text-gray-400 cursor-help ml-1 text-xs" title={group.detail}>&#9432;</span>
              )}
            </label>
            <div className={`pl-5 ${group.label === 'Throughput' ? 'grid grid-cols-2 gap-x-4' : ''}`}>
              {group.modes.map((mode) => (
                <label
                  key={mode.id}
                  className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer py-0.5 hover:text-gray-100"
                >
                  <input
                    type="checkbox"
                    checked={selectedModes.has(mode.id)}
                    onChange={() => onToggle(mode.id)}
                    className="accent-cyan-500"
                  />
                  <span>{mode.name}</span>
                  <span className="text-xs text-gray-600 ml-auto" title={mode.detail}>{mode.desc}</span>
                </label>
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}
