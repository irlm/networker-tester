import { useDocsStore } from '../../stores/docsStore';

interface HelpHintProps {
  collapsed: boolean;
}

export function HelpHint({ collapsed }: HelpHintProps) {
  const openHelp = useDocsStore((s) => s.openHelp);
  const openPalette = useDocsStore((s) => s.openPalette);

  if (collapsed) {
    return (
      <div className="flex flex-col items-center gap-1 py-1.5">
        <button
          onClick={openHelp}
          className="text-faint hover:text-gray-400 text-xs transition-colors focus:outline-none focus:text-cyan-500"
          title="Help (?)"
        >
          ?
        </button>
        <button
          onClick={openPalette}
          className="text-faint hover:text-gray-400 text-xs transition-colors focus:outline-none focus:text-cyan-500"
          title="Search (/)"
        >
          /
        </button>
      </div>
    );
  }

  return (
    <div className="px-3 py-1.5 flex items-center gap-3 text-xs text-faint">
      <button
        onClick={openHelp}
        className="hover:text-gray-400 transition-colors focus:outline-none focus:text-cyan-500"
      >
        <kbd className="text-gray-400">?</kbd> Help
      </button>
      <button
        onClick={openPalette}
        className="hover:text-gray-400 transition-colors focus:outline-none focus:text-cyan-500"
      >
        <kbd className="text-gray-400">/</kbd> Search
      </button>
    </div>
  );
}
