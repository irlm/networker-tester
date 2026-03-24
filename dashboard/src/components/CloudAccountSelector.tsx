import { useState, useEffect, useCallback } from 'react';
import { api } from '../api/client';
import type { CloudAccountSummary } from '../api/types';
import { useToast } from '../hooks/useToast';

interface CloudAccountSelectorProps {
  projectId: string;
  provider: string;
  selectedAccountId: string | null;
  onSelect: (accountId: string) => void;
}

const PROVIDER_LABELS: Record<string, string> = {
  azure: 'Azure',
  aws: 'AWS',
  gcp: 'GCP',
};

export function CloudAccountSelector({ projectId, provider, selectedAccountId, onSelect }: CloudAccountSelectorProps) {
  const [accounts, setAccounts] = useState<CloudAccountSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [showAdd, setShowAdd] = useState(false);
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState('');
  const [newRegion, setNewRegion] = useState('');
  const [credentials, setCredentials] = useState<Record<string, string>>({});
  const addToast = useToast();

  const loadAccounts = useCallback(async () => {
    try {
      const all = await api.getCloudAccounts(projectId);
      const filtered = all.filter(a => a.provider === provider);
      setAccounts(filtered);
      // Auto-select first active account if none selected
      if (!selectedAccountId && filtered.length > 0) {
        const active = filtered.find(a => a.status === 'active');
        if (active) onSelect(active.account_id);
      }
    } catch {
      // silent
    } finally {
      setLoading(false);
    }
  }, [projectId, provider, selectedAccountId, onSelect]);

  useEffect(() => {
    setLoading(true);
    loadAccounts();
  }, [loadAccounts]);

  const handleAdd = async () => {
    if (!newName.trim()) return;
    setAdding(true);
    try {
      const result = await api.createCloudAccount(projectId, {
        name: newName.trim(),
        provider,
        credentials,
        region_default: newRegion.trim() || undefined,
        personal: true,
      });
      addToast('success', `Account "${newName.trim()}" created`);
      setShowAdd(false);
      setNewName('');
      setNewRegion('');
      setCredentials({});
      await loadAccounts();
      onSelect(result.account_id);
    } catch {
      addToast('error', 'Failed to create account');
    } finally {
      setAdding(false);
    }
  };

  if (loading) {
    return <p className="text-gray-500 text-sm motion-safe:animate-pulse">Loading accounts...</p>;
  }

  if (accounts.length === 0 && !showAdd) {
    return (
      <div className="border border-gray-800 rounded p-4 text-center">
        <p className="text-gray-500 text-sm mb-2">
          No {PROVIDER_LABELS[provider] || provider.toUpperCase()} accounts configured
        </p>
        <button
          type="button"
          onClick={() => setShowAdd(true)}
          className="text-xs text-cyan-400 hover:text-cyan-300"
        >
          + Add account
        </button>
      </div>
    );
  }

  return (
    <div>
      <p className="text-sm text-gray-400 mb-2">
        Select {PROVIDER_LABELS[provider] || provider.toUpperCase()} account:
      </p>
      <div className="space-y-2 mb-3">
        {accounts.map(acct => (
          <label
            key={acct.account_id}
            className={`flex items-center gap-3 p-2.5 rounded border cursor-pointer transition-colors ${
              selectedAccountId === acct.account_id
                ? 'border-cyan-500/50 bg-cyan-500/5'
                : 'border-gray-800 hover:border-gray-700'
            }`}
          >
            <input
              type="radio"
              name="cloud-account"
              checked={selectedAccountId === acct.account_id}
              onChange={() => onSelect(acct.account_id)}
              className="accent-cyan-500"
            />
            <div className="flex-1 min-w-0">
              <span className="text-sm text-gray-200">{acct.name}</span>
              {acct.region_default && (
                <span className="text-xs text-gray-500 ml-2">{acct.region_default}</span>
              )}
            </div>
            <span className={`text-xs px-1.5 py-0.5 rounded border ${
              acct.status === 'active'
                ? 'bg-green-500/10 text-green-400 border-green-500/30'
                : 'bg-red-500/10 text-red-400 border-red-500/30'
            }`}>
              {acct.status}
            </span>
          </label>
        ))}
      </div>

      {!showAdd ? (
        <button
          type="button"
          onClick={() => setShowAdd(true)}
          className="text-xs text-cyan-400 hover:text-cyan-300"
        >
          + Add new account
        </button>
      ) : (
        <div className="border border-gray-800 rounded p-3 mt-2">
          <p className="text-xs text-gray-400 font-medium mb-2">New {PROVIDER_LABELS[provider] || provider} Account</p>
          <div className="grid grid-cols-2 gap-3 mb-2">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Name</label>
              <input
                value={newName}
                onChange={e => setNewName(e.target.value)}
                placeholder="Account name"
                className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                autoFocus
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Default Region</label>
              <input
                value={newRegion}
                onChange={e => setNewRegion(e.target.value)}
                placeholder="us-east-1"
                className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              />
            </div>
          </div>

          {/* Provider-specific credential fields */}
          {provider === 'azure' && (
            <div className="grid grid-cols-1 gap-2 mb-2">
              {[['tenant_id', 'Tenant ID'], ['client_id', 'Client ID'], ['client_secret', 'Client Secret']].map(([key, label]) => (
                <div key={key}>
                  <label className="block text-xs text-gray-400 mb-1">{label}</label>
                  <input
                    type="password"
                    value={credentials[key] || ''}
                    onChange={e => setCredentials(prev => ({ ...prev, [key]: e.target.value }))}
                    className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </div>
              ))}
            </div>
          )}
          {provider === 'aws' && (
            <div className="grid grid-cols-1 gap-2 mb-2">
              {[['access_key_id', 'Access Key ID'], ['secret_access_key', 'Secret Access Key']].map(([key, label]) => (
                <div key={key}>
                  <label className="block text-xs text-gray-400 mb-1">{label}</label>
                  <input
                    type="password"
                    value={credentials[key] || ''}
                    onChange={e => setCredentials(prev => ({ ...prev, [key]: e.target.value }))}
                    className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </div>
              ))}
            </div>
          )}
          {provider === 'gcp' && (
            <div className="mb-2">
              <label className="block text-xs text-gray-400 mb-1">Service Account JSON Key</label>
              <textarea
                value={credentials['json_key'] || ''}
                onChange={e => setCredentials(prev => ({ ...prev, json_key: e.target.value }))}
                rows={3}
                className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 font-mono"
                style={{ WebkitTextSecurity: 'disc' } as React.CSSProperties}
              />
            </div>
          )}

          <div className="flex gap-2">
            <button
              type="button"
              onClick={handleAdd}
              disabled={adding || !newName.trim()}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-3 py-1 rounded text-xs transition-colors disabled:opacity-50"
            >
              {adding ? 'Creating...' : 'Create'}
            </button>
            <button
              type="button"
              onClick={() => { setShowAdd(false); setNewName(''); setCredentials({}); }}
              className="text-gray-400 hover:text-gray-200 px-2 py-1 text-xs"
            >
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
