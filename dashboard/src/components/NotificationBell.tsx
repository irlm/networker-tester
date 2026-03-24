import { useState, useEffect, useCallback } from 'react';
import { api } from '../api/client';
import { useApprovalSSE } from '../hooks/useSSE';

interface Props {
  projectId: string;
}

export function NotificationBell({ projectId }: Props) {
  const [count, setCount] = useState(0);

  const fetchCount = useCallback(async () => {
    try {
      const c = await api.getPendingApprovalCount(projectId);
      setCount(c);
    } catch {
      // ignore
    }
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    api.getPendingApprovalCount(projectId).then(c => {
      if (!cancelled) setCount(c);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [projectId]);

  // Refetch count on any SSE approval event
  useApprovalSSE(() => {
    fetchCount();
  });

  if (count === 0) return null;

  return (
    <span className="bg-red-500 text-[9px] text-white font-bold rounded-full px-1.5 py-0.5 leading-none">
      {count > 9 ? '9+' : count}
    </span>
  );
}
