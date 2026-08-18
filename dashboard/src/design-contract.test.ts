import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(HERE);

function shippedFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      shippedFiles(full, acc);
    } else if (/\.(css|ts|tsx)$/.test(entry) && !/\.(test|spec)\.(ts|tsx)$/.test(entry)) {
      acc.push(full);
    }
  }
  return acc;
}

function findings(pattern: RegExp, description: string): string[] {
  const matches: string[] = [];
  for (const file of shippedFiles(SRC)) {
    const source = readFileSync(file, 'utf8');
    source.split('\n').forEach((line, index) => {
      if (pattern.test(line)) {
        matches.push(`${relative(SRC, file)}:${index + 1} ${description}`);
      }
      pattern.lastIndex = 0;
    });
  }
  return matches;
}

function fileFindings(pattern: RegExp, description: string): string[] {
  const matches: string[] = [];
  for (const file of shippedFiles(SRC)) {
    const source = readFileSync(file, 'utf8');
    if (pattern.test(source)) matches.push(`${relative(SRC, file)} ${description}`);
    pattern.lastIndex = 0;
  }
  return matches;
}

describe('DESIGN.md implementation contract', () => {
  it('keeps shipped typography on the 12px-or-larger type ramp', () => {
    expect(
      findings(
        /(?:text-\[(?:[0-9]|1[01])px\]|fontSize\s*=\s*\{\s*(?:[0-9]|1[01])(?:\.\d+)?\s*\}|fontSize\s*:\s*(?:[0-9]|1[01])(?:\.\d+)?\b|font-size\s*:\s*(?:[0-9]|1[01])(?:\.\d+)?px)/g,
        'uses literal type below the 12px data token',
      ),
    ).toEqual([]);
  });

  it('uses flat bordered depth instead of shadows', () => {
    expect(
      findings(/(?:\bbox-shadow\s*:|\bshadow-(?:\[[^\]]+\]|[a-z0-9]+))/gi, 'introduces a forbidden shadow'),
    ).toEqual([]);
  });

  it('does not introduce gradient UI treatments', () => {
    expect(
      findings(/(?:\bbg-gradient-|\btext-transparent\b.*\bbg-clip-text\b|\b(?:linear|radial)-gradient\s*\()/gi, 'introduces a forbidden gradient'),
    ).toEqual([]);
  });

  it('keeps dark text on cyan action fills for WCAG AA contrast', () => {
    expect(
      [
        ...findings(
          /(?:bg-cyan-(?:500|600|700)(?!\/)[^\n]*text-white|text-white[^\n]*bg-cyan-(?:500|600|700)(?!\/))/g,
          'uses low-contrast white text on an interactive cyan fill',
        ),
        ...fileFindings(
          /className=\{`[^`]{0,180}text-white[^`]*\$\{[\s\S]{0,180}bg-cyan-(?:500|600|700)(?!\/)/g,
          'applies white text outside a dynamic cyan action branch',
        ),
      ],
    ).toEqual([]);
  });

  it('scans a plausible shipped surface', () => {
    expect(shippedFiles(SRC).length).toBeGreaterThan(100);
  });
});
