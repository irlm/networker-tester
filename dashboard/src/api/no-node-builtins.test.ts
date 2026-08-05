import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

/**
 * `tsconfig.app.json` includes the `node` types so the route-contract drift
 * guard can read the C# sources off disk. That relaxation applies to the whole
 * project, so without this check a SHIPPED module could import `node:fs` and
 * still type-check cleanly.
 *
 * Vite does not save us: it emits "Module node:fs has been externalized for
 * browser compatibility" and **builds successfully** — verified, not assumed.
 * The failure then surfaces at runtime in the browser. So the rule is enforced
 * here: node builtins are for test files only.
 */

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(HERE, '..');

function sourceFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      sourceFiles(full, acc);
    } else if (/\.(ts|tsx)$/.test(entry) && !/\.(test|spec)\.(ts|tsx)$/.test(entry)) {
      acc.push(full);
    }
  }
  return acc;
}

describe('shipped frontend code', () => {
  it('never imports a node builtin', () => {
    const offenders: string[] = [];
    // `node:` prefixed, plus the bare builtins that resolve without it.
    const rx = /(?:from|import)\s*\(?\s*['"](node:[\w/]+|fs|path|url|os|child_process|crypto)['"]/g;

    for (const file of sourceFiles(SRC)) {
      const src = readFileSync(file, 'utf8');
      for (const m of src.matchAll(rx)) {
        offenders.push(`${relative(SRC, file)} imports "${m[1]}"`);
      }
    }

    expect(
      offenders,
      'these ship to the browser and would fail at runtime — vite only warns '
        + '("externalized for browser compatibility") and still builds:\n'
        + offenders.join('\n'),
    ).toEqual([]);
  });

  it('scans a plausible number of files', () => {
    // Guard the guard: an empty scan would pass forever.
    expect(sourceFiles(SRC).length).toBeGreaterThan(50);
  });
});
