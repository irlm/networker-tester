/**
 * The one canonical pass/fail rendering for a run: `7/9 · 2 fail`.
 *
 * The audit (F4/F5) found four competing formats (`7 / 2`, `7 ok / 2 fail`,
 * `7/9 · 2 fail`, `7 ok 2 fail`) and zero-failure counts painted red. Rules:
 * - ok/total in green only when something succeeded, grey otherwise
 * - the fail segment wears red ONLY when failures > 0 — zero is always grey
 */
export function RunResult({ ok, fail, className = '' }: { ok: number; fail: number; className?: string }) {
  const total = ok + fail;
  return (
    <span className={`font-mono tabular-nums whitespace-nowrap ${className}`}>
      <span className={ok > 0 ? 'text-green-400' : 'text-gray-600'}>
        {ok}/{total}
      </span>
      <span className="text-gray-600"> · </span>
      <span className={fail > 0 ? 'text-red-400' : 'text-gray-600'}>{fail} fail</span>
    </span>
  );
}
