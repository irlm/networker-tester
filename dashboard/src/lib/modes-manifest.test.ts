// Drift guard: shared/modes.json ⇄ dashboard mode copies.
//
// The canonical manifest (repo root shared/modes.json) is generated from the
// engine truth (crates/networker-tester/src/metrics.rs Protocol enum) and is
// guarded on the Rust side by modes_manifest_guard.rs and on the C# side by
// ModesManifestTests.cs. This file guards the two dashboard copies:
//
//   - components/common/mode-family.ts  (chip-family lookup)
//   - components/wizard/testbed-constants.ts (RUNTIME_TEMPLATES defaultModes)
//
// The 6-way unguarded copy of this list already shipped bugs (#377-379);
// this test exists so the next drift fails CI instead of production.

import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect } from 'vitest';
import { familyOf, FAMILY_BY_MODE } from '../components/common/mode-family';
import { RUNTIME_TEMPLATES } from '../components/wizard/testbed-constants';

interface ManifestMode {
  id: string;
  level: 'tester' | 'runner';
  catalog: boolean;
  family: string;
}

interface Manifest {
  modes: ManifestMode[];
  cli_aliases: Record<string, string>;
  cli_shorthands: Record<string, string[]>;
}

const here = dirname(fileURLToPath(import.meta.url));
const manifest: Manifest = JSON.parse(
  readFileSync(resolve(here, '../../../shared/modes.json'), 'utf-8'),
);

const modeIds = new Set(manifest.modes.map(m => m.id));
const aliasKeys = new Set(
  Object.keys(manifest.cli_aliases).filter(k => !k.startsWith('$')),
);

describe('shared/modes.json manifest sanity', () => {
  it('has tester and runner modes', () => {
    expect(manifest.modes.some(m => m.level === 'tester')).toBe(true);
    expect(manifest.modes.some(m => m.level === 'runner')).toBe(true);
  });

  it('has unique mode ids', () => {
    expect(modeIds.size).toBe(manifest.modes.length);
  });
});

describe('mode-family.ts ⇄ manifest', () => {
  it('maps every manifest mode to its manifest family', () => {
    for (const m of manifest.modes) {
      expect(familyOf(m.id), `familyOf(${m.id})`).toBe(m.family);
    }
  });

  it('maps CLI aliases to the same family as their target', () => {
    for (const [alias, target] of Object.entries(manifest.cli_aliases)) {
      if (alias.startsWith('$')) continue;
      expect(familyOf(alias), `familyOf(${alias})`).toBe(familyOf(target));
    }
  });

  it('contains no stale ids (every key is a manifest id or a CLI alias)', () => {
    for (const key of Object.keys(FAMILY_BY_MODE)) {
      expect(
        modeIds.has(key) || aliasKeys.has(key),
        `FAMILY_BY_MODE key "${key}" is not a manifest mode id or CLI alias — ` +
          'this is exactly the drift that shipped bugs #377-379',
      ).toBe(true);
    }
  });

  it('falls back to "other" for unknown modes', () => {
    expect(familyOf('definitely-not-a-mode')).toBe('other');
  });
});

describe('testbed-constants.ts RUNTIME_TEMPLATES ⇄ manifest', () => {
  it('every template default mode is a manifest catalog mode', () => {
    for (const t of RUNTIME_TEMPLATES) {
      for (const mode of t.defaultModes) {
        const m = manifest.modes.find(mm => mm.id === mode);
        expect(m, `template "${t.id}" mode "${mode}" missing from manifest`).toBeDefined();
        expect(
          m!.catalog,
          `template "${t.id}" uses non-catalog mode "${mode}"`,
        ).toBe(true);
      }
    }
  });

  it('apibench stays a runner-level mode (never sent to the tester binary)', () => {
    const apibench = manifest.modes.find(m => m.id === 'apibench');
    expect(apibench).toBeDefined();
    expect(apibench!.level).toBe('runner');
    // The api-compute template depends on the agent expanding apibench.
    const apiCompute = RUNTIME_TEMPLATES.find(t => t.id === 'api-compute');
    expect(apiCompute?.defaultModes).toContain('apibench');
  });
});
