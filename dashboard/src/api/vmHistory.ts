/**
 * VM usage history REST client.
 *
 * Wraps the route mounted by `crates/networker-dashboard/src/api/vm_history.rs`
 * (v0.27.19). Snake_case field names match the backend shape — no transform
 * layer; components read fields directly.
 */

import { request } from './client';

export type ResourceType = 'tester' | 'endpoint' | 'benchmark';
export type EventType =
  | 'created'
  | 'started'
  | 'stopped'
  | 'deleted'
  | 'auto_shutdown'
  | 'error';

export type VmLifecycleRow = {
  event_id: string;
  project_id: string;
  resource_type: string;
  resource_id: string;
  resource_name: string | null;
  cloud: string;
  region: string | null;
  vm_size: string | null;
  vm_name: string | null;
  vm_resource_id: string | null;
  cloud_connection_id: string | null;
  cloud_account_name_at_event: string | null;
  provider_account_id: string | null;
  event_type: string;
  event_time: string;
  triggered_by: string | null;
  metadata: Record<string, unknown> | null;
  created_at: string;
};

export type VmHistoryResponse = {
  events: VmLifecycleRow[];
  /** True when the returned page is full — a load-more control should be shown. */
  has_more: boolean;
};

export type VmHistoryFilters = {
  resource_type?: ResourceType;
  resource_id?: string;
  from?: string;
  to?: string;
  limit?: number;
  offset?: number;
};

/**
 * Fetch a page of VM lifecycle events for a project.
 *
 * When `resource_id` is set, the backend returns events oldest-first for a
 * natural timeline view. Without it, events come back newest-first so the
 * project-wide feed shows "what happened recently" at the top.
 */
export async function listVmHistory(
  projectId: string,
  filters: VmHistoryFilters = {},
): Promise<VmHistoryResponse> {
  const params = new URLSearchParams();
  if (filters.resource_type) params.set('resource_type', filters.resource_type);
  if (filters.resource_id) params.set('resource_id', filters.resource_id);
  if (filters.from) params.set('from', filters.from);
  if (filters.to) params.set('to', filters.to);
  if (filters.limit != null) params.set('limit', String(filters.limit));
  if (filters.offset != null) params.set('offset', String(filters.offset));

  const qs = params.toString();
  const path = `/projects/${projectId}/vm-history${qs ? `?${qs}` : ''}`;
  return request<VmHistoryResponse>(path);
}
