import { useEffect, useRef, useState } from 'react';
import { downloadExport } from '../../api/client';
import { useToast } from '../../hooks/useToast';

/** The document formats the report-export API serves (docs/reports-export.md). */
const FORMATS: ReadonlyArray<{ format: string; label: string; ext: string }> = [
  { format: 'pdf', label: 'PDF', ext: 'pdf' },
  { format: 'html', label: 'HTML', ext: 'html' },
  { format: 'docx', label: 'Word (.docx)', ext: 'docx' },
  { format: 'md', label: 'Markdown', ext: 'md' },
];

interface ExportMenuProps {
  /**
   * API path of the report WITHOUT the format query, e.g.
   * `/projects/p1/reports/integrated` — the menu appends `?format=…`.
   */
  path: string;
  /** Download-name stem used when the server sends no filename, e.g. `integrated-report`. */
  fileBase: string;
  /** Button label (default "Export"). */
  label?: string;
}

/**
 * "Export ▾" dropdown for the report-export endpoints: one entry per document
 * format, authenticated download via {@link downloadExport}, busy state while
 * a document renders server-side, error toast on failure. Closes on outside
 * click and Escape.
 */
export function ExportMenu({ path, fileBase, label = 'Export' }: ExportMenuProps) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const addToast = useToast();

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const run = (format: string, ext: string) => {
    setOpen(false);
    setBusy(format);
    const sep = path.includes('?') ? '&' : '?';
    downloadExport(`${path}${sep}format=${format}`, `${fileBase}.${ext}`)
      .catch((e) =>
        addToast('error', `Export failed: ${e instanceof Error ? e.message : String(e)}`))
      .finally(() => setBusy(null));
  };

  return (
    <div ref={rootRef} className="relative">
      <button
        onClick={() => setOpen(o => !o)}
        disabled={busy !== null}
        aria-haspopup="menu"
        aria-expanded={open}
        className="px-3 py-1.5 text-xs bg-gray-800 hover:bg-gray-700 text-gray-300 rounded transition-colors border border-gray-700 disabled:opacity-60 disabled:cursor-wait"
      >
        {busy ? `Exporting ${busy.toUpperCase()}…` : `${label} ▾`}
      </button>
      {open && (
        <div
          role="menu"
          className="absolute right-0 mt-1 w-40 z-20 bg-gray-900 border border-gray-700 rounded shadow-lg py-1"
        >
          {FORMATS.map(f => (
            <button
              key={f.format}
              role="menuitem"
              onClick={() => run(f.format, f.ext)}
              className="w-full text-left px-3 py-1.5 text-xs text-gray-300 hover:bg-gray-800 hover:text-gray-100 transition-colors"
            >
              {f.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
