import { DOC_CATEGORIES, type DocEntry } from '../../lib/docs/content';

interface DocEntryViewProps {
  entry: DocEntry;
  compact?: boolean;
  showCategory?: boolean;
}

export function DocEntryView({ entry, compact, showCategory = true }: DocEntryViewProps) {
  const cat = DOC_CATEGORIES.find((c) => c.id === entry.category);

  return (
    <div className={compact ? '' : 'py-3'}>
      {showCategory && cat && (
        <div className="flex items-center gap-2 mb-1">
          <span className="text-[10px] uppercase tracking-wider text-[#863bff] font-medium">
            {cat.label}
          </span>
        </div>
      )}
      <h3 className="text-cyan-400 text-sm font-medium mb-0.5">{entry.title}</h3>
      <p className="text-gray-500 text-xs">{entry.brief}</p>
      {!compact && (
        <>
          <div className="border-t border-[var(--border-default)] my-3" />
          <pre className="whitespace-pre-wrap text-xs text-gray-300 leading-relaxed max-w-prose">
            {formatDetail(entry.detail)}
          </pre>
        </>
      )}
    </div>
  );
}

/** Highlight field labels (e.g. "Primary metric:") with cyan color. */
function formatDetail(text: string): React.ReactNode {
  const lines = text.split('\n');
  const nodes: React.ReactNode[] = [];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    // Match lines like "Primary metric: value" or "Fields populated: value"
    const match = line.match(/^(\s*)([\w\s/()_-]+?:\s*)(.*)$/);
    if (match && match[2].length < 40) {
      nodes.push(
        <span key={i}>
          {match[1]}
          <span className="text-cyan-500/80">{match[2]}</span>
          {match[3]}
        </span>,
      );
    } else {
      nodes.push(line);
    }
    if (i < lines.length - 1) nodes.push('\n');
  }

  return <>{nodes}</>;
}
