import { QueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/http';

/**
 * Shared server-state policy for the dashboard.
 *
 * Domain features own their query keys and fetchers. This client owns only
 * cross-cutting behavior: a short freshness window, bounded retries, and
 * preservation of the current screen while background refreshes run.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 10_000,
      gcTime: 5 * 60_000,
      refetchOnWindowFocus: false,
      retry: (failureCount, error) => {
        if (error instanceof ApiError && error.status >= 400 && error.status < 500) {
          return false;
        }
        return failureCount < 2;
      },
    },
    mutations: {
      retry: false,
    },
  },
});
