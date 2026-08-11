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
  children: ReactNode;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Modal({ onClose, labelledBy, maxWidth = 'max-w-sm', variant = 'center', children }: ModalProps) {
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;

    // Capture the opener BEFORE moving focus so close restores it.
    const previouslyFocused = document.activeElement as HTMLElement | null;

    // Initial focus: prefer the first form field (dialog headers put a
    // close button before the inputs), else the first focusable element.
    const firstField = panel.querySelector<HTMLElement>(
      'input:not([disabled]), select:not([disabled]), textarea:not([disabled])',
    );
    (firstField ?? panel.querySelectorAll<HTMLElement>(FOCUSABLE)[0])?.focus();

    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
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
      previouslyFocused?.focus?.();
    };
  }, [onClose]);

  const isSlideOver = variant === 'slide-over';
  return (
    <div
      className={`fixed inset-0 z-50 flex ${isSlideOver ? 'justify-end slide-over-backdrop' : 'items-center justify-center'} bg-black/60`}
      onClick={onClose}
    >
      <div
        ref={panelRef}
        className={
          isSlideOver
            ? `slide-over-panel w-full md:w-[520px] h-full bg-[var(--bg-base)] md:border-l border-gray-800 overflow-y-auto`
            : `bg-[var(--bg-surface)] border border-gray-800 rounded-lg w-full ${maxWidth} p-6 max-h-[85vh] overflow-y-auto`
        }
        onClick={e => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
      >
        {children}
      </div>
    </div>
  );
}
