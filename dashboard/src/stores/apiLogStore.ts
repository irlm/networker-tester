import { create } from 'zustand';

export type RequestSource = 'user' | 'poll';

export interface ApiLogEntry {
  id: number;
  timestamp: number;
  method: string;
  path: string;
  status: number;
  /** Total round-trip time in ms (client-measured) */
  totalMs: number;
  /** Server processing time in ms (from X-Process-Time-Ms header) */
  serverMs: number | null;
  /** Network time = total - server */
  networkMs: number | null;
  error: string | null;
  /** Whether this was a user action or background polling */
  source: RequestSource;
}

export interface RenderLogEntry {
  id: number;
  timestamp: number;
  /** Component/page that rendered */
  component: string;
  /** What triggered it (e.g. "filter:status", "api:jobs", "mount") */
  trigger: string;
  /** Time from setState to paint in ms */
  renderMs: number;
  /** Number of items rendered */
  itemCount: number | null;
}

const MAX_ENTRIES = 200;
let nextApiId = 1;
let nextRenderId = 1;

interface ApiLogState {
  entries: ApiLogEntry[];
  renderEntries: RenderLogEntry[];
  enabled: boolean;
  add: (entry: Omit<ApiLogEntry, 'id'>) => void;
  addRender: (entry: Omit<RenderLogEntry, 'id'>) => void;
  clear: () => void;
  toggle: () => void;
}

export const useApiLogStore = create<ApiLogState>((set) => ({
  entries: [],
  renderEntries: [],
  enabled: true,
  add: (entry) =>
    set((state) => ({
      entries: [{ ...entry, id: nextApiId++ }, ...state.entries].slice(0, MAX_ENTRIES),
    })),
  addRender: (entry) =>
    set((state) => ({
      renderEntries: [{ ...entry, id: nextRenderId++ }, ...state.renderEntries].slice(0, MAX_ENTRIES),
    })),
  clear: () => set({ entries: [], renderEntries: [] }),
  toggle: () => set((s) => ({ enabled: !s.enabled })),
}));
