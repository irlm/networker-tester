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

  it('classifies throughput / UDP / page-load as needing a networker-endpoint', () => {
    for (const m of ['download', 'upload', 'download3', 'webupload', 'udp', 'udpdownload',
      'pageload', 'pageload3', 'browser2']) {
      expect(requirementOf(m)).toBe('networker-endpoint');
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
  it('URL target: only any-target modes are allowed', () => {
    expect(isModeSupported('http3', url)).toBe(true);
    expect(isModeSupported('tls', url)).toBe(true);
    // endpoint-only modes fail against a raw URL — the "always fails" case.
    expect(isModeSupported('download', url)).toBe(false);
    expect(isModeSupported('udpdownload', url)).toBe(false);
    expect(isModeSupported('pageload3', url)).toBe(false);
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
