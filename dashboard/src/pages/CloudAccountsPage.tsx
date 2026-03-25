import { useState, useEffect, useCallback } from 'react';
import { api } from '../api/client';
import type { CloudAccountSummary } from '../api/types';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { SettingsTabs } from '../components/common/SettingsTabs';

const PROVIDERS = ['azure', 'aws', 'gcp'] as const;

const PROVIDER_LABELS: Record<string, string> = {
  azure: 'Azure',
  aws: 'AWS',
  gcp: 'GCP',
};

const PROVIDER_COLORS: Record<string, string> = {
  azure: 'text-blue-400',
  aws: 'text-orange-400',
  gcp: 'text-green-400',
};

const STATUS_STYLES: Record<string, string> = {
  active: 'bg-green-500/10 text-green-400 border-green-500/30',
  error: 'bg-red-500/10 text-red-400 border-red-500/30',
};

interface CredentialFields {
  azure: { tenant_id: string; client_id: string; client_secret: string };
  aws: { access_key_id: string; secret_access_key: string };
  gcp: { json_key: string };
}

function emptyCredentials(): CredentialFields {
  return {
    azure: { tenant_id: '', client_id: '', client_secret: '' },
    aws: { access_key_id: '', secret_access_key: '' },
    gcp: { json_key: '' },
  };
}

export function CloudAccountsPage() {
  const { projectId, isOperator, isProjectAdmin } = useProject();
  const [accounts, setAccounts] = useState<CloudAccountSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [showAdd, setShowAdd] = useState(false);
  const [adding, setAdding] = useState(false);
  const [validating, setValidating] = useState<string | null>(null);

  // Add form state
  const [newName, setNewName] = useState('');
  const [newProvider, setNewProvider] = useState<string>('azure');
  const [newRegion, setNewRegion] = useState('');
  const [newPersonal, setNewPersonal] = useState(false);
  const [credentials, setCredentials] = useState<CredentialFields>(emptyCredentials());

  const addToast = useToast();
  usePageTitle('Settings');

  const loadAccounts = useCallback(async () => {
    if (!projectId) return;
    try {
      const data = await api.getCloudAccounts(projectId);
      setAccounts(data);
    } catch {
      addToast('error', 'Failed to load cloud accounts');
    } finally {
      setLoading(false);
    }
  }, [projectId, addToast]);

  useEffect(() => { loadAccounts(); }, [loadAccounts]);

  const resetForm = () => {
    setShowAdd(false);
    setNewName('');
    setNewProvider('azure');
    setNewRegion('');
    setNewPersonal(false);
    setCredentials(emptyCredentials());
  };

  const handleAdd = async () => {
    if (!projectId || !newName.trim()) return;
    setAdding(true);
    try {
      const creds: Record<string, string> =
        newProvider === 'azure' ? { ...credentials.azure } :
        newProvider === 'aws' ? { ...credentials.aws } :
        { ...credentials.gcp };

      await api.createCloudAccount(projectId, {
        name: newName.trim(),
        provider: newProvider,
        credentials: creds,
        region_default: newRegion.trim() || undefined,
        personal: newPersonal,
      });
      addToast('success', `Cloud account "${newName.trim()}" created`);
      resetForm();
      loadAccounts();
    } catch {
      addToast('error', 'Failed to create cloud account');
    } finally {
      setAdding(false);
    }
  };

  const handleValidate = async (accountId: string) => {
    if (!projectId) return;
    setValidating(accountId);
    try {
      const result = await api.validateCloudAccount(projectId, accountId);
      if (result.status === 'active') {
        addToast('success', 'Account validated successfully');
      } else {
        addToast('error', result.validation_error || 'Validation failed');
      }
      loadAccounts();
    } catch {
      addToast('error', 'Validation request failed');
    } finally {
      setValidating(null);
    }
  };

  const handleDelete = async (accountId: string, name: string) => {
    if (!projectId) return;
    if (!window.confirm(`Delete cloud account "${name}"? This cannot be undone.`)) return;
    try {
      await api.deleteCloudAccount(projectId, accountId);
      addToast('success', `Deleted "${name}"`);
      loadAccounts();
    } catch {
      addToast('error', 'Failed to delete cloud account');
    }
  };

  if (loading) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Settings</h2>
        <SettingsTabs />
        <p className="text-gray-500 motion-safe:animate-pulse">Loading accounts...</p>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Settings</h2>
        {isOperator && (
          <button
            onClick={() => setShowAdd(true)}
            className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-2 rounded text-sm transition-colors"
          >
            Add Account
          </button>
        )}
      </div>
      <SettingsTabs />

      {showAdd && (
        <div className="border border-gray-800 rounded p-4 mb-6">
          <h3 className="text-sm text-gray-200 font-medium mb-3">Add Cloud Account</h3>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-3">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Provider</label>
              <select
                value={newProvider}
                onChange={e => setNewProvider(e.target.value)}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              >
                {PROVIDERS.map(p => (
                  <option key={p} value={p}>{PROVIDER_LABELS[p]}</option>
                ))}
              </select>
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Name</label>
              <input
                value={newName}
                onChange={e => setNewName(e.target.value)}
                placeholder="My Azure Account"
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                autoFocus
              />
            </div>
          </div>

          {/* Provider-specific credential fields */}
          {newProvider === 'azure' && (
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-3">
              <div>
                <label className="block text-xs text-gray-400 mb-1">Tenant ID</label>
                <input
                  type="password"
                  value={credentials.azure.tenant_id}
                  onChange={e => setCredentials(prev => ({ ...prev, azure: { ...prev.azure, tenant_id: e.target.value } }))}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
              </div>
              <div>
                <label className="block text-xs text-gray-400 mb-1">Client ID</label>
                <input
                  type="password"
                  value={credentials.azure.client_id}
                  onChange={e => setCredentials(prev => ({ ...prev, azure: { ...prev.azure, client_id: e.target.value } }))}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
              </div>
              <div>
                <label className="block text-xs text-gray-400 mb-1">Client Secret</label>
                <input
                  type="password"
                  value={credentials.azure.client_secret}
                  onChange={e => setCredentials(prev => ({ ...prev, azure: { ...prev.azure, client_secret: e.target.value } }))}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
              </div>
            </div>
          )}
          {newProvider === 'aws' && (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-3">
              <div>
                <label className="block text-xs text-gray-400 mb-1">Access Key ID</label>
                <input
                  type="password"
                  value={credentials.aws.access_key_id}
                  onChange={e => setCredentials(prev => ({ ...prev, aws: { ...prev.aws, access_key_id: e.target.value } }))}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
              </div>
              <div>
                <label className="block text-xs text-gray-400 mb-1">Secret Access Key</label>
                <input
                  type="password"
                  value={credentials.aws.secret_access_key}
                  onChange={e => setCredentials(prev => ({ ...prev, aws: { ...prev.aws, secret_access_key: e.target.value } }))}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
              </div>
            </div>
          )}
          {newProvider === 'gcp' && (
            <div className="mb-3">
              <label className="block text-xs text-gray-400 mb-1">Service Account JSON Key</label>
              <textarea
                value={credentials.gcp.json_key}
                onChange={e => setCredentials(prev => ({ ...prev, gcp: { ...prev.gcp, json_key: e.target.value } }))}
                rows={4}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 font-mono"
                style={{ WebkitTextSecurity: 'disc' } as React.CSSProperties}
              />
            </div>
          )}

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-3">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Default Region (optional)</label>
              <input
                value={newRegion}
                onChange={e => setNewRegion(e.target.value)}
                placeholder="us-east-1"
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              />
            </div>
            <div className="flex items-end pb-1">
              <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
                <input
                  type="checkbox"
                  checked={newPersonal}
                  onChange={e => setNewPersonal(e.target.checked)}
                  className="accent-cyan-500"
                  disabled={!isProjectAdmin && !newPersonal}
                />
                Personal account
                {!isProjectAdmin && (
                  <span className="text-xs text-gray-600">(shared requires admin)</span>
                )}
              </label>
            </div>
          </div>

          <div className="flex gap-3">
            <button
              onClick={handleAdd}
              disabled={adding || !newName.trim()}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
            >
              {adding ? 'Creating...' : 'Create Account'}
            </button>
            <button
              onClick={resetForm}
              className="text-gray-400 hover:text-gray-200 px-3 py-1.5 text-sm"
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {accounts.length === 0 ? (
        <div className="border border-gray-800 rounded p-8 text-center">
          <p className="text-gray-500 text-sm">No cloud accounts configured</p>
          <p className="text-gray-600 text-xs mt-1">Add a cloud account to enable deployments with stored credentials</p>
        </div>
      ) : (
        <div className="table-container">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                <th className="px-4 py-2.5 text-left font-medium">Name</th>
                <th className="px-4 py-2.5 text-left font-medium">Provider</th>
                <th className="px-4 py-2.5 text-left font-medium">Region</th>
                <th className="px-4 py-2.5 text-left font-medium">Type</th>
                <th className="px-4 py-2.5 text-left font-medium">Status</th>
                <th className="px-4 py-2.5 text-left font-medium">Last Validated</th>
                <th className="px-4 py-2.5 text-left font-medium"></th>
              </tr>
            </thead>
            <tbody>
              {accounts.map(acct => (
                <tr key={acct.account_id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                  <td className="px-4 py-3 text-gray-200">{acct.name}</td>
                  <td className="px-4 py-3">
                    <span className={`text-xs font-medium ${PROVIDER_COLORS[acct.provider] || 'text-gray-400'}`}>
                      {PROVIDER_LABELS[acct.provider] || acct.provider.toUpperCase()}
                    </span>
                  </td>
                  <td className="px-4 py-3 text-gray-400 text-xs">{acct.region_default || '\u2014'}</td>
                  <td className="px-4 py-3">
                    <span className={`text-xs ${acct.personal ? 'text-gray-500' : 'text-cyan-400'}`}>
                      {acct.personal ? 'personal' : 'shared'}
                    </span>
                  </td>
                  <td className="px-4 py-3">
                    <span className={`inline-block px-2 py-0.5 rounded text-xs border ${STATUS_STYLES[acct.status] || 'bg-gray-500/10 text-gray-400 border-gray-500/30'}`}>
                      {acct.status}
                    </span>
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs">
                    {acct.last_validated ? new Date(acct.last_validated).toLocaleString() : '\u2014'}
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex gap-3">
                      <button
                        onClick={() => handleValidate(acct.account_id)}
                        disabled={validating === acct.account_id}
                        className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors disabled:opacity-50"
                      >
                        {validating === acct.account_id ? 'Validating...' : 'Validate'}
                      </button>
                      <button
                        onClick={() => handleDelete(acct.account_id, acct.name)}
                        className="text-xs text-gray-600 hover:text-red-400 transition-colors"
                      >
                        Delete
                      </button>
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
