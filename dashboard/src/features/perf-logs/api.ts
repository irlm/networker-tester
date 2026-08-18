import { request } from '../../api/http';
import type { PerfLogRow, PerfLogStats } from '../../api/types';

export interface PerfLogListParams {
  kind?: string;
  path?: string;
  userId?: string;
  /** Relative window evaluated when the request starts; omit for all retained rows. */
  windowMs?: number;
  limit?: number;
  offset?: number;
}

function addSince(search: URLSearchParams, windowMs?: number) {
  if (windowMs !== undefined) {
    search.set('since', new Date(Date.now() - windowMs).toISOString());
  }
}

export const perfLogsApi = {
  list: (params: PerfLogListParams = {}, signal?: AbortSignal) => {
    const search = new URLSearchParams();
    if (params.kind) search.set('kind', params.kind);
    if (params.path) search.set('path', params.path);
    if (params.userId) search.set('user_id', params.userId);
    addSince(search, params.windowMs);
    if (params.limit) search.set('limit', String(params.limit));
    if (params.offset) search.set('offset', String(params.offset));
    const query = search.toString();
    return request<PerfLogRow[]>(`/perf-log${query ? `?${query}` : ''}`, { signal });
  },

  stats: (windowMs?: number, signal?: AbortSignal) => {
    const search = new URLSearchParams();
    addSince(search, windowMs);
    const query = search.toString();
    return request<PerfLogStats>(`/perf-log/stats${query ? `?${query}` : ''}`, { signal });
  },
};
