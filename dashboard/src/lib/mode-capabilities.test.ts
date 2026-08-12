import { describe, it, expect } from 'vitest';
import {
  requirementOf,
  unsupportedReason,
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
