// #765 (routing): six legacy bookmark routes once resolved wrong because the
// `<Navigate relative="path" to="..">` depth miscounted URL segments, so
// /runs/new, /runs/new/probe, /vms/testers, /vms/endpoints, /benchmark-wizard
// and /app-benchmark-wizard fell through to the /projects catch-all or a 404
// run page. #766 corrected the depths and #779 refactored App.tsx around them;
// nothing pinned the behavior. This test parses the redirect <Route>s straight
// out of App.tsx (the route-contract.test.ts approach: read the real seam, no
// duplicate table), replays each with react-router's own path resolution, and
// asserts every legacy URL terminates on its intended real page — never the
// catch-all.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { matchRoutes, resolvePath } from 'react-router';

const HERE = dirname(fileURLToPath(import.meta.url));
const APP_SOURCE = readFileSync(join(HERE, 'App.tsx'), 'utf8');

/** Every `<Route path="…">` in App.tsx (conditional platform routes included). */
function allRoutePaths(): string[] {
  return [...APP_SOURCE.matchAll(/<Route\s+path="([^"]+)"/gs)].map((m) => m[1]);
}

/** The path-relative redirects: route path → Navigate `to`. */
function pathRelativeRedirects(): Map<string, string> {
  const out = new Map<string, string>();
  const re = /<Route\s+path="([^"]+)"\s+element=\{<Navigate\s+to=(?:"([^"]+)"|\{`([^`]+)`\})\s+replace\s+relative="path"\s*\/>\}/gs;
  for (const m of APP_SOURCE.matchAll(re)) {
    out.set(m[1], m[2] ?? m[3]);
  }
  return out;
}

/** Substitute sample values for the params a legacy URL needs. */
function fillParams(routePath: string): string {
  return routePath
    .replace(':projectId', 'p1')
    .replace(':jobId', 'j1')
    .replace(':configId', 'c1')
    .replace(':runId', 'r1');
}

/**
 * Follow a URL through App.tsx routing: rank-match it against every route
 * (react-router's own matchRoutes, so static beats dynamic exactly as in the
 * app), and while the matched route is a path-relative redirect, resolve its
 * `to` against the current pathname (react-router's own resolvePath — the
 * exact semantics of `<Navigate relative="path">`).
 */
function follow(startPathname: string): { pathname: string; routePath: string } {
  const redirects = pathRelativeRedirects();
  const routes = allRoutePaths().map((path) => ({ path }));
  let pathname = startPathname;
  for (let hop = 0; hop < 5; hop++) {
    const matches = matchRoutes(routes, { pathname });
    expect(matches, `no route matched ${pathname} (from ${startPathname})`).toBeTruthy();
    const routePath = matches![matches!.length - 1].route.path!;
    const to = redirects.get(routePath);
    if (to === undefined) return { pathname, routePath };
    pathname = resolvePath(to, pathname).pathname;
  }
  throw new Error(`redirect loop starting at ${startPathname}`);
}

describe('legacy redirect routes (App.tsx)', () => {
  it('extracts the path-relative redirect table', () => {
    // Sanity for the source regex: all the known legacy redirects are seen.
    const redirects = pathRelativeRedirects();
    expect(redirects.size).toBeGreaterThanOrEqual(13);
    expect(redirects.get('/projects/:projectId/runs/new')).toBe('../../tests/new');
  });

  // The six routes from the #765 E2E pass, plus the remaining legacy
  // redirects — every one must land on its intended page.
  const expectations: Record<string, string> = {
    '/projects/p1/runs/new': '/projects/p1/tests/new',
    '/projects/p1/runs/new/probe': '/projects/p1/probe',
    '/projects/p1/vms/testers': '/projects/p1/vms',
    '/projects/p1/vms/endpoints': '/projects/p1/vms',
    '/projects/p1/benchmark-wizard': '/projects/p1/tests/new',
    '/projects/p1/app-benchmark-wizard': '/projects/p1/tests/new',
    '/projects/p1/diagnostics': '/projects/p1/probe',
    '/projects/p1/tests': '/projects/p1/runs',
    '/projects/p1/tests/j1': '/projects/p1/runs',
    '/projects/p1/benchmarks': '/projects/p1/runs',
    '/projects/p1/benchmarks/compare': '/projects/p1/runs/compare',
    '/projects/p1/benchmarks/r1': '/projects/p1/runs',
    '/projects/p1/benchmark-progress/c1': '/projects/p1/runs',
    '/projects/p1/deploy': '/projects/p1/vms',
    '/projects/p1/testers': '/projects/p1/vms',
    '/projects/p1/vm-history': '/projects/p1/vms/history',
  };

  for (const [start, terminal] of Object.entries(expectations)) {
    it(`${start} lands on ${terminal}`, () => {
      const { pathname, routePath } = follow(start);
      expect(pathname).toBe(terminal);
      // Never the catch-all — that is the exact #765 failure mode.
      expect(routePath).not.toBe('*');
    });
  }

  it('every path-relative redirect terminates on a real page, not the catch-all', () => {
    for (const routePath of pathRelativeRedirects().keys()) {
      const { routePath: terminalRoute } = follow(fillParams(routePath));
      expect(terminalRoute, `${routePath} fell through to ${terminalRoute}`).not.toBe('*');
    }
  });
});
