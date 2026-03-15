const statusColors: Record<string, string> = {
  online: 'bg-green-500/20 text-green-400 border-green-500/30',
  offline: 'bg-gray-500/20 text-gray-400 border-gray-500/30',
  busy: 'bg-yellow-500/20 text-yellow-400 border-yellow-500/30',
  pending: 'bg-gray-500/20 text-gray-400 border-gray-500/30',
  assigned: 'bg-blue-500/20 text-blue-400 border-blue-500/30',
  running: 'bg-cyan-500/20 text-cyan-400 border-cyan-500/30',
  completed: 'bg-green-500/20 text-green-400 border-green-500/30',
  failed: 'bg-red-500/20 text-red-400 border-red-500/30',
  cancelled: 'bg-orange-500/20 text-orange-400 border-orange-500/30',
};

interface StatusBadgeProps {
  status: string;
}

export function StatusBadge({ status }: StatusBadgeProps) {
  const color = statusColors[status] || statusColors.offline;
  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 text-xs rounded border ${color}`}
    >
      <span className="w-1.5 h-1.5 rounded-full bg-current mr-1.5" aria-hidden="true" />
      {status}
    </span>
  );
}
