interface EmptyStateProps {
  message: string;
  detail?: React.ReactNode;
  action?: React.ReactNode;
  compact?: boolean;
}

export function EmptyState({ message, detail, action, compact }: EmptyStateProps) {
  return (
    <div className={`border border-gray-800 rounded ${compact ? 'p-6' : 'p-8 py-12'} text-center`}>
      <p className="text-gray-400 text-sm">{message}</p>
      {detail && (
        <div className="text-gray-600 text-xs mt-2 max-w-lg mx-auto">{detail}</div>
      )}
      {action && <div className="mt-3">{action}</div>}
    </div>
  );
}
