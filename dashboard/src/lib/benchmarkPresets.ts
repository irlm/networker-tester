import { api } from '../api/client';
import type {
  BenchmarkComparePreset,
  BenchmarkComparePresetFilters,
  BenchmarkComparePresetInput,
} from '../api/types';

export type { BenchmarkComparePreset, BenchmarkComparePresetFilters, BenchmarkComparePresetInput };
export type BenchmarkRunFilterPreset = BenchmarkComparePresetFilters;

function storageKey(projectId: string): string {
  return `benchmark-compare-presets:${projectId}`;
}

function hasLocalStorage(): boolean {
  return typeof window !== 'undefined' && typeof window.localStorage !== 'undefined';
}

function readCachedPresets(projectId: string): BenchmarkComparePreset[] {
  if (!projectId || !hasLocalStorage()) return [];

  try {
    const raw = window.localStorage.getItem(storageKey(projectId));
    if (!raw) return [];
    const parsed = JSON.parse(raw) as BenchmarkComparePreset[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function writeCachedPresets(projectId: string, presets: BenchmarkComparePreset[]) {
  if (!projectId || !hasLocalStorage()) return;
  window.localStorage.setItem(storageKey(projectId), JSON.stringify(presets));
}

function sanitizeFilters(filters?: BenchmarkComparePresetFilters): BenchmarkComparePresetFilters | undefined {
  if (!filters) return undefined;
  const normalized = {
    targetSearch: filters.targetSearch?.trim() ?? '',
    scenario: filters.scenario?.trim() ?? '',
    phaseModel: filters.phaseModel?.trim() ?? '',
    serverRegion: filters.serverRegion?.trim() ?? '',
    networkType: filters.networkType?.trim() ?? '',
  };
  return Object.values(normalized).some(Boolean) ? normalized : undefined;
}

function sanitizePreset(input: Partial<BenchmarkComparePresetInput>): BenchmarkComparePreset | null {
  if (!input.name || !input.runIds || input.runIds.length < 2) return null;

  const uniqueRunIds = Array.from(new Set(input.runIds.filter(Boolean))).slice(0, 4);
  if (uniqueRunIds.length < 2) return null;

  const baselineRunId =
    input.baselineRunId && uniqueRunIds.includes(input.baselineRunId)
      ? input.baselineRunId
      : uniqueRunIds[0];

  return {
    id: input.id ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
    name: input.name.trim(),
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    runIds: uniqueRunIds,
    baselineRunId,
    filters: sanitizeFilters(input.filters),
  };
}

async function syncLocalPresetsToServer(
  projectId: string,
  cachedPresets: BenchmarkComparePreset[],
): Promise<BenchmarkComparePreset[]> {
  let next = cachedPresets;
  for (const preset of cachedPresets) {
    next = await api.saveBenchmarkComparePreset(projectId, {
      id: preset.id,
      name: preset.name,
      runIds: preset.runIds,
      baselineRunId: preset.baselineRunId,
      filters: preset.filters,
    });
  }
  writeCachedPresets(projectId, next);
  return next;
}

export async function loadBenchmarkComparePresets(projectId: string): Promise<BenchmarkComparePreset[]> {
  if (!projectId) return [];

  const cached = readCachedPresets(projectId);

  try {
    const remote = await api.getBenchmarkComparePresets(projectId);
    if (remote.length === 0 && cached.length > 0) {
      return syncLocalPresetsToServer(projectId, cached);
    }
    writeCachedPresets(projectId, remote);
    return remote;
  } catch {
    return cached;
  }
}

export async function saveBenchmarkComparePreset(
  projectId: string,
  presetInput: Partial<BenchmarkComparePresetInput>,
): Promise<BenchmarkComparePreset[]> {
  if (!projectId) return [];

  const preset = sanitizePreset(presetInput);
  if (!preset) return loadBenchmarkComparePresets(projectId);

  try {
    const remote = await api.saveBenchmarkComparePreset(projectId, {
      id: preset.id,
      name: preset.name,
      runIds: preset.runIds,
      baselineRunId: preset.baselineRunId,
      filters: preset.filters,
    });
    writeCachedPresets(projectId, remote);
    return remote;
  } catch {
    const current = readCachedPresets(projectId);
    const filtered = current.filter(
      (existing) =>
        existing.id !== preset.id &&
        existing.name.toLowerCase() !== preset.name.toLowerCase(),
    );
    const next = [preset, ...filtered].slice(0, 8);
    writeCachedPresets(projectId, next);
    return next;
  }
}

export async function deleteBenchmarkComparePreset(
  projectId: string,
  presetId: string,
): Promise<BenchmarkComparePreset[]> {
  if (!projectId) return [];

  try {
    const remote = await api.deleteBenchmarkComparePreset(projectId, presetId);
    writeCachedPresets(projectId, remote);
    return remote;
  } catch {
    const next = readCachedPresets(projectId).filter(
      (preset) => preset.id !== presetId,
    );
    writeCachedPresets(projectId, next);
    return next;
  }
}
