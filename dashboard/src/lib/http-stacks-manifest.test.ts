// Drift guard: shared/http-stacks.json ⇄ dashboard/src/lib/http-stacks.ts.
//
// The canonical manifest (repo root shared/http-stacks.json) is embedded by the
// Rust tester (http_stacks.rs) and the control plane (HttpStackCatalog, guarded
// by HttpStacksManifestTests.cs) and read by lab/validate.sh. This guards the
// dashboard hand-copy the mode pickers gate on — the same pattern as
// modes-manifest.test.ts for shared/modes.json.

import { describe, it, expect } from 'vitest';
import { HTTP_STACKS, H3_MODES, stackHasH3, anyStackHasH3, isH3Mode, stacksWithoutH3 } from './http-stacks';
import manifestJson from '../../../shared/http-stacks.json';
import modesJson from '../../../shared/modes.json';
import { PROXY_LABELS } from '../components/wizard/testbed-constants';

interface Manifest {
  h3_modes: string[];
  stacks: { id: string; http_port: number; https_port: number; h3: boolean }[];
}

const manifest = manifestJson as unknown as Manifest;
const modeIds = new Set((modesJson as unknown as { modes: { id: string }[] }).modes.map(m => m.id));

describe('shared/http-stacks.json ⇄ lib/http-stacks.ts', () => {
  it('lists exactly the manifest stacks, in order, with the same h3 flag', () => {
    expect(HTTP_STACKS.map(s => s.id)).toEqual(manifest.stacks.map(s => s.id));
    for (const s of manifest.stacks) {
      expect(stackHasH3(s.id), `${s.id} h3`).toBe(s.h3);
    }
  });

  it('lists exactly the manifest h3_modes, and every one is a real mode id', () => {
    expect([...H3_MODES]).toEqual(manifest.h3_modes);
    for (const m of manifest.h3_modes) {
      expect(modeIds.has(m), `${m} is in shared/modes.json`).toBe(true);
      expect(isH3Mode(m)).toBe(true);
    }
  });

  it('knows every proxy the wizards offer (testbed-constants PROXY_LABELS)', () => {
    // A proxy the wizard can pick but the manifest does not know would fail
    // open silently — every picker id must resolve to a manifest row.
    for (const id of Object.keys(PROXY_LABELS)) {
      expect(stackHasH3(id), `${id} in shared/http-stacks.json`).not.toBeNull();
    }
  });
});

describe('http-stacks helpers', () => {
  it('fails open on unknown or blank stacks', () => {
    expect(stackHasH3('envoy')).toBeNull();
    expect(stackHasH3('')).toBeNull();
    expect(stackHasH3(null)).toBeNull();
    expect(stackHasH3(undefined)).toBeNull();
    expect(anyStackHasH3([])).toBeNull();
    expect(anyStackHasH3(['envoy'])).toBeNull();
  });

  it('is case-insensitive like the Rust by_name()', () => {
    expect(stackHasH3('NGINX')).toBe(true);
    expect(stackHasH3(' Apache ')).toBe(false);
    expect(isH3Mode('HTTP3')).toBe(true);
    expect(isH3Mode('http2')).toBe(false);
  });

  it('a mixed matrix has h3 as long as one stack serves it', () => {
    expect(anyStackHasH3(['apache', 'nginx'])).toBe(true);
    expect(anyStackHasH3(['apache', 'haproxy', 'traefik'])).toBe(false);
    expect(anyStackHasH3(['apache', 'envoy'])).toBe(false);
    expect(stacksWithoutH3(['nginx', 'apache', 'envoy', 'traefik'])).toEqual(['apache', 'traefik']);
  });
});
