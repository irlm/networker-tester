import { useState, useCallback } from 'react';
import { cloudProviderBadge } from '../lib/provider';
import { api } from '../api/client';
import type { BenchmarkVmCatalogEntry } from '../api/types';
import { useProject } from '../hooks/useProject';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { PageHeader } from '../components/common/PageHeader';
import { EmptyState } from '../components/common/EmptyState';
import { DataTable } from '../components/common/DataTable';
import { timeAgo } from '../lib/format';


const statusBadge: Record<string, string> = {
  online: 'bg-green-500/20 text-green-400',
  offline: 'bg-red-500/20 text-red-400',
  unknown: 'bg-gray-500/20 text-gray-400',
};

export function BenchmarkCatalogPage() {
  usePageTitle('VM Catalog');
  const toast = useToast();
  const { projectId, isOperator } = useProject();

  const [vms, setVms] = useState<BenchmarkVmCatalogEntry[]>([]);
  const [showRegister, setShowRegister] = useState(false);
  const [detectingVmId, setDetectingVmId] = useState<string | null>(null);
  const [deletingVmId, setDeletingVmId] = useState<string | null>(null);
  const [registerLoading, setRegisterLoading] = useState(false);

  // Register form state
  const [formName, setFormName] = useState('');
  const [formIp, setFormIp] = useState('');
  const [formSshUser, setFormSshUser] = useState('azureuser');
  const [formCloud, setFormCloud] = useState('azure');
  const [formRegion, setFormRegion] = useState('');

  const refresh = useCallback(async () => {
    if (!projectId) return;
    try {
      const data = await api.listBenchmarkCatalog(projectId);
      setVms(data);
    } catch {
      // retry on next poll
    }
  }, [projectId]);

  usePolling(refresh, 15000);

  const handleRegister = async () => {
    if (!formName.trim() || !formIp.trim()) return;
    setRegisterLoading(true);
    try {
      await api.registerBenchmarkVm(projectId, {
        name: formName.trim(),
        ip: formIp.trim(),
        ssh_user: formSshUser.trim() || 'azureuser',
        cloud: formCloud,
        region: formRegion.trim(),
      });
      toast('success', `Registered VM "${formName.trim()}"`);
      setFormName('');
      setFormIp('');
      setFormSshUser('azureuser');
      setFormCloud('azure');
      setFormRegion('');
      setShowRegister(false);
      refresh();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to register VM';
      toast('error', msg);
    } finally {
      setRegisterLoading(false);
    }
  };

  const handleDetect = async (vmId: string) => {
    setDetectingVmId(vmId);
    try {
      const result = await api.detectBenchmarkVmLanguages(projectId, vmId);
      toast('success', `Detected ${result.languages.length} language(s)`);
      refresh();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Detection failed';
      toast('error', msg);
    } finally {
      setDetectingVmId(null);
    }
  };

  const handleDelete = async (vmId: string) => {
    setDeletingVmId(null);
    try {
      await api.deleteBenchmarkVm(projectId, vmId);
      toast('success', 'VM removed');
      refresh();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to delete VM';
      toast('error', msg);
    }
  };

  return (
    <div className="p-4 md:p-6 max-w-5xl">
      <PageHeader
        title="VM Catalog"
        action={isOperator ? (
          <button
            onClick={() => setShowRegister(!showRegister)}
            className="px-3 py-1.5 text-xs rounded border border-cyan-700 text-cyan-400 hover:bg-cyan-500/10 transition-colors"
          >
            Register VM
          </button>
        ) : undefined}
      />

      {/* Register form */}
      {showRegister && (
        <div className="mb-4 border border-gray-800 rounded bg-[var(--bg-card)] p-4">
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Name</label>
              <input
                type="text"
                value={formName}
                onChange={(e) => setFormName(e.target.value)}
                placeholder="benchmark-ubuntu-east"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                autoFocus
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">IP Address</label>
              <input
                type="text"
                value={formIp}
                onChange={(e) => setFormIp(e.target.value)}
                placeholder="10.0.0.5"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">SSH User</label>
              <input
                type="text"
                value={formSshUser}
                onChange={(e) => setFormSshUser(e.target.value)}
                placeholder="azureuser"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Cloud</label>
              <select
                value={formCloud}
                onChange={(e) => setFormCloud(e.target.value)}
                className="text-xs bg-gray-800 border border-gray-700 rounded px-2 py-1.5 text-gray-300 w-full"
              >
                <option value="azure">Azure</option>
                <option value="aws">AWS</option>
                <option value="gcp">GCP</option>
                <option value="manual">Manual</option>
              </select>
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Region</label>
              <input
                type="text"
                value={formRegion}
                onChange={(e) => setFormRegion(e.target.value)}
                placeholder="eastus"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
              />
            </div>
          </div>
          <div className="flex items-center gap-2 mt-4">
            <button
              onClick={handleRegister}
              disabled={registerLoading || !formName.trim() || !formIp.trim()}
              className="px-3 py-1.5 text-xs rounded bg-cyan-500/20 text-cyan-400 hover:bg-cyan-500/30 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
            >
              {registerLoading ? 'Registering...' : 'Register'}
            </button>
            <button
              onClick={() => { setShowRegister(false); setFormName(''); setFormIp(''); }}
              className="px-2 py-1.5 text-xs text-gray-400 hover:text-gray-300 transition-colors"
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {/* Empty state */}
      {vms.length === 0 && (
        <EmptyState
          message="No benchmark VMs registered"
          detail={<>Register a VM that has language servers deployed at <code className="text-gray-400">/opt/bench/</code>. Registered VMs appear in the benchmark wizard under "Use existing VM".</>}
        />
      )}

      {/* VM table */}
      {vms.length > 0 && (
        <DataTable
          columns={[
            { key: 'name', label: 'Name', cellClass: 'text-gray-200', render: (vm) => vm.name },
            {
              key: 'cloud',
              label: 'Cloud',
              render: (vm) => (
                <span className={`text-xs px-1.5 py-0.5 rounded ${cloudProviderBadge(vm.cloud)}`}>
                  {vm.cloud}
                </span>
              ),
            },
            { key: 'region', label: 'Region', cellClass: 'text-gray-400', render: (vm) => vm.region || '\u2014' },
            { key: 'ip', label: 'IP', cellClass: 'text-gray-300', render: (vm) => vm.ip },
            {
              key: 'languages',
              label: 'Languages',
              render: (vm) => (
                <div className="flex flex-wrap gap-1">
                  {vm.languages.length === 0 && (
                    <span className="text-faint text-xs">none</span>
                  )}
                  {vm.languages.map((lang) => (
                    <span
                      key={lang}
                      className="text-xs px-1.5 py-0.5 rounded border border-cyan-700/50 bg-cyan-500/10 text-cyan-400"
                    >
                      {lang}
                    </span>
                  ))}
                </div>
              ),
            },
            {
              key: 'status',
              label: 'Status',
              render: (vm) => (
                <span className={`text-xs px-1.5 py-0.5 rounded ${statusBadge[vm.status] || statusBadge.unknown}`}>
                  {vm.status}
                </span>
              ),
            },
            {
              key: 'last_health',
              label: 'Last Health',
              cellClass: 'text-gray-400',
              render: (vm) => (vm.last_health_check ? timeAgo(vm.last_health_check) : '\u2014'),
            },
            {
              key: 'actions',
              label: 'Actions',
              render: (vm) => (
                <div className="flex items-center gap-1.5">
                  {isOperator && (
                    <>
                      <button
                        onClick={() => handleDetect(vm.vm_id)}
                        disabled={detectingVmId === vm.vm_id}
                        className="px-2 py-1 text-xs rounded text-cyan-400 hover:bg-cyan-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                        title="Detect installed languages"
                      >
                        {detectingVmId === vm.vm_id ? (
                          <span className="inline-block motion-safe:animate-spin">&#8635;</span>
                        ) : (
                          'Detect'
                        )}
                      </button>
                      {deletingVmId === vm.vm_id ? (
                        <span className="flex items-center gap-1">
                          <button
                            onClick={() => handleDelete(vm.vm_id)}
                            className="px-2 py-1 text-xs rounded bg-red-500/20 text-red-400 hover:bg-red-500/30 transition-colors"
                          >
                            Confirm
                          </button>
                          <button
                            onClick={() => setDeletingVmId(null)}
                            className="px-1.5 py-1 text-xs text-gray-400 hover:text-gray-300 transition-colors"
                          >
                            Cancel
                          </button>
                        </span>
                      ) : (
                        <button
                          onClick={() => setDeletingVmId(vm.vm_id)}
                          className="px-2 py-1 text-xs rounded text-red-400 hover:bg-red-500/20 transition-colors"
                          title="Delete VM"
                        >
                          &#10005;
                        </button>
                      )}
                    </>
                  )}
                </div>
              ),
            },
          ]}
          rows={vms}
          rowKey={(vm) => vm.vm_id}
        />
      )}
    </div>
  );
}
