import type { ReactNode } from 'react';
import { Button } from './Button';

export function LoadingState({ label = 'Loading…', children }: { label?: string; children?: ReactNode }) {
  return (
    <div className="text-sm text-gray-400 motion-safe:animate-pulse" role="status">
      <span className="sr-only">{label}</span>
      {children ?? label}
    </div>
  );
}

interface ErrorStateProps {
  title: string;
  message: string;
  onRetry?: () => void;
  secondaryAction?: ReactNode;
}

export function ErrorState({ title, message, onRetry, secondaryAction }: ErrorStateProps) {
  return (
    <div className="alert alert-error" role="alert">
      <h3 className="font-bold text-red-400">{title}</h3>
      <p className="mt-1 text-sm text-red-300">{message}</p>
      {(onRetry || secondaryAction) && (
        <div className="mt-3 flex items-center gap-2">
          {onRetry && <Button variant="primary" size="xs" onClick={onRetry}>Retry</Button>}
          {secondaryAction}
        </div>
      )}
    </div>
  );
}
