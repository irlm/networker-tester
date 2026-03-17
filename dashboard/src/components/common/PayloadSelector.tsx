const PAYLOAD_PRESETS = [
  { label: 'Small (64KB)', value: '64k' },
  { label: 'Medium (1MB)', value: '1m' },
  { label: 'Large (16MB)', value: '16m' },
];

interface PayloadSelectorProps {
  selected: Set<string>;
  onToggle: (value: string) => void;
}

export function PayloadSelector({ selected, onToggle }: PayloadSelectorProps) {
  return (
    <div className="flex gap-2">
      {PAYLOAD_PRESETS.map((p) => (
        <label
          key={p.value}
          className={`flex items-center gap-2 px-3 py-1.5 rounded border cursor-pointer text-sm transition-colors ${
            selected.has(p.value)
              ? 'border-cyan-500/50 bg-cyan-500/10 text-cyan-400'
              : 'border-gray-700 text-gray-400 hover:border-gray-600'
          }`}
        >
          <input
            type="checkbox"
            checked={selected.has(p.value)}
            onChange={() => onToggle(p.value)}
            className="sr-only"
          />
          {p.label}
        </label>
      ))}
    </div>
  );
}
