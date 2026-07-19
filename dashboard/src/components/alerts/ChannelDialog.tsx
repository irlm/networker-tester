import { useEffect, useRef, useState } from 'react';
import { api } from '../../api/client';
import type { AlertChannel, AlertChannelKind } from '../../api/types';
import { useToast } from '../../hooks/useToast';
import { channelConfigFromForm, validateChannelForm } from './alert-form';

interface ChannelDialogProps {
  projectId: string;
  /** When set, the dialog edits this channel instead of creating one. */
  existing?: AlertChannel | null;
  onClose: () => void;
  onSaved: () => void;
}

const inputCls =
  'w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500';

export function ChannelDialog({ projectId, existing, onClose, onSaved }: ChannelDialogProps) {
  const [kind, setKind] = useState<AlertChannelKind>(existing?.kind ?? 'webhook');
  const [name, setName] = useState(existing?.name ?? '');
  const [url, setUrl] = useState(existing?.config.url ?? '');
  // Prefilled with the mask when a secret is stored — the API keeps the
  // stored secret when the mask round-trips untouched (write-only contract).
  const [secret, setSecret] = useState(existing?.config.secret ?? '');
  const [to, setTo] = useState((existing?.config.to ?? []).join(', '));
  const [enabled, setEnabled] = useState(existing?.enabled ?? true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);
  const addToast = useToast();

  useEffect(() => {
    firstInputRef.current?.focus();
  }, []);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [onClose]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const values = { kind, name, url, secret, to };
    const validation = validateChannelForm(values);
    if (validation) {
      setError(validation);
      return;
    }
    setSaving(true);
    setError(null);
    try {
      if (existing) {
        await api.updateAlertChannel(existing.channel_id, {
          name: name.trim(),
          config: channelConfigFromForm(values),
          enabled,
        });
        addToast('success', 'Channel updated');
      } else {
        await api.createAlertChannel(projectId, {
          kind,
          name: name.trim(),
          config: channelConfigFromForm(values),
          enabled,
        });
        addToast('success', 'Channel created');
      }
      onSaved();
      onClose();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to save channel';
      setError(msg);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="alert-channel-dialog-title"
        className="relative w-full md:w-[520px] md:max-w-[90vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <form onSubmit={handleSubmit} className="p-4 md:p-6">
          <div className="flex items-center justify-between mb-6">
            <h3 id="alert-channel-dialog-title" className="text-lg font-bold text-gray-100">
              {existing ? 'Edit Channel' : 'New Channel'}
            </h3>
            <button type="button" onClick={onClose} className="text-gray-500 hover:text-gray-300 text-sm" aria-label="Close">
              &#x2715;
            </button>
          </div>

          {error && (
            <div role="alert" className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4 text-red-400 text-sm">
              {error}
            </div>
          )}

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-4">
            <div>
              <label htmlFor="channel-name" className="block text-xs text-gray-400 mb-1">Name</label>
              <input
                id="channel-name"
                ref={firstInputRef}
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="ops hook"
                className={inputCls}
              />
            </div>
            <div>
              <label htmlFor="channel-kind" className="block text-xs text-gray-400 mb-1">Kind</label>
              <select
                id="channel-kind"
                value={kind}
                onChange={(e) => setKind(e.target.value as AlertChannelKind)}
                disabled={existing != null}
                title={existing != null ? 'Kind cannot be changed after creation' : undefined}
                className={`${inputCls} disabled:opacity-50`}
              >
                <option value="webhook">webhook</option>
                <option value="email">email</option>
              </select>
            </div>
          </div>

          {kind === 'webhook' ? (
            <>
              <div className="mb-4">
                <label htmlFor="channel-url" className="block text-xs text-gray-400 mb-1">Webhook URL</label>
                <input
                  id="channel-url"
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  placeholder="https://hooks.example.com/networker"
                  className={`${inputCls} font-mono`}
                />
              </div>
              <div className="mb-4">
                <label htmlFor="channel-secret" className="block text-xs text-gray-400 mb-1">
                  HMAC secret <span className="text-gray-600">(optional)</span>
                </label>
                <input
                  id="channel-secret"
                  type="password"
                  autoComplete="off"
                  value={secret}
                  onChange={(e) => setSecret(e.target.value)}
                  className={`${inputCls} font-mono`}
                />
                <p className="text-[11px] text-gray-600 mt-1">
                  {existing?.config.secret
                    ? 'Write-only — leave the mask untouched to keep the stored secret, clear it to remove signing.'
                    : 'When set, deliveries carry an X-Networker-Signature HMAC-SHA256 header.'}
                </p>
              </div>
            </>
          ) : (
            <div className="mb-4">
              <label htmlFor="channel-to" className="block text-xs text-gray-400 mb-1">Recipients</label>
              <textarea
                id="channel-to"
                value={to}
                onChange={(e) => setTo(e.target.value)}
                placeholder="sre@example.com, oncall@example.com"
                rows={3}
                className={`${inputCls} font-mono resize-y`}
              />
              <p className="text-[11px] text-gray-600 mt-1">Comma or newline separated. One send per address.</p>
            </div>
          )}

          <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer mb-6">
            <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} className="accent-cyan-500" />
            Enabled
          </label>

          <div className="flex justify-end gap-3 pt-4 border-t border-gray-800/50 mt-6">
            <button type="button" onClick={onClose} className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200">
              Cancel
            </button>
            <button
              type="submit"
              disabled={saving}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
            >
              {saving ? 'Saving...' : existing ? 'Save Channel' : 'Create Channel'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
