import { useCallback, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { useSearchParams } from 'react-router';
import { api, ApiError } from '../api/client';
import type { AlertChannel, AlertEvent, AlertRule } from '../api/types';
import { DataTable } from '../components/common/DataTable';
import { EmptyState } from '../components/common/EmptyState';
import { StatusBadge } from '../components/common/StatusBadge';
import { StatusFooter } from '../components/common/StatusFooter';
import { ChannelDialog } from '../components/alerts/ChannelDialog';
import { RuleDialog } from '../components/alerts/RuleDialog';
import { formatCondition, formatThreshold, SECRET_MASK } from '../components/alerts/alert-form';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { timeAgo } from '../lib/format';
import { useTestConfigsQuery } from '../features/runs/queries';

const TABS = ['rules', 'channels', 'history'] as const;
type Tab = (typeof TABS)[number];

const EVENTS_PAGE_SIZE = 50;

/** Extract the `{ "error": "..." }` envelope message from an ApiError body. */
function apiErrorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError && err.body) {
    try {
      const parsed = JSON.parse(err.body) as { error?: string };
      if (parsed.error) return parsed.error;
    } catch {
      // not JSON — fall through
    }
  }
  return err instanceof Error ? err.message : fallback;
}

function deliveryStatusColor(status: string | null): string {
  if (!status) return 'text-faint';
  if (status === 'delivered') return 'text-green-400';
  if (status.startsWith('failed')) return 'text-red-400';
  if (status.startsWith('skipped')) return 'text-gray-400';
  return 'text-yellow-400';
}

function Toggle({ on, busy, onClick, title }: { on: boolean; busy?: boolean; onClick: () => void; title: string }) {
  return (
    <button
      onClick={onClick}
      disabled={busy}
      title={title}
      aria-label={title}
      className={`w-9 h-5 rounded-full transition-colors relative inline-block ${busy ? 'opacity-50' : ''} ${on ? 'bg-cyan-600' : 'bg-gray-700'}`}
    >
      <span
        className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform ${on ? 'left-[18px]' : 'left-0.5'}`}
      />
    </button>
  );
}

export function AlertsPage() {
  const { projectId, isOperator } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const addToast = useToast();
  usePageTitle('Alerts');

  const tabParam = searchParams.get('tab');
  const tab: Tab = TABS.includes(tabParam as Tab) ? (tabParam as Tab) : 'rules';

  const [rules, setRules] = useState<AlertRule[]>([]);
  const [channels, setChannels] = useState<AlertChannel[]>([]);
  const [events, setEvents] = useState<AlertEvent[]>([]);
  const [eventsOffset, setEventsOffset] = useState(0);
  const [eventsRuleFilter, setEventsRuleFilter] = useState('');
  const [alertsLoading, setAlertsLoading] = useState(true);
  const configsQuery = useTestConfigsQuery(projectId, { polling: false });
  const configs = useMemo(() => configsQuery.data ?? [], [configsQuery.data]);
  const refetchConfigs = configsQuery.refetch;
  const loading = alertsLoading || configsQuery.isPending;

  const [ruleDialog, setRuleDialog] = useState<{ open: boolean; rule: AlertRule | null }>({ open: false, rule: null });
  const [channelDialog, setChannelDialog] = useState<{ open: boolean; channel: AlertChannel | null }>({ open: false, channel: null });
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [toggling, setToggling] = useState<string | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<Record<string, string>>({});
  const tabRefs = useRef<Record<Tab, HTMLButtonElement | null>>({ rules: null, channels: null, history: null });

  const setTab = useCallback(
    (next: Tab) => {
      setSearchParams(
        (prev) => {
          const p = new URLSearchParams(prev);
          if (next === 'rules') p.delete('tab');
          else p.set('tab', next);
          return p;
        },
        { replace: true },
      );
    },
    [setSearchParams],
  );

  const handleTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>, current: Tab) => {
    const currentIndex = TABS.indexOf(current);
    let next: Tab | undefined;
    if (event.key === 'ArrowRight') next = TABS[(currentIndex + 1) % TABS.length];
    if (event.key === 'ArrowLeft') next = TABS[(currentIndex - 1 + TABS.length) % TABS.length];
    if (event.key === 'Home') next = TABS[0];
    if (event.key === 'End') next = TABS[TABS.length - 1];
    if (!next) return;
    event.preventDefault();
    setTab(next);
    tabRefs.current[next]?.focus();
  };

  const loadEvents = useCallback(
    (offset: number, ruleId: string) => {
      if (!projectId) return;
      api
        .listAlertEvents(projectId, {
          limit: EVENTS_PAGE_SIZE,
          offset,
          ...(ruleId ? { rule_id: ruleId } : {}),
        })
        .then(setEvents)
        .catch(() => addToast('error', 'Failed to load alert history'));
    },
    [projectId, addToast],
  );

  // Declared above refresh because its setter is called inside the callback.
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null);

  const refresh = useCallback(() => {
    if (!projectId) return;
    void refetchConfigs();
    Promise.all([api.listAlertRules(projectId), api.listAlertChannels(projectId)])
      .then(([r, c]) => {
        setRules(r);
        setChannels(c);
        setAlertsLoading(false);
        setLastUpdatedAt(Date.now());
      })
      .catch(() => {
        addToast('error', 'Failed to load alerts');
        setAlertsLoading(false);
      });
    loadEvents(eventsOffset, eventsRuleFilter);
  }, [projectId, addToast, loadEvents, eventsOffset, eventsRuleFilter, refetchConfigs]);

  // StatusFooter wiring: pause flips usePolling's enabled flag; refresh
  // bumps resetKey (restarts the loop with an immediate tick).
  const [paused, setPaused] = useState(false);
  const [refreshTick, setRefreshTick] = useState(0);
  usePolling(refresh, 15000, !paused, refreshTick);

  const configNames = useMemo(() => new Map(configs.map((c) => [c.id, c.name])), [configs]);
  const channelNames = useMemo(() => new Map(channels.map((c) => [c.channel_id, c.name])), [channels]);

  const scopeLabel = useCallback(
    (testConfigId: string | null) =>
      testConfigId === null ? 'all configs' : configNames.get(testConfigId) ?? testConfigId.slice(0, 8),
    [configNames],
  );

  // ── Mutations ─────────────────────────────────────────────────────────

  const twoStepDelete = (key: string, run: () => Promise<void>) => {
    if (confirmDelete !== key) {
      setConfirmDelete(key);
      setTimeout(() => setConfirmDelete((prev) => (prev === key ? null : prev)), 3000);
      return;
    }
    setConfirmDelete(null);
    void run();
  };

  const handleToggleRule = async (rule: AlertRule) => {
    setToggling(rule.rule_id);
    try {
      await api.updateAlertRule(rule.rule_id, { enabled: !rule.enabled });
      addToast('success', `Rule ${!rule.enabled ? 'enabled' : 'disabled'}`);
      refresh();
    } catch (err) {
      addToast('error', apiErrorMessage(err, 'Failed to toggle rule'));
    } finally {
      setToggling(null);
    }
  };

  const handleDeleteRule = (rule: AlertRule) =>
    twoStepDelete(`rule:${rule.rule_id}`, async () => {
      try {
        await api.deleteAlertRule(rule.rule_id);
        addToast('success', 'Rule deleted');
        refresh();
      } catch (err) {
        addToast('error', apiErrorMessage(err, 'Failed to delete rule'));
      }
    });

  const handleToggleChannel = async (channel: AlertChannel) => {
    setToggling(channel.channel_id);
    try {
      await api.updateAlertChannel(channel.channel_id, { enabled: !channel.enabled });
      addToast('success', `Channel ${!channel.enabled ? 'enabled' : 'disabled'}`);
      refresh();
    } catch (err) {
      addToast('error', apiErrorMessage(err, 'Failed to toggle channel'));
    } finally {
      setToggling(null);
    }
  };

  const handleDeleteChannel = (channel: AlertChannel) =>
    twoStepDelete(`channel:${channel.channel_id}`, async () => {
      try {
        await api.deleteAlertChannel(channel.channel_id);
        addToast('success', 'Channel deleted');
        refresh();
      } catch (err) {
        if (err instanceof ApiError && err.status === 409) {
          addToast('error', apiErrorMessage(err, 'Channel is referenced by alert rules — delete or repoint them first'));
        } else {
          addToast('error', apiErrorMessage(err, 'Failed to delete channel'));
        }
      }
    });

  const handleTestChannel = async (channel: AlertChannel) => {
    setTesting(channel.channel_id);
    setTestResults((prev) => ({ ...prev, [channel.channel_id]: '' }));
    try {
      const { delivery_status } = await api.testAlertChannel(channel.channel_id);
      setTestResults((prev) => ({ ...prev, [channel.channel_id]: delivery_status }));
      if (delivery_status === 'delivered') addToast('success', `Test notification delivered via '${channel.name}'`);
      else addToast('error', `Test fire: ${delivery_status}`);
    } catch (err) {
      const msg = apiErrorMessage(err, 'Test fire failed');
      setTestResults((prev) => ({ ...prev, [channel.channel_id]: `failed: ${msg}` }));
      addToast('error', msg);
    } finally {
      setTesting(null);
    }
  };

  const changeEventsPage = (offset: number) => {
    setEventsOffset(offset);
    loadEvents(offset, eventsRuleFilter);
  };

  const changeEventsRuleFilter = (ruleId: string) => {
    setEventsRuleFilter(ruleId);
    setEventsOffset(0);
    loadEvents(0, ruleId);
  };

  // ── Render ────────────────────────────────────────────────────────────

  const enabledRules = rules.filter((r) => r.enabled).length;

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-4 md:mb-6">
        <div className="flex items-center gap-3 min-w-0">
          <h2 className="text-lg md:text-xl font-bold text-gray-100">Alerts</h2>
          {rules.length > 0 && (
            <span className="text-xs text-faint hidden sm:inline">
              <span className="text-green-400">{enabledRules}</span> active
              {enabledRules !== rules.length && <> · {rules.length} total</>}
            </span>
          )}
        </div>
        {isOperator && tab === 'rules' && (
          <button
            onClick={() => setRuleDialog({ open: true, rule: null })}
            className="bg-cyan-600 hover:bg-cyan-500 text-[var(--bg-base)] px-3 py-1.5 rounded text-sm transition-colors"
          >
            + Rule
          </button>
        )}
        {isOperator && tab === 'channels' && (
          <button
            onClick={() => setChannelDialog({ open: true, channel: null })}
            className="bg-cyan-600 hover:bg-cyan-500 text-[var(--bg-base)] px-3 py-1.5 rounded text-sm transition-colors"
          >
            + Channel
          </button>
        )}
      </div>

      {/* Tabs */}
      <div className="flex gap-1 border-b border-gray-800 mb-4" role="tablist" aria-label="Alert sections">
        {TABS.map((t) => (
          <button
            key={t}
            id={`alerts-${t}-tab`}
            role="tab"
            aria-selected={tab === t}
            aria-controls={`alerts-${t}-panel`}
            tabIndex={tab === t ? 0 : -1}
            ref={node => { tabRefs.current[t] = node; }}
            onClick={() => setTab(t)}
            onKeyDown={event => handleTabKeyDown(event, t)}
            className={`px-3 py-2 text-sm capitalize border-b-2 -mb-px transition-colors ${
              tab === t
                ? 'border-cyan-500 text-gray-100'
                : 'border-transparent text-gray-400 hover:text-gray-300'
            }`}
          >
            {t}
          </button>
        ))}
      </div>

      <div id={`alerts-${tab}-panel`} role="tabpanel" aria-labelledby={`alerts-${tab}-tab`}>
      {loading ? (
        <div className="py-10 text-center text-sm text-gray-400 motion-safe:animate-pulse">Loading alerts...</div>
      ) : tab === 'rules' ? (
        rules.length === 0 ? (
          <EmptyState
            message="No alert rules yet"
            detail="A rule watches one metric across consecutive runs and notifies a channel when the threshold breaks — e.g. p95_ms > 500 for 3 runs."
            action={
              isOperator ? (
                <button
                  onClick={() => setRuleDialog({ open: true, rule: null })}
                  className="text-cyan-400 hover:text-cyan-300 text-sm"
                >
                  + Create your first rule
                </button>
              ) : undefined
            }
          />
        ) : (
          <DataTable
            columns={[
              {
                key: 'condition',
                label: 'Condition',
                cellClass: 'text-cyan-400/90',
                render: (r) => formatCondition(r.metric, r.comparator, r.threshold),
              },
              {
                key: 'window',
                label: 'Window',
                cellClass: 'text-gray-400',
                render: (r) => <>{r.window_runs} run{r.window_runs === 1 ? '' : 's'}</>,
              },
              { key: 'scope', label: 'Scope', cellClass: 'text-gray-300', render: (r) => scopeLabel(r.test_config_id) },
              {
                key: 'channel',
                label: 'Channel',
                cellClass: 'text-gray-300',
                render: (r) => channelNames.get(r.channel_id) ?? r.channel_id.slice(0, 8),
              },
              { key: 'created', label: 'Created', hideBelow: 'lg', cellClass: 'text-gray-400', render: (r) => timeAgo(r.created_at) },
              {
                key: 'on',
                label: 'On',
                cellClass: 'text-center',
                render: (r) =>
                  isOperator ? (
                    <Toggle
                      on={r.enabled}
                      busy={toggling === r.rule_id}
                      onClick={() => handleToggleRule(r)}
                      title={r.enabled ? 'Disable rule' : 'Enable rule'}
                    />
                  ) : (
                    <StatusBadge status={r.enabled ? 'online' : 'offline'} label={r.enabled ? 'on' : 'off'} />
                  ),
              },
              {
                key: 'actions',
                label: '',
                render: (r) =>
                  isOperator && (
                    <div className="flex items-center gap-2">
                      <button
                        onClick={() => setRuleDialog({ open: true, rule: r })}
                        className="text-xs text-cyan-400 hover:text-cyan-300"
                        title="Edit rule"
                      >
                        edit
                      </button>
                      <button
                        onClick={() => handleDeleteRule(r)}
                        className={`text-xs transition-colors ${
                          confirmDelete === `rule:${r.rule_id}` ? 'text-red-400' : 'text-faint hover:text-red-400'
                        }`}
                        title={confirmDelete === `rule:${r.rule_id}` ? 'Click again to confirm' : 'Delete rule'}
                        aria-label={confirmDelete === `rule:${r.rule_id}` ? 'Confirm delete rule' : 'Delete rule'}
                      >
                        {confirmDelete === `rule:${r.rule_id}` ? 'delete?' : '✕'}
                      </button>
                    </div>
                  ),
              },
            ]}
            rows={rules}
            rowKey={(r) => r.rule_id}
            rowClass={(r) => (r.enabled ? undefined : 'opacity-60')}
          />
        )
      ) : tab === 'channels' ? (
        channels.length === 0 ? (
          <EmptyState
            message="No notification channels yet"
            detail="Channels are where alerts go: an HTTP webhook (optionally HMAC-signed) or an email recipient list. Rules point at exactly one channel."
            action={
              isOperator ? (
                <button
                  onClick={() => setChannelDialog({ open: true, channel: null })}
                  className="text-cyan-400 hover:text-cyan-300 text-sm"
                >
                  + Create your first channel
                </button>
              ) : undefined
            }
          />
        ) : (
          <DataTable
            columns={[
              { key: 'name', label: 'Name', cellClass: 'text-gray-200', render: (c) => c.name },
              { key: 'kind', label: 'Kind', cellClass: 'text-gray-400', render: (c) => c.kind },
              {
                key: 'destination',
                label: 'Destination',
                cellClass: 'text-gray-400',
                render: (c) =>
                  c.kind === 'webhook' ? (
                    <span className="inline-flex items-center gap-2">
                      <span className="truncate max-w-[28rem]" title={c.config.url}>
                        {c.config.url}
                      </span>
                      {c.config.secret === SECRET_MASK && (
                        <span
                          className="text-xs text-purple-300 border border-purple-500/30 bg-purple-500/10 rounded px-1.5 py-0.5"
                          title="Deliveries carry an HMAC-SHA256 signature header"
                        >
                          signed
                        </span>
                      )}
                    </span>
                  ) : (
                    <span className="truncate max-w-[28rem] inline-block" title={(c.config.to ?? []).join(', ')}>
                      {(c.config.to ?? []).join(', ')}
                    </span>
                  ),
              },
              {
                key: 'on',
                label: 'On',
                cellClass: 'text-center',
                render: (c) =>
                  isOperator ? (
                    <Toggle
                      on={c.enabled}
                      busy={toggling === c.channel_id}
                      onClick={() => handleToggleChannel(c)}
                      title={c.enabled ? 'Disable channel' : 'Enable channel'}
                    />
                  ) : (
                    <StatusBadge status={c.enabled ? 'online' : 'offline'} label={c.enabled ? 'on' : 'off'} />
                  ),
              },
              {
                key: 'actions',
                label: '',
                render: (c) =>
                  isOperator && (
                    <div className="flex items-center gap-2 flex-wrap">
                      <button
                        onClick={() => handleTestChannel(c)}
                        disabled={testing === c.channel_id}
                        className="text-xs text-cyan-400 hover:text-cyan-300 disabled:opacity-50"
                        title="Send a test notification through this channel"
                      >
                        {testing === c.channel_id ? 'testing...' : 'Test'}
                      </button>
                      {testResults[c.channel_id] && (
                        <span
                          className={`text-xs ${deliveryStatusColor(testResults[c.channel_id])}`}
                          title={testResults[c.channel_id]}
                        >
                          {testResults[c.channel_id]}
                        </span>
                      )}
                      <button
                        onClick={() => setChannelDialog({ open: true, channel: c })}
                        className="text-xs text-cyan-400 hover:text-cyan-300"
                        title="Edit channel"
                      >
                        edit
                      </button>
                      <button
                        onClick={() => handleDeleteChannel(c)}
                        className={`text-xs transition-colors ${
                          confirmDelete === `channel:${c.channel_id}` ? 'text-red-400' : 'text-faint hover:text-red-400'
                        }`}
                        title={confirmDelete === `channel:${c.channel_id}` ? 'Click again to confirm' : 'Delete channel'}
                        aria-label={confirmDelete === `channel:${c.channel_id}` ? 'Confirm delete channel' : 'Delete channel'}
                      >
                        {confirmDelete === `channel:${c.channel_id}` ? 'delete?' : '✕'}
                      </button>
                    </div>
                  ),
              },
            ]}
            rows={channels}
            rowKey={(c) => c.channel_id}
            rowClass={(c) => (c.enabled ? undefined : 'opacity-60')}
          />
        )
      ) : (
        // ── History ──────────────────────────────────────────────────────
        <>
          <div className="flex items-center gap-3 mb-3">
            <select
              value={eventsRuleFilter}
              onChange={(e) => changeEventsRuleFilter(e.target.value)}
              aria-label="Filter history by rule"
              className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 md:px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
            >
              <option value="">All rules</option>
              {rules.map((r) => (
                <option key={r.rule_id} value={r.rule_id}>
                  {formatCondition(r.metric, r.comparator, r.threshold)} — {scopeLabel(r.test_config_id)}
                </option>
              ))}
            </select>
            <span className="text-xs text-faint">newest first</span>
          </div>

          {events.length === 0 && eventsOffset === 0 ? (
            <EmptyState
              message="No alert events yet"
              detail="Events are recorded when a rule transitions between quiet and firing. Once a scheduled run breaches a threshold, it shows up here with its delivery outcome."
            />
          ) : (
            <DataTable
              columns={[
                {
                  key: 'time',
                  label: 'Time',
                  cellClass: 'text-gray-400 whitespace-nowrap',
                  titleOf: (e) => e.fired_at,
                  render: (e) => timeAgo(e.fired_at),
                },
                {
                  key: 'state',
                  label: 'State',
                  render: (e) => <StatusBadge status={e.state === 'firing' ? 'failed' : 'completed'} label={e.state} />,
                },
                {
                  key: 'rule',
                  label: 'Rule',
                  cellClass: 'text-cyan-400/90 whitespace-nowrap',
                  render: (e) => formatCondition(e.metric, e.comparator, e.threshold),
                },
                {
                  key: 'value',
                  label: 'Value',
                  cellClass: 'text-gray-200',
                  render: (e) => (e.value !== null ? formatThreshold(e.metric, e.value) : '--'),
                },
                {
                  key: 'scope',
                  label: 'Scope',
                  hideBelow: 'lg',
                  cellClass: 'text-gray-400',
                  render: (e) => scopeLabel(e.test_config_id),
                },
                {
                  key: 'message',
                  label: 'Message',
                  hideBelow: 'md',
                  cellClass: 'text-gray-400 max-w-[24rem] truncate',
                  titleOf: (e) => e.message ?? undefined,
                  render: (e) => e.message ?? '--',
                },
                {
                  key: 'delivery',
                  label: 'Delivery',
                  cellClass: 'max-w-[16rem] truncate',
                  titleOf: (e) => e.delivery_status ?? undefined,
                  render: (e) => (
                    <span className={deliveryStatusColor(e.delivery_status)}>{e.delivery_status ?? '--'}</span>
                  ),
                },
              ]}
              rows={events}
              rowKey={(e) => e.event_id}
            />
          )}

          {(eventsOffset > 0 || events.length === EVENTS_PAGE_SIZE) && (
            <div className="flex items-center justify-between mt-3 text-xs text-gray-400">
              <button
                onClick={() => changeEventsPage(Math.max(0, eventsOffset - EVENTS_PAGE_SIZE))}
                disabled={eventsOffset === 0}
                className="px-3 py-1.5 border border-gray-800 rounded hover:text-gray-300 hover:border-gray-700 disabled:opacity-40 disabled:hover:text-gray-400"
              >
                &#x2190; Newer
              </button>
              <span>
                {eventsOffset + 1}–{eventsOffset + events.length}
              </span>
              <button
                onClick={() => changeEventsPage(eventsOffset + EVENTS_PAGE_SIZE)}
                disabled={events.length < EVENTS_PAGE_SIZE}
                className="px-3 py-1.5 border border-gray-800 rounded hover:text-gray-300 hover:border-gray-700 disabled:opacity-40 disabled:hover:text-gray-400"
              >
                Older &#x2192;
              </button>
            </div>
          )}
        </>
      )}
      </div>

      {/* One footer for the whole page, mounted outside the tab switch —
          the single refresh() poll feeds all three tabs, and StatusFooter
          binds document-level r/p keys so only one instance may exist. */}
      <StatusFooter
        paused={paused}
        onPauseToggle={() => setPaused(p => !p)}
        onRefresh={() => { setRefreshTick(t => t + 1); if (paused) refresh(); }}
        lastUpdatedAt={lastUpdatedAt}
        intervalMs={15000}
        pills={
          <span className="text-faint">
            {rules.length} rule{rules.length !== 1 ? 's' : ''}
            {rules.length > 0 && ` (${enabledRules} active)`}
            {` · ${channels.length} channel${channels.length !== 1 ? 's' : ''}`}
          </span>
        }
      />

      {ruleDialog.open && (
        <RuleDialog
          projectId={projectId}
          channels={channels}
          configs={configs}
          existing={ruleDialog.rule}
          onClose={() => setRuleDialog({ open: false, rule: null })}
          onSaved={refresh}
        />
      )}
      {channelDialog.open && (
        <ChannelDialog
          projectId={projectId}
          existing={channelDialog.channel}
          onClose={() => setChannelDialog({ open: false, channel: null })}
          onSaved={refresh}
        />
      )}
    </div>
  );
}
