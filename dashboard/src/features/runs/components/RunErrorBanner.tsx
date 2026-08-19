import { stripAnsi } from '../../../lib/ansi';

/**
 * The run's `error_message`, rendered prominently under the run header
 * whenever present. Failed/cancelled runs used to hide it entirely — it only
 * rendered inside the live-progress block for queued/running runs, so the #791
 * "launch failed with no visible error" run showed a bare FAILED badge.
 * Red block for terminal failures; muted context line otherwise.
 */
export function RunErrorBanner({ status, message }: { status: string; message?: string | null }) {
  if (!message) return null;
  const failed = status === 'failed' || status === 'cancelled';
  return (
    <div
      data-testid="run-error-banner"
      className={`mb-6 border px-4 py-3 text-sm whitespace-pre-wrap break-words ${
        failed
          ? 'border-red-500/40 bg-red-950/20 text-red-300'
          : 'border-gray-800 text-gray-400'
      }`}
    >
      <span className={`block text-xs uppercase tracking-wider mb-1 ${failed ? 'text-red-400' : 'text-faint'}`}>
        {failed ? 'error' : 'last error'}
      </span>
      {stripAnsi(message)}
    </div>
  );
}
