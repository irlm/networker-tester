import { useCallback, useEffect, useMemo, useState } from 'react';
import { PageHeader } from '../components/common/PageHeader';
import { CreateTesterModal } from '../components/CreateTesterModal';
import { TesterDetailDrawer } from '../components/TesterDetailDrawer';
import { TesterRegionGroup } from '../components/TesterRegionGroup';
import { testersApi, type TesterRow } from '../api/testers';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import { useTesterSubscription } from '../hooks/useTesterSubscription';
import { useToast } from '../hooks/useToast';

type GroupedTesters = { cloud: string; region: string; testers: TesterRow[] }[];

function groupByRegion(rows: TesterRow[]): GroupedTesters {
  const map = new Map<string, { cloud: string; region: string; testers: TesterRow[] }>();
  for (const row of rows) {
    const key = `${row.cloud}::${row.region}`;
    let entry = map.get(key);
    if (!entry) {
      entry = { cloud: row.cloud, region: row.region, testers: [] };
      map.set(key, entry);
    }
    entry.testers.push(row);
  }
  return Array.from(map.values()).sort((a, b) => {
    if (a.cloud !== b.cloud) return a.cloud.localeCompare(b.cloud);
    return a.region.localeCompare(b.region);
  });
}

interface CreateDefaults {
  cloud: string;
  region: string;
  name: string;
  vmSize: string;
  autoShutdownEnabled: boolean;
  autoShutdownHour: number;
}

const FIRST_TESTER_DEFAULTS: CreateDefaults = {
  cloud: 'azure',
  region: 'eastus',
  name: 'eastus-1',
  vmSize: 'Standard_D2s_v3',
  autoShutdownEnabled: true,
  autoShutdownHour: 23,
};

export function TestersPage() {
  usePageTitle('Testers');
  const { projectId, isProjectAdmin } = useProject();
  const addToast = useToast();

  const [rows, setRows] = useState<TesterRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<TesterRow | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [createDefaults, setCreateDefaults] = useState<CreateDefaults | null>(null);
  const [refreshingVersion, setRefreshingVersion] = useState(false);

  const refresh = useCallback(async () => {
    if (!projectId) return;
    setLoading(true);
    setError(null);
    try {
      const list = await testersApi.listTesters(projectId);
      setRows(list);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load testers');
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const grouped = useMemo(() => groupByRegion(rows), [rows]);
  const testerIds = useMemo(() => rows.map((r) => r.tester_id), [rows]);
  const queueMap = useTesterSubscription(projectId, testerIds);

  const handleAdd = useCallback((cloud: string, region: string) => {
    setCreateDefaults({
      cloud,
      region,
      name: '',
      vmSize: 'Standard_D2s_v3',
      autoShutdownEnabled: true,
      autoShutdownHour: 23,
    });
    setShowCreate(true);
  }, []);

  const handleEmptyStateCreate = useCallback(() => {
    setCreateDefaults(FIRST_TESTER_DEFAULTS);
    setShowCreate(true);
  }, []);

  const handleRefreshVersion = useCallback(async () => {
    if (!projectId) return;
    setRefreshingVersion(true);
    try {
      const res = await testersApi.refreshLatestVersion(projectId);
      addToast('success', `Latest version: ${res.latest_version}`);
    } catch (e) {
      addToast(
        'error',
        e instanceof Error ? e.message : 'Failed to refresh version',
      );
    } finally {
      setRefreshingVersion(false);
    }
  }, [projectId, addToast]);

  return (
    <div className="p-4 md:p-6 max-w-5xl mx-auto">
      <PageHeader
        title="Testers"
        subtitle="Persistent Azure VMs that run benchmarks against your targets."
        action={
          isProjectAdmin ? (
            <div className="flex gap-2">
              <button
                type="button"
                disabled={refreshingVersion}
                onClick={handleRefreshVersion}
                className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 disabled:opacity-50"
              >
                {refreshingVersion ? 'Refreshing…' : 'Refresh latest version'}
              </button>
              <button
                type="button"
                onClick={() => {
                  setCreateDefaults(null);
                  setShowCreate(true);
                }}
                className="px-3 py-1 text-xs rounded bg-cyan-600 hover:bg-cyan-500 text-white"
              >
                + New tester
              </button>
            </div>
          ) : null
        }
      />

      {error && (
        <div
          role="alert"
          className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4"
        >
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      )}

      {loading && rows.length === 0 ? (
        <p className="text-sm text-gray-500">Loading…</p>
      ) : rows.length === 0 ? (
        <div
          className="border border-gray-800 rounded p-6 text-center"
          data-testid="testers-empty-state"
        >
          <h3 className="text-sm font-bold text-gray-200 mb-2">
            No testers yet
          </h3>
          <p className="text-xs text-gray-500 mb-4 max-w-md mx-auto">
            Create a persistent tester VM in your preferred region. It will be
            reused across benchmarks and stopped each night to save costs.
          </p>
          <button
            type="button"
            onClick={handleEmptyStateCreate}
            className="px-4 py-1.5 text-xs rounded bg-cyan-600 hover:bg-cyan-500 text-white"
          >
            + Create your first tester in eastus (recommended)
          </button>
        </div>
      ) : (
        grouped.map((g) => (
          <TesterRegionGroup
            key={`${g.cloud}-${g.region}`}
            cloud={g.cloud}
            region={g.region}
            testers={g.testers}
            queues={queueMap}
            onSelect={setSelected}
            onAdd={handleAdd}
          />
        ))
      )}

      {showCreate && (
        <CreateTesterModal
          projectId={projectId}
          defaultCloud={createDefaults?.cloud}
          defaultRegion={createDefaults?.region}
          defaultName={createDefaults?.name}
          defaultVmSize={createDefaults?.vmSize}
          defaultAutoShutdownEnabled={createDefaults?.autoShutdownEnabled}
          defaultAutoShutdownHour={createDefaults?.autoShutdownHour}
          onCreated={() => {
            setShowCreate(false);
            setCreateDefaults(null);
            void refresh();
          }}
          onClose={() => {
            setShowCreate(false);
            setCreateDefaults(null);
          }}
        />
      )}

      {selected && (
        <TesterDetailDrawer
          projectId={projectId}
          tester={selected}
          onClose={() => setSelected(null)}
          onChanged={() => {
            void refresh();
          }}
        />
      )}
    </div>
  );
}
