import { useMemo, useState } from 'react';
import { useParams } from 'react-router-dom';
import { api, type TlsProfileDetail } from '../api/client';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useProject } from '../hooks/useProject';

export function TlsProfileDetailPage() {
  const { projectId } = useProject();
  const { runId } = useParams<{ runId: string }>();
  const [detail, setDetail] = useState<TlsProfileDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const shortId = runId?.slice(0, 8) ?? '';
  usePageTitle(runId ? `TLS ${shortId}` : 'TLS Profile');

  usePolling(
    () => {
      if (!runId) return;
      if (!projectId) return;
      api
        .getTlsProfile(projectId, runId)
        .then((data) => {
          setDetail(data);
          setError(null);
          setLoading(false);
        })
        .catch((e) => {
          setError(String(e));
          setLoading(false);
        });
    },
    15000,
    !!runId && !!projectId,
  );

  const findingsBySeverity = useMemo(() => {
    const findings = detail?.profile.findings ?? [];
    return {
      error: findings.filter((f) => f.severity === 'error'),
      warning: findings.filter((f) => f.severity === 'warning'),
      info: findings.filter((f) => f.severity === 'info'),
    };
  }, [detail]);

  if (loading && !detail) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'TLS Profiles', to: `/projects/${projectId}/tls-profiles` }, { label: `Run ${shortId}` }]} />
        <div className="text-gray-500 motion-safe:animate-pulse">Loading TLS profile...</div>
      </div>
    );
  }

  if (error && !detail) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'TLS Profiles', to: `/projects/${projectId}/tls-profiles` }, { label: `Run ${shortId}` }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load TLS profile</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
        </div>
      </div>
    );
  }

  if (!detail) return null;

  const leaf = detail.profile.certificate.leaf;
  const trust = detail.profile.trust;
  const connectivity = detail.profile.connectivity;
  const resumption = detail.profile.resumption;

  return (
    <div className="p-4 md:p-6 space-y-6">
      <Breadcrumb items={[{ label: 'TLS Profiles', to: `/projects/${projectId}/tls-profiles` }, { label: `${detail.host}:${detail.port}` }]} />

      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-xl font-bold text-gray-100 mb-1">{detail.host}:{detail.port}</h2>
          <p className="text-sm text-gray-500">
            {detail.target_kind} · {detail.coverage_level} · {new Date(detail.started_at).toLocaleString()}
          </p>
        </div>
        <div className="text-right">
          <div className={`text-sm font-semibold ${statusClass(detail.summary_status)}`}>{detail.summary_status}</div>
          {detail.summary_score != null && <div className="text-xs text-amber-400 mt-1">score {detail.summary_score}</div>}
        </div>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        <MetricCard label="TCP connect" value={formatMs(connectivity.tcp_connect_ms)} />
        <MetricCard label="TLS handshake" value={formatMs(connectivity.tls_handshake_ms)} />
        <MetricCard label="TLS version" value={connectivity.negotiated_tls_version ?? '-'} />
        <MetricCard label="ALPN" value={connectivity.alpn ?? '-'} />
      </div>

      <Section title="Trust posture">
        <KeyValueGrid
          items={[
            ['Hostname matches', boolLabel(trust.hostname_matches)],
            ['Chain valid', boolLabel(trust.chain_valid)],
            ['Trusted by system store', boolLabel(trust.trusted_by_system_store)],
            ['Verification performed', boolLabel(trust.verification_performed)],
            ['Chain presented', boolLabel(trust.chain_presented)],
            ['Verified chain depth', trust.verified_chain_depth ?? '-'],
            ['Revocation method', trust.revocation.method],
            ['Revocation status', trust.revocation.status],
            ['OCSP stapled', boolLabel(trust.revocation.ocsp_stapled)],
          ]}
        />
        <StringList title="Trust issues" items={trust.issues ?? []} empty="No trust issues recorded." />
      </Section>

      <Section title="Leaf certificate">
        {leaf ? (
          <>
            <KeyValueGrid
              items={[
                ['Subject', leaf.subject],
                ['Issuer', leaf.issuer],
                ['Key type', leaf.key_type],
                ['Key bits', leaf.key_bits ?? '-'],
                ['Signature algorithm', leaf.signature_algorithm],
                ['Not before', leaf.not_before ?? '-'],
                ['Not after', leaf.not_after ?? '-'],
                ['Must-staple', boolLabel(leaf.must_staple)],
                ['SCTs present', boolLabel(leaf.scts_present)],
                ['SHA-256 fingerprint', leaf.sha256_fingerprint],
                ['SPKI SHA-256', leaf.spki_sha256],
              ]}
            />
            <StringList title="SAN DNS" items={leaf.san_dns ?? []} empty="No DNS SANs recorded." />
            <StringList title="SAN IP" items={leaf.san_ip ?? []} empty="No IP SANs recorded." />
          </>
        ) : (
          <p className="text-sm text-gray-500">No leaf certificate details recorded.</p>
        )}
      </Section>

      <Section title="Path characteristics">
        <KeyValueGrid
          items={[
            ['Connected IP', detail.profile.path_characteristics.connected_ip ?? '-'],
            ['Requested IP', detail.profile.target.requested_ip ?? '-'],
            ['SNI', detail.profile.target.sni ?? '-'],
            ['Direct IP match', boolLabel(detail.profile.path_characteristics.direct_ip_match)],
            ['Proxy detected', boolLabel(detail.profile.path_characteristics.proxy_detected)],
            ['Classification', detail.profile.path_characteristics.classification],
            ['Cipher suite', connectivity.negotiated_cipher_suite ?? '-'],
            ['Key exchange group', connectivity.negotiated_key_exchange_group ?? '-'],
          ]}
        />
        <StringList title="Path evidence" items={detail.profile.path_characteristics.evidence ?? []} empty="No path evidence recorded." />
        <StringList title="Resolved IPs" items={detail.profile.target.resolved_ips ?? []} empty="No resolved IPs recorded." />
      </Section>

      <Section title="Session behavior">
        <KeyValueGrid
          items={[
            ['Resumption supported', boolLabel(resumption.supported)],
            ['Method', resumption.method ?? '-'],
            ['Initial handshake', formatMs(resumption.initial_handshake_ms)],
            ['Resumed handshake', formatMs(resumption.resumed_handshake_ms)],
            ['Resumption ratio', formatRatio(resumption.resumption_ratio)],
            ['Resumed TLS version', resumption.resumed_tls_version ?? '-'],
            ['Resumed cipher suite', resumption.resumed_cipher_suite ?? '-'],
            ['0-RTT offered', boolLabel(resumption.early_data_offered)],
            ['0-RTT accepted', typeof resumption.early_data_accepted === 'boolean' ? boolLabel(resumption.early_data_accepted) : '-'],
          ]}
        />
        <StringList title="Resumption notes" items={resumption.notes ?? []} empty="No resumption notes recorded." />
      </Section>

      <Section title="Findings">
        <FindingsList title="Errors" items={findingsBySeverity.error} empty="No error findings." color="text-red-300" />
        <FindingsList title="Warnings" items={findingsBySeverity.warning} empty="No warnings." color="text-yellow-300" />
        <FindingsList title="Info" items={findingsBySeverity.info} empty="No informational findings." color="text-cyan-300" />
      </Section>

      <Section title="Limitations and unsupported checks">
        <StringList title="Limitations" items={detail.profile.limitations ?? []} empty="No limitations recorded." />
        <StringList title="Unsupported checks" items={detail.profile.unsupported_checks ?? []} empty="No unsupported checks recorded." />
      </Section>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="table-container p-4 space-y-4">
      <h3 className="text-xs text-gray-500 tracking-wider font-medium uppercase">{title}</h3>
      {children}
    </section>
  );
}

function MetricCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-gray-800 bg-[var(--bg-surface)] p-3">
      <div className="text-[11px] uppercase tracking-wider text-gray-500">{label}</div>
      <div className="mt-1 text-sm font-medium text-gray-200">{value}</div>
    </div>
  );
}

function KeyValueGrid({ items }: { items: Array<[string, React.ReactNode]> }) {
  return (
    <div className="grid md:grid-cols-2 gap-x-6 gap-y-3 text-sm">
      {items.map(([label, value]) => (
        <div key={label} className="min-w-0">
          <div className="text-xs text-gray-500 mb-1">{label}</div>
          <div className="text-gray-200 break-all">{value}</div>
        </div>
      ))}
    </div>
  );
}

function StringList({ title, items, empty }: { title: string; items: string[]; empty: string }) {
  return (
    <div>
      <div className="text-xs text-gray-500 mb-2">{title}</div>
      {items.length === 0 ? (
        <p className="text-sm text-gray-600">{empty}</p>
      ) : (
        <ul className="space-y-1 text-sm text-gray-300 list-disc pl-5">
          {items.map((item, i) => <li key={`${title}-${i}`} className="break-all">{item}</li>)}
        </ul>
      )}
    </div>
  );
}

function FindingsList({ title, items, empty, color }: { title: string; items: Array<{ code: string; message: string }>; empty: string; color: string }) {
  return (
    <div>
      <div className="text-xs text-gray-500 mb-2">{title}</div>
      {items.length === 0 ? (
        <p className="text-sm text-gray-600">{empty}</p>
      ) : (
        <ul className="space-y-2">
          {items.map((item, i) => (
            <li key={`${item.code}-${i}`} className="rounded border border-gray-800 bg-[var(--bg-surface)] p-3">
              <div className={`text-xs font-semibold uppercase tracking-wider ${color}`}>{item.code}</div>
              <div className="text-sm text-gray-300 mt-1">{item.message}</div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function boolLabel(value: boolean) {
  return value ? 'Yes' : 'No';
}

function formatMs(value?: number | null) {
  return typeof value === 'number' ? `${value.toFixed(1)} ms` : '-';
}

function formatRatio(value?: number | null) {
  return typeof value === 'number' ? value.toFixed(2) : '-';
}

function statusClass(status: string) {
  const normalized = status.toLowerCase();
  if (normalized.includes('pass') || normalized.includes('ok') || normalized.includes('good')) return 'text-green-400';
  if (normalized.includes('warn') || normalized.includes('partial')) return 'text-yellow-400';
  if (normalized.includes('fail') || normalized.includes('error')) return 'text-red-400';
  return 'text-gray-300';
}
