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

const CLOUD_SETUP_GUIDES: Record<string, { steps: string[]; fieldHelp: Record<string, string> }> = {
  azure: {
    steps: [
      '1. portal.azure.com \u2192 Microsoft Entra ID \u2192 App registrations \u2192 New registration',
      '2. Name: "AletheDash VM Manager", Supported account types: "Single tenant"',
      '3. Overview page: copy Application (client) ID and Directory (tenant) ID',
      '4. Certificates & secrets \u2192 New client secret \u2192 copy the Value immediately (shown once!)',
      '5. Subscriptions \u2192 your subscription \u2192 Access control (IAM) \u2192 Add role assignment',
      '   \u2192 Role: "Virtual Machine Contributor" \u2192 Members: select your app \u2192 Review + assign',
      '6. If testers need public IPs, also assign "Network Contributor" on the same subscription',
      '7. Paste the three values below: Tenant ID, Client ID, Client Secret',
    ],
    fieldHelp: {
      tenant_id: 'App registrations \u2192 your app \u2192 Overview \u2192 Directory (tenant) ID',
      client_id: 'Same Overview page \u2192 Application (client) ID',
      client_secret: 'Certificates & secrets \u2192 Client secrets \u2192 Value (shown once after creation)',
    },
  },
  aws: {
    steps: [
      'Option A \u2014 IAM User (permanent keys, no session token):',
      '  1. AWS Console \u2192 IAM \u2192 Users \u2192 Create user',
      '  2. Attach policy: AmazonEC2FullAccess',
      '  3. Security credentials \u2192 Create access key \u2192 "Application running outside AWS"',
      '  4. Copy Access Key ID + Secret Access Key. Leave Session Token empty.',
      '',
      'Option B \u2014 SSO / Temporary credentials (requires session token):',
      '  1. Run: aws sso login',
      '  2. Run: aws configure export-credentials --format env',
      '  3. Copy all three values: AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN',
      '  4. Note: temporary credentials expire. You will need to update them periodically.',
    ],
    fieldHelp: {
      access_key_id: 'Starts with "AKIA" (permanent) or "ASIA" (temporary/SSO).',
      secret_access_key: 'Paired with the access key. Shown only once for IAM users.',
    },
  },
  gcp: {
    steps: [
      '1. Go to console.cloud.google.com \u2192 IAM & Admin \u2192 Service Accounts',
      '2. Create Service Account \u2192 Name: "alethedash-vms"',
      '3. Grant roles to alethedash-vms: Compute Admin (roles/compute.admin) AND Service Account User (roles/iam.serviceAccountUser)',
      '4. Grant alethedash-vms the Service Account User role ON the Compute Engine default service account (PROJECT_NUMBER-compute@developer.gserviceaccount.com) \u2014 required so it can attach the default SA when creating VMs',
      '5. Click the alethedash-vms service account \u2192 Keys tab \u2192 Add key \u2192 Create new key \u2192 JSON',
      '6. Download the JSON file and paste its entire contents below',
      '',
      'Quick CLI for step 4: gcloud iam service-accounts add-iam-policy-binding PROJECT_NUMBER-compute@developer.gserviceaccount.com --member=serviceAccount:alethedash-vms@PROJECT_ID.iam.gserviceaccount.com --role=roles/iam.serviceAccountUser --project=PROJECT_ID',
    ],
    fieldHelp: {
      json_key: 'The full JSON key file. Contains project_id, client_email, private_key, etc.',
    },
  },
};

interface CredentialFields {
  azure: { tenant_id: string; subscription_id: string; resource_group: string; client_id: string; client_secret: string };
  aws: { access_key_id: string; secret_access_key: string; session_token: string };
  gcp: { json_key: string };
}

function emptyCredentials(): CredentialFields {
  return {
    azure: { tenant_id: '', subscription_id: '', resource_group: '', client_id: '', client_secret: '' },
    aws: { access_key_id: '', secret_access_key: '', session_token: '' },
    gcp: { json_key: '' },
  };
}

export function CloudAccountsPage() {
  const { projectId, isOperator, isProjectAdmin } = useProject();
  const [accounts, setAccounts] = useState<CloudAccountSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [saving, setSaving] = useState(false);
  const [validating, setValidating] = useState<string | null>(null);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [justCreatedId, setJustCreatedId] = useState<string | null>(null);

  // Form state
  const [editingId, setEditingId] = useState<string | null>(null);
  const [formName, setFormName] = useState('');
  const [formProvider, setFormProvider] = useState<string>('azure');
  const [formRegion, setFormRegion] = useState('');
  const [formPersonal, setFormPersonal] = useState(false);
  const [credentials, setCredentials] = useState<CredentialFields>(emptyCredentials());

  const isEditing = editingId !== null;

  const addToast = useToast();
  usePageTitle('Settings');

  const loadAccounts = useCallback(async () => {
    if (!projectId) return;
    try {
      const data = await api.getCloudAccounts(projectId);
      setAccounts(data);
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Unknown error';
      addToast('error', `Failed to load cloud accounts: ${msg}`);
    } finally {
      setLoading(false);
    }
  }, [projectId, addToast]);

  useEffect(() => { loadAccounts(); }, [loadAccounts]);

  const resetForm = () => {
    setShowForm(false);
    setEditingId(null);
    setFormName('');
    setFormProvider('azure');
    setFormRegion('');
    setFormPersonal(false);
    setCredentials(emptyCredentials());
    setValidationError(null);
    setJustCreatedId(null);
  };

  const openEditForm = (acct: CloudAccountSummary) => {
    setEditingId(acct.account_id);
    setFormName(acct.name);
    setFormProvider(acct.provider);
    setFormRegion(acct.region_default || '');
    setFormPersonal(acct.personal);
    setCredentials(emptyCredentials());
    setValidationError(null);
    setJustCreatedId(null);
    setShowForm(true);
  };

  const handleSave = async () => {
    if (!projectId || !formName.trim()) return;
    setSaving(true);
    setValidationError(null);
    setJustCreatedId(null);

    try {
      if (isEditing) {
        // Build credentials payload: only include if any field is filled
        const creds: Record<string, string> =
          formProvider === 'azure' ? { ...credentials.azure } :
          formProvider === 'aws' ? { ...credentials.aws } :
          { ...credentials.gcp };
        const filledCreds = Object.fromEntries(
          Object.entries(creds).filter(([, v]) => v.trim() !== '')
        );
        const hasNewCreds = Object.keys(filledCreds).length > 0;

        await api.updateCloudAccount(projectId, editingId, {
          name: formName.trim(),
          region_default: formRegion.trim() || undefined,
          credentials: hasNewCreds ? filledCreds : undefined,
        });

        // If credentials were updated, auto-validate
        if (hasNewCreds) {
          try {
            const result = await api.validateCloudAccount(projectId, editingId);
            if (result.status === 'active') {
              addToast('success', `Cloud account "${formName.trim()}" updated and validated`);
            } else {
              addToast('error', result.validation_error || 'Credentials updated but validation failed');
            }
          } catch (valErr) {
            const vmsg = valErr instanceof Error ? valErr.message : 'Unknown error';
            addToast('error', `Credentials updated but validation failed: ${vmsg}`);
          }
        } else {
          addToast('success', `Cloud account "${formName.trim()}" updated`);
        }
        resetForm();
        loadAccounts();
      } else {
        // Create mode: create then validate
        const creds: Record<string, string> =
          formProvider === 'azure' ? { ...credentials.azure } :
          formProvider === 'aws' ? { ...credentials.aws } :
          { ...credentials.gcp };

        const { account_id } = await api.createCloudAccount(projectId, {
          name: formName.trim(),
          provider: formProvider,
          credentials: creds,
          region_default: formRegion.trim() || undefined,
          personal: formPersonal,
        });

        // Immediately validate
        setJustCreatedId(account_id);
        try {
          const result = await api.validateCloudAccount(projectId, account_id);
          if (result.status === 'active') {
            addToast('success', `Cloud account "${formName.trim()}" created and validated`);
            resetForm();
          } else {
            setValidationError(result.validation_error || 'Validation failed -- check your credentials and provider setup.');
          }
        } catch (valErr) {
          const msg = valErr instanceof Error ? valErr.message : 'Unknown error';
          setValidationError(`Validation request failed: ${msg}`);
        }
        loadAccounts();
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Unknown error';
      addToast('error', `Failed to ${isEditing ? 'update' : 'create'} cloud account: ${msg}`);
    } finally {
      setSaving(false);
    }
  };

  const handleDeleteJustCreated = async () => {
    if (!projectId || !justCreatedId) return;
    try {
      await api.deleteCloudAccount(projectId, justCreatedId);
      addToast('success', 'Draft account deleted');
      resetForm();
      loadAccounts();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Unknown error';
      addToast('error', `Failed to delete: ${msg}`);
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
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Unknown error';
      addToast('error', `Validation request failed: ${msg}`);
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
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Unknown error';
      addToast('error', `Failed to delete cloud account: ${msg}`);
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
            onClick={() => { resetForm(); setShowForm(true); }}
            className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-2 rounded text-sm transition-colors"
          >
            Add Account
          </button>
        )}
      </div>
      <SettingsTabs />

      {showForm && (
        <div className="border border-gray-800 rounded p-4 mb-6">
          <h3 className="text-sm text-gray-200 font-medium mb-3">
            {isEditing ? 'Edit Cloud Account' : 'Add Cloud Account'}
          </h3>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-3">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Provider</label>
              <select
                value={formProvider}
                onChange={e => {
                  const p = e.target.value;
                  setFormProvider(p);
                  setFormName(`My ${PROVIDER_LABELS[p] || p} Account`);
                  setCredentials(emptyCredentials());
                }}
                disabled={isEditing}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 disabled:opacity-50"
              >
                {PROVIDERS.map(p => (
                  <option key={p} value={p}>{PROVIDER_LABELS[p]}</option>
                ))}
              </select>
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Name</label>
              <input
                value={formName}
                onChange={e => setFormName(e.target.value)}
                placeholder={`My ${PROVIDER_LABELS[formProvider] || formProvider} Account`}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                autoFocus
              />
            </div>
          </div>

          {/* Setup guide (only for new accounts) */}
          {!isEditing && CLOUD_SETUP_GUIDES[formProvider] && (
            <div className="mb-3 bg-gray-900/40 border border-gray-800 rounded p-3">
              <div className="text-[10px] text-cyan-400/80 font-medium uppercase tracking-wider mb-1.5">Setup Guide</div>
              <ol className="text-[11px] text-gray-400 space-y-0.5 list-none pl-0">
                {CLOUD_SETUP_GUIDES[formProvider].steps.map((step, i) => (
                  <li key={i} className="font-mono">{step}</li>
                ))}
              </ol>
            </div>
          )}

          {/* Provider-specific credential fields */}
          {isEditing && (
            <p className="text-xs text-gray-500 mb-2">Leave credential fields empty to keep existing values.</p>
          )}
          {formProvider === 'azure' && (
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-3">
              <div>
                <label className="block text-xs text-gray-400 mb-1">Subscription ID</label>
                <input
                  type="text"
                  value={credentials.azure.subscription_id}
                  onChange={e => setCredentials(prev => ({ ...prev, azure: { ...prev.azure, subscription_id: e.target.value } }))}
                  placeholder={isEditing ? 'leave empty to keep existing' : 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx'}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
                <p className="text-[10px] text-gray-600 mt-0.5">Subscriptions page → your subscription → Overview → Subscription ID</p>
              </div>
              <div>
                <label className="block text-xs text-gray-400 mb-1">Resource Group</label>
                <input
                  type="text"
                  value={credentials.azure.resource_group}
                  onChange={e => setCredentials(prev => ({ ...prev, azure: { ...prev.azure, resource_group: e.target.value } }))}
                  placeholder={isEditing ? 'leave empty to keep existing' : 'networker-testers'}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
                <p className="text-[10px] text-gray-600 mt-0.5">Resource group where tester VMs will be created. Must exist beforehand.</p>
              </div>
              <div>
                <label className="block text-xs text-gray-400 mb-1">Tenant ID</label>
                <input
                  type="text"
                  value={credentials.azure.tenant_id}
                  onChange={e => setCredentials(prev => ({ ...prev, azure: { ...prev.azure, tenant_id: e.target.value } }))}
                  placeholder={isEditing ? 'leave empty to keep existing' : undefined}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
                <p className="text-[10px] text-gray-600 mt-0.5">{CLOUD_SETUP_GUIDES.azure.fieldHelp.tenant_id}</p>
              </div>
              <div>
                <label className="block text-xs text-gray-400 mb-1">Client ID</label>
                <input
                  type="text"
                  value={credentials.azure.client_id}
                  onChange={e => setCredentials(prev => ({ ...prev, azure: { ...prev.azure, client_id: e.target.value } }))}
                  placeholder={isEditing ? 'leave empty to keep existing' : undefined}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
                <p className="text-[10px] text-gray-600 mt-0.5">{CLOUD_SETUP_GUIDES.azure.fieldHelp.client_id}</p>
              </div>
              <div>
                <label className="block text-xs text-gray-400 mb-1">Client Secret</label>
                <input
                  type="password"
                  value={credentials.azure.client_secret}
                  onChange={e => setCredentials(prev => ({ ...prev, azure: { ...prev.azure, client_secret: e.target.value } }))}
                  placeholder={isEditing ? 'leave empty to keep existing' : undefined}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
                <p className="text-[10px] text-gray-600 mt-0.5">{CLOUD_SETUP_GUIDES.azure.fieldHelp.client_secret}</p>
              </div>
            </div>
          )}
          {formProvider === 'aws' && (
            <div className="space-y-3 mb-3">
              <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Access Key ID</label>
                  <input
                    type="text"
                    value={credentials.aws.access_key_id}
                    onChange={e => setCredentials(prev => ({ ...prev, aws: { ...prev.aws, access_key_id: e.target.value } }))}
                    placeholder={isEditing ? 'leave empty to keep existing' : 'AKIA...'}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                  <p className="text-[10px] text-gray-600 mt-0.5">{CLOUD_SETUP_GUIDES.aws.fieldHelp.access_key_id}</p>
                </div>
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Secret Access Key</label>
                  <input
                    type="password"
                    value={credentials.aws.secret_access_key}
                    onChange={e => setCredentials(prev => ({ ...prev, aws: { ...prev.aws, secret_access_key: e.target.value } }))}
                    placeholder={isEditing ? 'leave empty to keep existing' : undefined}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                  <p className="text-[10px] text-gray-600 mt-0.5">{CLOUD_SETUP_GUIDES.aws.fieldHelp.secret_access_key}</p>
                </div>
              </div>
              <div>
                <label className="block text-xs text-gray-400 mb-1">
                  Session Token <span className="text-gray-600">(optional — only for temporary/SSO credentials)</span>
                </label>
                <input
                  type="password"
                  value={credentials.aws.session_token}
                  onChange={e => setCredentials(prev => ({ ...prev, aws: { ...prev.aws, session_token: e.target.value } }))}
                  placeholder={isEditing ? 'leave empty to keep existing' : undefined}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
                <p className="text-[10px] text-gray-600 mt-0.5">
                  Required if using <span className="text-gray-400">aws sso login</span> or <span className="text-gray-400">aws sts assume-role</span>.
                  Not needed for permanent IAM user keys. Get all three values with: <span className="text-gray-400 font-mono">aws configure export-credentials --format env</span>
                </p>
              </div>
            </div>
          )}
          {formProvider === 'gcp' && (
            <div className="mb-3">
              <label className="block text-xs text-gray-400 mb-1">Service Account JSON Key</label>
              <textarea
                value={credentials.gcp.json_key}
                onChange={e => setCredentials(prev => ({ ...prev, gcp: { ...prev.gcp, json_key: e.target.value } }))}
                rows={4}
                placeholder={isEditing ? 'leave empty to keep existing' : undefined}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 font-mono"
                style={{ WebkitTextSecurity: 'disc' } as React.CSSProperties}
              />
              <p className="text-[10px] text-gray-600 mt-0.5">{CLOUD_SETUP_GUIDES.gcp.fieldHelp.json_key}</p>
            </div>
          )}

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-3">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Default Region (optional)</label>
              <input
                value={formRegion}
                onChange={e => setFormRegion(e.target.value)}
                placeholder="us-east-1"
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              />
            </div>
            {!isEditing && (
              <div className="flex items-end pb-1">
                <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={formPersonal}
                    onChange={e => setFormPersonal(e.target.checked)}
                    className="accent-cyan-500"
                    disabled={!isProjectAdmin && !formPersonal}
                  />
                  Personal account
                  {!isProjectAdmin && (
                    <span className="text-xs text-gray-600">(shared requires admin)</span>
                  )}
                </label>
              </div>
            )}
          </div>

          {/* Validation error after create */}
          {validationError && (
            <div className="mb-3 border border-red-500/30 bg-red-500/10 rounded p-3">
              <p className="text-sm text-red-400 mb-2">Validation failed: {validationError}</p>
              <p className="text-xs text-gray-500 mb-2">
                The account was created but credentials could not be verified. You can keep it as a draft and fix your provider setup, or delete it.
              </p>
              <div className="flex gap-3">
                <button
                  onClick={() => { resetForm(); }}
                  className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors"
                >
                  Keep as draft
                </button>
                <button
                  onClick={handleDeleteJustCreated}
                  className="text-xs text-red-400 hover:text-red-300 transition-colors"
                >
                  Delete account
                </button>
              </div>
            </div>
          )}

          <div className="flex gap-3">
            <button
              onClick={handleSave}
              disabled={saving || !formName.trim() || !!validationError}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
            >
              {saving
                ? (isEditing ? 'Saving...' : 'Saving & Validating...')
                : (isEditing ? 'Save' : 'Save & Validate')}
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
                      {isOperator && (
                        <button
                          onClick={() => openEditForm(acct)}
                          className="text-xs text-gray-400 hover:text-cyan-300 transition-colors"
                        >
                          Edit
                        </button>
                      )}
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
