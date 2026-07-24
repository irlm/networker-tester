import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { RunEnvelopeBlock } from './RunEnvelopeBlock';
import { geoLabel } from '../lib/geo';
import type { RunEnvelope } from '../api/types';

// Guards the run-envelope context line (V046 api/v2 pass-through): populated
// envelopes render the geo / clock / load facts, the noisy-tester warning
// keys on load vs client_info.cpu_cores, and absent data renders NOTHING —
// old runs (no envelope member) must not grow an empty container.

const fullEnvelope: RunEnvelope = {
  client_geo: { country: 'US', asn: 13335, as_org: 'Cloudflare' },
  target_geo: { country: 'DE', city: 'Falkenstein', asn: 24940, as_org: 'Hetzner' },
  clock_sync: { ntp_server: 'pool.ntp.org:123', offset_ms: -3.25, round_trip_ms: 12.1 },
  client_load_before: { load_avg_1m: 0.42 },
  client_load_after: { load_avg_1m: 0.88 },
  client_info: { os: 'linux', arch: 'x86_64', cpu_cores: 4 },
};

describe('geoLabel', () => {
  it('joins country, city and ASN org with middle dots', () => {
    expect(geoLabel({ country: 'US', city: 'Ashburn', asn: 13335, as_org: 'Cloudflare' }))
      .toBe('US · Ashburn · AS13335 Cloudflare');
  });

  it('renders "US · AS13335 Cloudflare" style without a city', () => {
    expect(geoLabel({ country: 'US', asn: 13335, as_org: 'Cloudflare' }))
      .toBe('US · AS13335 Cloudflare');
  });

  it('handles ASN without org and org without ASN', () => {
    expect(geoLabel({ country: 'SE', asn: 1257 })).toBe('SE · AS1257');
    expect(geoLabel({ country: 'SE', as_org: 'Tele2' })).toBe('SE · Tele2');
  });

  it('returns null for absent or empty geo', () => {
    expect(geoLabel(null)).toBeNull();
    expect(geoLabel(undefined)).toBeNull();
    expect(geoLabel({})).toBeNull();
  });
});

describe('RunEnvelopeBlock', () => {
  it('renders geo, clock offset and load for a populated envelope', () => {
    render(<RunEnvelopeBlock envelope={fullEnvelope} />);

    expect(screen.getByText('US · AS13335 Cloudflare')).toBeInTheDocument();
    expect(screen.getByText('DE · Falkenstein · AS24940 Hetzner')).toBeInTheDocument();
    expect(screen.getByText('-3.3ms')).toBeInTheDocument();
    expect(screen.getByText('0.42 → 0.88')).toBeInTheDocument();
    expect(screen.getByText('(4 cores)')).toBeInTheDocument();
    // Load (0.88) is under the 4-core count — no contention warning.
    expect(screen.queryByText(/tester contended/)).not.toBeInTheDocument();
  });

  it('prefixes a positive clock offset with +', () => {
    render(<RunEnvelopeBlock envelope={{ clock_sync: { offset_ms: 2.5 } }} />);
    expect(screen.getByText('+2.5ms')).toBeInTheDocument();
  });

  it('warns when load_avg_1m exceeded cpu_cores on either sample', () => {
    render(
      <RunEnvelopeBlock
        envelope={{
          ...fullEnvelope,
          client_load_before: { load_avg_1m: 0.5 },
          client_load_after: { load_avg_1m: 6.1 }, // > 4 cores
        }}
      />
    );

    expect(screen.getByText(/tester contended/)).toBeInTheDocument();
    expect(screen.getByText(/load exceeded 4 cores/)).toBeInTheDocument();
  });

  it('does not warn without cpu_cores (no client_info in the envelope)', () => {
    render(
      <RunEnvelopeBlock
        envelope={{ client_load_before: { load_avg_1m: 99 } }}
      />
    );

    expect(screen.getByText('99.00 → ?')).toBeInTheDocument();
    expect(screen.queryByText(/tester contended/)).not.toBeInTheDocument();
  });

  it('renders nothing when the envelope is absent (old runs)', () => {
    const { container: missing } = render(<RunEnvelopeBlock envelope={undefined} />);
    expect(missing).toBeEmptyDOMElement();

    const { container: nulled } = render(<RunEnvelopeBlock envelope={null} />);
    expect(nulled).toBeEmptyDOMElement();
  });

  it('renders nothing when no displayed field is present', () => {
    // client_network/server_info alone carry nothing this line shows.
    const { container } = render(
      <RunEnvelopeBlock envelope={{ client_network: { default_interface: 'en0' } }} />
    );
    expect(container).toBeEmptyDOMElement();
  });
});
