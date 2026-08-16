import { describe, it, expect } from 'vitest';
import {
  requirementOf,
  unsupportedReason,
  unsupportedModes,
  isModeSupported,
  type TargetCapabilities,
} from './mode-capabilities';

const url: TargetCapabilities = { kind: 'url' };
const endpoint: TargetCapabilities = { kind: 'endpoint' };
const sdk: TargetCapabilities = { kind: 'sdk' };

describe('requirementOf', () => {
  it('classifies the network + HTTP primitives as any-target', () => {
    // ping (ICMP echo), path (hop discovery), dualstack (IPv4-vs-IPv6), and
    // pmtud (DF-bit path-MTU discovery — concludes from ICMP errors alone)
    // probe any reachable host — no endpoint routes required.
    for (const m of ['tcp', 'dns', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'curl', 'ping', 'path', 'dualstack', 'pmtud']) {
      expect(requirementOf(m)).toBe('any');
    }
  });

  it('classifies THROUGHPUT modes as needing a networker-endpoint', () => {
    for (const m of ['download', 'upload', 'download3', 'webupload', 'udpdownload', 'udpupload']) {
      expect(requirementOf(m)).toBe('networker-endpoint');
    }
  });

  it('classifies rpm (latency under load) as needing a networker-endpoint', () => {
    // rpm saturates the link via the endpoint /download route while probing
    // its UDP echo server — an arbitrary URL runs neither.
    expect(requirementOf('rpm')).toBe('networker-endpoint');
  });

  it('classifies websocket as needing a networker-endpoint', () => {
    // The message-RTT phase needs the endpoint's /ws echo route — an
    // arbitrary URL has no frame-echoing WebSocket server.
    expect(requirementOf('websocket')).toBe('networker-endpoint');
  });

  it('classifies udp + native page-load as endpoint-only (proven live 2026-08-12)', () => {
    // udp echoes off the endpoint's :9999 server; pageload* fetch the
    // endpoint's synthetic /asset ladder (404 on real sites since the
    // v0.28.81 2xx-only rule). Against a raw URL they fail by construction.
    for (const m of ['udp', 'pageload', 'pageload2', 'pageload3']) {
      expect(requirementOf(m)).toBe('networker-endpoint');
    }
  });

  it('classifies browser modes as any-target (they load the real page)', () => {
    for (const m of ['browser1', 'browser2', 'browser3']) {
      expect(requirementOf(m)).toBe('any');
    }
  });

  it('classifies sdkprobe and apibench to their special targets', () => {
    expect(requirementOf('sdkprobe')).toBe('sdk-endpoint');
    expect(requirementOf('apibench')).toBe('reference-apis');
  });

  it('is case-insensitive and defaults unknown modes to any', () => {
    expect(requirementOf('HTTP3')).toBe('any');
    expect(requirementOf('DOWNLOAD')).toBe('networker-endpoint');
    expect(requirementOf('totally-made-up')).toBe('any');
  });
});

describe('unsupportedReason / isModeSupported', () => {
  it('URL target: primitives + browser ok; endpoint-only/throughput/sdk/apibench blocked', () => {
    // The URL Probe runs these against arbitrary URLs.
    expect(isModeSupported('http3', url)).toBe(true);
    expect(isModeSupported('tls', url)).toBe(true);
    expect(isModeSupported('browser2', url)).toBe(true);
    // udp echo + native pageload asset ladder only exist on the endpoint —
    // against a raw URL they fail by construction (user-caught 2026-08-12).
    expect(isModeSupported('udp', url)).toBe(false);
    expect(isModeSupported('pageload3', url)).toBe(false);
    // Throughput needs the endpoint's servers — the "always fails" case on a raw URL.
    expect(isModeSupported('download', url)).toBe(false);
    expect(isModeSupported('udpdownload', url)).toBe(false);
    expect(isModeSupported('sdkprobe', url)).toBe(false);
    expect(isModeSupported('apibench', url)).toBe(false);
    expect(unsupportedReason('download', url)).toContain('networker-endpoint');
  });

  it('networker-endpoint target: primitives + endpoint modes ok; sdk/apibench not', () => {
    expect(isModeSupported('http2', endpoint)).toBe(true);
    expect(isModeSupported('download', endpoint)).toBe(true);
    expect(isModeSupported('browser3', endpoint)).toBe(true);
    // A raw endpoint is not an SDK endpoint and has no reference APIs.
    expect(isModeSupported('sdkprobe', endpoint)).toBe(false);
    expect(isModeSupported('apibench', endpoint)).toBe(false);
    expect(unsupportedReason('sdkprobe', endpoint)).toContain('SDK');
    expect(unsupportedReason('apibench', endpoint)).toContain('Application');
  });

  it('SDK endpoint target: sdkprobe + primitives ok; apibench still its own flow', () => {
    expect(isModeSupported('sdkprobe', sdk)).toBe(true);
    expect(isModeSupported('http1', sdk)).toBe(true);
    expect(isModeSupported('apibench', sdk)).toBe(false);
  });

  it('supported modes return a null reason', () => {
    expect(unsupportedReason('http1', endpoint)).toBeNull();
    expect(unsupportedReason('tcp', url)).toBeNull();
  });
});

describe('HTTP/3 by proxy stack (shared/http-stacks.json)', () => {
  const H3 = ['http3', 'pageload3', 'browser3', 'download3', 'upload3'];

  it('stacks with QUIC allow every h3 mode', () => {
    for (const stack of ['nginx', 'caddy', 'iis', 'endpoint']) {
      for (const m of H3) {
        expect(unsupportedReason(m, { kind: 'endpoint', stack }), `${m} on ${stack}`).toBeNull();
      }
    }
  });

  it('apache / haproxy / traefik disable exactly the h3 modes, naming the stack', () => {
    for (const stack of ['apache', 'haproxy', 'traefik']) {
      for (const m of H3) {
        const why = unsupportedReason(m, { kind: 'endpoint', stack });
        expect(why, `${m} on ${stack}`).toContain(`${stack} has no HTTP/3`);
        expect(why).toContain('shared/http-stacks.json');
      }
      // The H1/H2 siblings and the rest of the catalog stay on.
      for (const m of ['http1', 'http2', 'pageload', 'pageload2', 'browser1', 'browser2', 'download', 'upload', 'tcp', 'udp']) {
        expect(unsupportedReason(m, { kind: 'endpoint', stack }), `${m} on ${stack}`).toBeNull();
      }
    }
  });

  it('a matrix keeps h3 as long as one stack serves it; disables when none does', () => {
    expect(unsupportedReason('http3', { kind: 'endpoint', stack: ['apache', 'nginx'] })).toBeNull();
    const why = unsupportedReason('http3', { kind: 'endpoint', stack: ['apache', 'traefik'] });
    expect(why).toContain('apache, traefik has no HTTP/3');
    // No stacks picked yet → nothing to decide → fail open.
    expect(unsupportedReason('http3', { kind: 'endpoint', stack: [] })).toBeNull();
  });

  it('unknown / absent stacks fail open (never fabricate a capability)', () => {
    expect(unsupportedReason('http3', { kind: 'endpoint', stack: 'envoy' })).toBeNull();
    expect(unsupportedReason('http3', { kind: 'endpoint', stack: null })).toBeNull();
    expect(unsupportedReason('http3', { kind: 'endpoint' })).toBeNull();
    // A raw URL normally carries no stack, so http3 against a URL is fine…
    expect(unsupportedReason('http3', { kind: 'url' })).toBeNull();
    // …but a caller that DOES say the URL sits behind apache gets the same
    // answer as the server gate (the stack rule is kind-independent).
    expect(unsupportedReason('http3', { kind: 'url', stack: 'apache' })).toContain('apache has no HTTP/3');
  });

  it('the kind rule wins over the stack rule (one reason per mode)', () => {
    // pageload3 against a URL is off because it needs the endpoint asset ladder,
    // regardless of any stack hint.
    expect(unsupportedReason('pageload3', { kind: 'url', stack: 'apache' })).toContain('networker-endpoint');
  });
});

describe('live self-report + runner inventory', () => {
  it('a live unsupported map narrows on positive knowledge only', () => {
    const live = new Map([['udp', 'UDP echo listener disabled on this target']]);
    expect(unsupportedReason('udp', { kind: 'endpoint', unsupported: live })).toBe('UDP echo listener disabled on this target');
    expect(unsupportedReason('stamp', { kind: 'endpoint', unsupported: live })).toBeNull();
    // Plain-object form works too.
    expect(unsupportedReason('stamp', { kind: 'endpoint', unsupported: { stamp: 'STAMP reflector disabled on this target' } }))
      .toContain('STAMP');
  });

  it('a pinned runner without Chrome disables only the browser modes', () => {
    const noChrome = { kind: 'endpoint' as const, runner: { chrome: false, tshark: true } };
    for (const m of ['browser1', 'browser2', 'browser3']) {
      expect(unsupportedReason(m, noChrome), m).toContain('Chrome');
    }
    for (const m of ['http3', 'pageload3', 'download', 'tcp']) {
      expect(unsupportedReason(m, noChrome), m).toBeNull();
    }
    // Unknown inventory (null / auto-pick) never disables.
    expect(unsupportedReason('browser3', { kind: 'endpoint', runner: null })).toBeNull();
    expect(unsupportedReason('browser3', { kind: 'endpoint', runner: { chrome: null } })).toBeNull();
    expect(unsupportedReason('browser3', { kind: 'endpoint', runner: { chrome: true } })).toBeNull();
    // Even a URL target (browser modes are any-target) honours the runner.
    expect(unsupportedReason('browser1', { kind: 'url', runner: { chrome: false } })).toContain('Chrome');
  });

  it('unsupportedModes() collects mode → reason across every axis', () => {
    const off = unsupportedModes(
      ['tcp', 'http3', 'browser3', 'udp', 'sdkprobe'],
      { kind: 'endpoint', stack: 'haproxy', runner: { chrome: false }, unsupported: new Map([['udp', 'off']]) },
    );
    expect([...off.keys()].sort()).toEqual(['browser3', 'http3', 'sdkprobe', 'udp']);
    expect(off.get('http3')).toContain('haproxy');
    expect(off.get('browser3')).toContain('haproxy'); // stack rule fires before the runner rule
    expect(off.get('udp')).toBe('off');
  });
});
