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
    for (const m of ['tcp', 'dns', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'curl']) {
      expect(requirementOf(m)).toBe('any');
    }
  });

  it('classifies THROUGHPUT modes as needing a networker-endpoint', () => {
    for (const m of ['download', 'upload', 'download3', 'webupload', 'udpdownload', 'udpupload']) {
      expect(requirementOf(m)).toBe('networker-endpoint');
    }
  });

  it('classifies udp / page-load / browser as any-target (the URL Probe runs them against raw URLs)', () => {
    for (const m of ['udp', 'pageload', 'pageload3', 'browser1', 'browser2', 'browser3']) {
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
  it('URL target: primitives + udp + page-load + browser ok; only throughput/sdk/apibench blocked', () => {
    // The URL Probe runs all of these against arbitrary URLs.
    expect(isModeSupported('http3', url)).toBe(true);
    expect(isModeSupported('tls', url)).toBe(true);
    expect(isModeSupported('udp', url)).toBe(true);
    expect(isModeSupported('pageload3', url)).toBe(true);
    expect(isModeSupported('browser2', url)).toBe(true);
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
