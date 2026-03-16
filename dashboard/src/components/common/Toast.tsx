import { useToastStore, type ToastType } from '../../hooks/useToast';

const typeStyles: Record<ToastType, string> = {
  success: 'bg-green-500/15 border-green-500/40 text-green-400',
  error: 'bg-red-500/15 border-red-500/40 text-red-400',
  info: 'bg-cyan-500/15 border-cyan-500/40 text-cyan-400',
};

const typeIcons: Record<ToastType, string> = {
  success: '\u2713',
  error: '\u2717',
  info: '\u2139',
};

export function ToastContainer() {
  const toasts = useToastStore((s) => s.toasts);
  const removeToast = useToastStore((s) => s.removeToast);

  if (toasts.length === 0) return null;

  return (
    <div
      className="fixed bottom-4 right-4 z-[100] flex flex-col gap-2 max-w-sm"
      aria-live="polite"
      aria-label="Notifications"
    >
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={`flex items-center gap-2 px-4 py-3 rounded-lg border text-sm shadow-lg ${typeStyles[toast.type]}`}
          role="alert"
        >
          <span className="font-bold" aria-hidden="true">
            {typeIcons[toast.type]}
          </span>
          <span className="flex-1">{toast.message}</span>
          <button
            onClick={() => removeToast(toast.id)}
            className="ml-2 opacity-60 hover:opacity-100 text-current"
            aria-label="Dismiss notification"
          >
            {'\u2715'}
          </button>
        </div>
      ))}
    </div>
  );
}
