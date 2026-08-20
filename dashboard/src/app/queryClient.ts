import { QueryClient } from '@tanstack/react-query';
import { ApiError } from '../api/http';

/**
 * Shared server-state policy for the dashboard.
 *
 * Domain features own their query keys and fetchers. This client owns only
 * cross-cutting behavior: a short freshness window, bounded retries, and
 * preservation of the current screen while background refreshes run.
 *
 * Returning to the tab is a refresh trigger (freshness audit): data older
 * than staleTime refetches on focus, so a backgrounded dashboard catches up
 * the moment the user comes back. staleTime bounds the burst — rapid tab
 * flips inside the 10s window issue no requests — and keepPreviousData on
 * list queries keeps the screen steady while the refetch runs.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 10_000,
      gcTime: 5 * 60_000,
      refetchOnWindowFocus: true,
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
