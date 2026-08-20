import { useEffect, useRef, type ReactNode } from 'react';

/**
 * The one modal shell. Before this existed, ~15 dialogs each hand-rolled
 * the same backdrop + Escape listener (and most skipped the focus trap
 * entirely, so Tab walked out of the dialog into the page behind it).
 *
 * Owns: backdrop (click closes), Escape, focus trap (Tab cycles inside),
 * initial focus (first autofocusable/focusable element), aria wiring, and
 * the canonical surface look. Content stays the caller's.
 */
interface ModalProps {
  onClose: () => void;
  /** id of the heading element inside — wired to aria-labelledby. */
  labelledBy: string;
  /** Tailwind max-width class for the panel (default max-w-sm). */
  maxWidth?: string;
  /**
   * 'center' (default): the classic centered dialog. 'slide-over': a
   * full-height right-docked panel with the slide animation — the shape
   * the create/edit forms use.
   */
  variant?: 'center' | 'slide-over';
  role?: 'dialog' | 'alertdialog';
  describedBy?: string;
  closeDisabled?: boolean;
  rootClassName?: string;
  panelClassName?: string;
  testId?: string;
  children: ReactNode;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

// Multiple modal shells can coexist briefly (for example a destructive
// confirmation above a slide-over). Only the topmost shell should react to
// document-level keyboard input.
const openPanels: HTMLDivElement[] = [];

export function Modal({
  onClose,
  labelledBy,
  maxWidth = 'max-w-sm',
  variant = 'center',
  role = 'dialog',
  describedBy,
  closeDisabled = false,
  rootClassName = '',
  panelClassName = '',
  testId,
  children,
}: ModalProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  const closeDisabledRef = useRef(closeDisabled);

  useEffect(() => {
    onCloseRef.current = onClose;
    closeDisabledRef.current = closeDisabled;
  }, [closeDisabled, onClose]);

  useEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    openPanels.push(panel);

    // Capture the opener BEFORE moving focus so close restores it.
    const previouslyFocused = document.activeElement as HTMLElement | null;

    // Initial focus: prefer the first form field (dialog headers put a
    // close button before the inputs), else the first focusable element.
    const firstField = panel.querySelector<HTMLElement>(
      'input:not([disabled]), select:not([disabled]), textarea:not([disabled])',
    );
    (firstField ?? panel.querySelectorAll<HTMLElement>(FOCUSABLE)[0])?.focus();

    function onKeyDown(e: KeyboardEvent) {
      if (openPanels[openPanels.length - 1] !== panel) return;
      if (e.key === 'Escape') {
        e.stopPropagation();
        if (!closeDisabledRef.current) onCloseRef.current();
        return;
      }
      if (e.key !== 'Tab' || !panel) return;
      // Focus trap: Tab cycles within the panel.
      const items = panel.querySelectorAll<HTMLElement>(FOCUSABLE);
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || !panel.contains(active))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (active === last || !panel.contains(active))) {
        e.preventDefault();
        first.focus();
      }
    }

    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      const stackIndex = openPanels.lastIndexOf(panel);
      if (stackIndex >= 0) openPanels.splice(stackIndex, 1);
      previouslyFocused?.focus?.();
    };
  }, []);

  const isSlideOver = variant === 'slide-over';
  return (
    <div
      className={`fixed inset-0 z-50 flex min-w-0 overflow-hidden ${isSlideOver ? 'justify-end slide-over-backdrop' : 'items-center justify-center'} bg-black/60 ${rootClassName}`}
      onClick={() => { if (!closeDisabled) onClose(); }}
      data-testid={testId}
    >
      <div
        ref={panelRef}
        className={
          isSlideOver
            ? `slide-over-panel min-w-0 max-w-full w-full md:w-[520px] h-full bg-[var(--bg-base)] md:border-l border-gray-800 overflow-y-auto ${panelClassName}`
            : `min-w-0 max-w-full bg-[var(--bg-surface)] border border-gray-800 rounded-lg w-full ${maxWidth} p-6 max-h-[85dvh] overflow-y-auto ${panelClassName}`
        }
        onClick={e => e.stopPropagation()}
        role={role}
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
      >
        {children}
      </div>
    </div>
  );
}
