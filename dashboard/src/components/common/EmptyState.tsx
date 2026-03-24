interface EmptyStateProps {
  message: string;
  action?: React.ReactNode;
  compact?: boolean;
}

export function EmptyState({ message, action, compact }: EmptyStateProps) {
  return (
    <div className={`border border-gray-800 rounded ${compact ? 'p-6' : 'p-8'} text-center`}>
      <p className="text-gray-500 text-sm">{message}</p>
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}
