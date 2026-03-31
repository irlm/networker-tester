import { useState, useCallback } from 'react';
import { api } from '../api/client';
import type { BenchmarkVmCatalogEntry } from '../api/types';
import { useProject } from '../hooks/useProject';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';

const cloudBadge: Record<string, string> = {
  azure: 'bg-blue-500/20 text-blue-400',
  aws: 'bg-yellow-500/20 text-yellow-400',
  gcp: 'bg-red-500/20 text-red-400',
  manual: 'bg-gray-500/20 text-gray-400',
};

const statusBadge: Record<string, string> = {
  online: 'bg-green-500/20 text-green-400',
  offline: 'bg-red-500/20 text-red-400',
  unknown: 'bg-gray-500/20 text-gray-500',
};

function timeAgo(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

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
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-lg md:text-xl font-bold text-gray-100">VM Catalog</h1>
        {isOperator && (
          <button
            onClick={() => setShowRegister(!showRegister)}
            className="px-3 py-1.5 text-xs rounded border border-cyan-700 text-cyan-400 hover:bg-cyan-500/10 transition-colors"
          >
            Register VM
          </button>
        )}
      </div>

      {/* Register form */}
      {showRegister && (
        <div className="mb-4 border border-gray-800 rounded bg-[var(--bg-card)] p-4">
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            <div>
              <label className="block text-xs text-gray-500 mb-1">Name</label>
              <input
                type="text"
                value={formName}
                onChange={(e) => setFormName(e.target.value)}
                placeholder="benchmark-ubuntu-east"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 font-mono"
                autoFocus
              />
            </div>
            <div>
              <label className="block text-xs text-gray-500 mb-1">IP Address</label>
              <input
                type="text"
                value={formIp}
                onChange={(e) => setFormIp(e.target.value)}
                placeholder="10.0.0.5"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 font-mono"
              />
            </div>
            <div>
              <label className="block text-xs text-gray-500 mb-1">SSH User</label>
              <input
                type="text"
                value={formSshUser}
                onChange={(e) => setFormSshUser(e.target.value)}
                placeholder="azureuser"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 font-mono"
              />
            </div>
            <div>
              <label className="block text-xs text-gray-500 mb-1">Cloud</label>
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
              <label className="block text-xs text-gray-500 mb-1">Region</label>
              <input
                type="text"
                value={formRegion}
                onChange={(e) => setFormRegion(e.target.value)}
                placeholder="eastus"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 font-mono"
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
              className="px-2 py-1.5 text-xs text-gray-500 hover:text-gray-300 transition-colors"
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {/* Empty state */}
      {vms.length === 0 && (
        <p className="text-gray-500 text-sm py-12 text-center">
          No benchmark VMs registered. Register a VM to get started.
        </p>
      )}

      {/* VM table */}
      {vms.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-xs text-gray-600 border-b border-gray-800">
                <th className="py-2 pr-3 font-medium">Name</th>
                <th className="py-2 pr-3 font-medium">Cloud</th>
                <th className="py-2 pr-3 font-medium">Region</th>
                <th className="py-2 pr-3 font-medium">IP</th>
                <th className="py-2 pr-3 font-medium">Languages</th>
                <th className="py-2 pr-3 font-medium">Status</th>
                <th className="py-2 pr-3 font-medium">Last Health</th>
                <th className="py-2 font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              {vms.map((vm) => (
                <tr
                  key={vm.vm_id}
                  className="border-b border-gray-800/50 hover:bg-gray-800/20 transition-colors"
                >
                  <td className="py-2.5 pr-3 font-mono text-gray-200">{vm.name}</td>
                  <td className="py-2.5 pr-3">
                    <span className={`text-[10px] px-1.5 py-0.5 rounded font-mono ${cloudBadge[vm.cloud] || cloudBadge.manual}`}>
                      {vm.cloud}
                    </span>
                  </td>
                  <td className="py-2.5 pr-3 text-gray-400 font-mono">{vm.region || '\u2014'}</td>
                  <td className="py-2.5 pr-3 text-gray-300 font-mono">{vm.ip}</td>
                  <td className="py-2.5 pr-3">
                    <div className="flex flex-wrap gap-1">
                      {vm.languages.length === 0 && (
                        <span className="text-gray-600 text-xs">none</span>
                      )}
                      {vm.languages.map((lang) => (
                        <span
                          key={lang}
                          className="text-[10px] px-1.5 py-0.5 rounded border border-cyan-700/50 bg-cyan-500/10 text-cyan-400 font-mono"
                        >
                          {lang}
                        </span>
                      ))}
                    </div>
                  </td>
                  <td className="py-2.5 pr-3">
                    <span className={`text-[10px] px-1.5 py-0.5 rounded ${statusBadge[vm.status] || statusBadge.unknown}`}>
                      {vm.status}
                    </span>
                  </td>
                  <td className="py-2.5 pr-3 text-xs text-gray-500">
                    {vm.last_health_check ? timeAgo(vm.last_health_check) : '\u2014'}
                  </td>
                  <td className="py-2.5">
                    <div className="flex items-center gap-1.5">
                      {isOperator && (
                        <>
                          <button
                            onClick={() => handleDetect(vm.vm_id)}
                            disabled={detectingVmId === vm.vm_id}
                            className="px-2 py-1 text-[10px] rounded text-cyan-400 hover:bg-cyan-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
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
                                className="px-2 py-1 text-[10px] rounded bg-red-500/20 text-red-400 hover:bg-red-500/30 transition-colors"
                              >
                                Confirm
                              </button>
                              <button
                                onClick={() => setDeletingVmId(null)}
                                className="px-1.5 py-1 text-[10px] text-gray-500 hover:text-gray-300 transition-colors"
                              >
                                Cancel
                              </button>
                            </span>
                          ) : (
                            <button
                              onClick={() => setDeletingVmId(vm.vm_id)}
                              className="px-2 py-1 text-[10px] rounded text-red-400 hover:bg-red-500/20 transition-colors"
                              title="Delete VM"
                            >
                              &#10005;
                            </button>
                          )}
                        </>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
