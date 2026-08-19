import { Button } from '../../../components/common/Button';

interface KeyboardRailProps {
  enabled: boolean;
  onToggle: () => void;
}

function Key({ children }: { children: string }) {
  return <kbd className="border border-gray-700 bg-[var(--bg-raised)] px-1.5 py-0.5 text-gray-300">{children}</kbd>;
}

export function KeyboardRail({ enabled, onToggle }: KeyboardRailProps) {
  return (
    <aside
      className="sticky bottom-0 z-20 -mx-4 mt-8 border-y border-[var(--border-default)] bg-[var(--bg-sidebar)]/95 px-4 py-2 md:-mx-6 md:px-6"
      aria-label="Scenario keyboard controls"
    >
      <div className="mx-auto flex max-w-[1440px] flex-wrap items-center gap-x-4 gap-y-2 text-xs text-gray-400">
        <Button
          size="xs"
          variant={enabled ? 'secondary' : 'ghost'}
          onClick={onToggle}
          aria-pressed={enabled}
          aria-label={`Scenario keyboard shortcuts ${enabled ? 'enabled' : 'disabled'}. Click to ${enabled ? 'disable' : 'enable'}.`}
        >
          {enabled ? '-- NORMAL --' : 'KEYS OFF'}
        </Button>
        {enabled ? (
          <>
            <span><Key>h/l</Key> intent</span>
            <span><Key>j/k</Key> test</span>
            <span><Key>1–4</Key> jump</span>
            <span><Key>Enter</Key> open</span>
            <span><Key>r</Key> recommended</span>
            <span><Key>m</Key> method</span>
            <span><Key>s</Key> status</span>
            <span className="ml-auto"><Key>/</Key> palette · <Key>?</Key> help</span>
          </>
        ) : (
          <span>Character shortcuts are disabled. Global <Key>/</Key> and <Key>?</Key> shortcuts still work.</span>
        )}
      </div>
    </aside>
  );
}
