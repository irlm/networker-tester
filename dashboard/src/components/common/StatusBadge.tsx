// One status ramp per the north star: green=success, red=failure,
// yellow=needs attention, cyan=in flight, grey=inert. Purple stays reserved
// for the logo; the previous blue/purple/orange one-offs (running=blue,
// provisioning=purple, cancelled=orange) were audit finding F12.
const statusColors: Record<string, string> = {
  online: 'bg-green-500/20 text-green-400 border-green-500/30',
  offline: 'bg-gray-500/20 text-gray-400 border-gray-500/30',
  busy: 'bg-yellow-500/20 text-yellow-400 border-yellow-500/30',
  pending: 'bg-gray-500/20 text-gray-400 border-gray-500/30',
  provisioning: 'bg-gray-500/20 text-gray-300 border-gray-500/30',
  queued: 'bg-gray-500/20 text-gray-300 border-gray-500/30',
  deploying: 'bg-cyan-500/20 text-cyan-400 border-cyan-500/30',
  waiting: 'bg-yellow-500/20 text-yellow-400 border-yellow-500/30',
  assigned: 'bg-cyan-500/20 text-cyan-400 border-cyan-500/30',
  running: 'bg-cyan-500/20 text-cyan-400 border-cyan-500/30',
  completed: 'bg-green-500/20 text-green-400 border-green-500/30',
  partial: 'bg-amber-500/20 text-amber-400 border-amber-500/30',
  failed: 'bg-red-500/20 text-red-400 border-red-500/30',
  cancelled: 'bg-gray-500/20 text-gray-400 border-gray-500/30',
};

interface StatusBadgeProps {
  status: string;
  label?: string;
}

export function StatusBadge({ status, label }: StatusBadgeProps) {
  const color = statusColors[status] || statusColors.offline;
  const isPulsing =
    status === 'deploying' ||
    status === 'running' ||
    status === 'assigned' ||
    status === 'provisioning';
  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 text-xs rounded border ${color}`}
    >
      <span className={`w-1.5 h-1.5 rounded-full bg-current mr-1.5 ${isPulsing ? 'motion-safe:animate-pulse' : ''}`} aria-hidden="true" />
      {label || status}
    </span>
  );
}
