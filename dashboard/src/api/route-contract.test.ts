import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { testersApi } from './testers';

/**
 * Audit P1-12: the frontend suite mocks `../api/client`, so every test proves
 * what a component does with a STUBBED client and nothing proves the client
 * talks to routes the server actually serves. Rename a route on either side
 * and all 44 test files stay green while the feature 404s in the browser.
 *
 * This is the contract check MSW would give us, without the dependency: the
 * REAL api functions run against a spied `fetch`, and every URL they build is
 * matched against the routes the control plane actually registers — parsed
 * from the C# sources at test time, so the two sides cannot drift silently.
 *
 * Same family as the `shared/modes.json` guard and the installer ⇄ release.yml
 * asset-name guard: a cross-stack seam is only safe when something reads both
 * ends. This repo has been bitten repeatedly by written-but-never-read seams.
 */

const REPO_ROOT = resolve(__dirname, '../../..');
const CONTROL_PLANE = join(REPO_ROOT, 'src/Networker.ControlPlane');

/** Every `.cs` file under the control plane. */
function csharpSources(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    if (entry === 'obj' || entry === 'bin') continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) csharpSources(full, acc);
    else if (entry.endsWith('.cs')) acc.push(full);
  }
  return acc;
}

/** Strip the query string and any trailing slash. */
function normalize(path: string): string {
  return path.split('?')[0].replace(/\/+$/, '');
}

/**
 * A server route template (`/api/projects/{projectId}/testers/{id:guid}`)
 * becomes a matcher where each `{...}` accepts exactly ONE path segment. The
 * frontend sends concrete values (`/api/projects/p1/testers/t1`), so string
 * equality would never hold — the SHAPE is what has to match.
 */
function toMatcher(template: string): RegExp {
  const escaped = normalize(template)
    .replace(/[.*+?^$()|[\]\\]/g, '\\$&')   // escape regex metacharacters
    .replace(/\{[^}]*\}/g, '[^/]+');        // a param spans exactly one segment
  return new RegExp(`^${escaped}$`);
}

/**
 * Routes the control plane registers via Map{Get,Post,Put,Delete,Patch}.
 *
 * Many are built by interpolation — `app.MapPost($"{basePath}/start", ...)`
 * with `const string basePath = "/api/projects/{projectId}/testers/{testerId:guid}"`
 * a few lines above. A parser that only reads plain string literals silently
 * misses those and then reports the frontend as broken, so file-local
 * `const string` values are resolved first.
 */
function serverRoutes(): string[] {
  const routes = new Set<string>();
  const routeRx = /Map(?:Get|Post|Put|Delete|Patch)\(\s*\$?"([^"]+)"/g;
  const constRx = /const\s+string\s+(\w+)\s*=\s*"([^"]+)"\s*;/g;

  for (const file of csharpSources(CONTROL_PLANE)) {
    const src = readFileSync(file, 'utf8');

    const consts = new Map<string, string>();
    for (const c of src.matchAll(constRx)) consts.set(c[1], c[2]);

    for (const m of src.matchAll(routeRx)) {
      // Substitute known const names BEFORE `{...}` is treated as a route
      // param — `{basePath}` and `{projectId}` are indistinguishable otherwise.
      let route = m[1];
      for (const [name, value] of consts) {
        route = route.split(`{${name}}`).join(value);
      }
      if (route.startsWith('/')) routes.add(normalize(route));
    }
  }
  return [...routes];
}

const ROUTES = serverRoutes();
const MATCHERS = ROUTES.map((r) => toMatcher(r));

function isRegistered(path: string): boolean {
  return MATCHERS.some((rx) => rx.test(path));
}

type Call = { method: string; path: string };

/** Drive the REAL api functions and record what they ask `fetch` for. */
async function recordCalls(
  invoke: () => Promise<unknown>,
): Promise<Call[]> {
  const calls: Call[] = [];
  const spy = (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    calls.push({ method: (init?.method ?? 'GET').toUpperCase(), path: url });
    return Promise.resolve(
      new Response('{}', { status: 200, headers: { 'Content-Type': 'application/json' } }),
    );
  };
  const original = globalThis.fetch;
  globalThis.fetch = spy as typeof fetch;
  try {
    await invoke();
  } finally {
    globalThis.fetch = original;
  }
  return calls;
}

describe('frontend ⇄ control-plane route contract', () => {
  beforeEach(() => {
    // request() reads a token and records perf entries; a plain store is enough.
    globalThis.localStorage?.clear?.();
  });

  afterEach(() => {
    globalThis.localStorage?.clear?.();
  });

  it('parses a plausible number of server routes', () => {
    // Guard the guard: if the C# scan silently stopped matching, every
    // assertion below would pass against an empty set and prove nothing.
    expect(ROUTES.length).toBeGreaterThan(50);
    expect(ROUTES.some((r) => r.startsWith('/api/'))).toBe(true);
    // …the matcher really accepts a known-good concrete URL…
    expect(isRegistered('/api/projects/anything/testers')).toBe(true);
    // …and rejects an obviously wrong one. A matcher that accepted everything
    // would make every assertion below meaningless.
    expect(isRegistered('/api/definitely/not/a/route/here')).toBe(false);
  });

  // Every testersApi member, with placeholder arguments. Listed explicitly
  // rather than reflected so a NEW api function fails this test until someone
  // adds it here — silently skipping unknown members is how a contract guard
  // rots into a no-op.
  const testerCalls: Array<[string, () => Promise<unknown>]> = [
    ['listTesters', () => testersApi.listTesters('p1')],
    ['getTester', () => testersApi.getTester('p1', 't1')],
    ['getCostEstimate', () => testersApi.getCostEstimate('p1', 't1')],
    ['startTester', () => testersApi.startTester('p1', 't1')],
    ['stopTester', () => testersApi.stopTester('p1', 't1')],
    ['upgradeTester', () => testersApi.upgradeTester('p1', 't1', { confirm: true })],
    ['deleteTester', () => testersApi.deleteTester('p1', 't1')],
    ['createTester', () => testersApi.createTester('p1', {} as never)],
    ['forceStop', () => testersApi.forceStop('p1', 't1', {} as never)],
    ['postpone', () => testersApi.postpone('p1', 't1', {} as never)],
    ['probe', () => testersApi.probe('p1', 't1')],
    ['refreshLatestVersion', () => testersApi.refreshLatestVersion('p1')],
    ['rotateKey', () => testersApi.rotateKey('p1', 't1')],
    ['updateSchedule', () => testersApi.updateSchedule('p1', 't1', {} as never)],
  ];

  it.each(testerCalls)('%s targets a route the server registers', async (name, invoke) => {
    const calls = await recordCalls(invoke);
    expect(calls.length, `${name} issued no fetch at all`).toBeGreaterThan(0);

    for (const call of calls) {
      const normalized = normalize(call.path);
      expect(
        isRegistered(normalized),
        `${name} → ${call.method} ${call.path}\n`
          + `normalized to "${normalized}", which matches no route the control plane registers.\n`
          + `Either the frontend path is wrong or the server route was renamed — `
          + `this is exactly the drift the mocked-client tests cannot see.`,
      ).toBe(true);
    }
  });

  it('covers every exported testersApi function', () => {
    // The list above is manual; this makes forgetting to extend it a failure
    // rather than a silent coverage hole.
    const exported = Object.keys(testersApi).sort();
    const covered = testerCalls.map(([name]) => name).sort();
    const uncovered = exported.filter((k) => !covered.includes(k));
    expect(
      uncovered,
      `testersApi gained function(s) with no route-contract coverage: ${uncovered.join(', ')}`,
    ).toEqual([]);
  });
});
