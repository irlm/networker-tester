// Single source of truth for navigation: the sidebar, the `/` command
// palette, and the `g`-key sequences all consume this list. Add a page
// here and every navigation surface picks it up; there is no second list
// to forget.
//
// Structure (workflow, not product taxonomy — 2026-08 UI pass):
//   main    — the working loop: see state, start work, read results
//   reports — derived/analysis views over finished runs
//   admin   — platform-scoped, role-gated
// Project Settings is deliberately NOT in `admin`: it is project-scoped
// and member-visible; parking it under ADMIN hid it from the people it
// serves.
//
// Creation flows: "Start a test" (the scenario gallery) is the single
// entry point for all four test types — the per-type builder pages are
// one click deeper and stay reachable from the gallery, the palette, and
// in-page buttons. Keeping four builders out of the top level is what
// keeps this list short.

export type NavGate = 'project' | 'admin' | 'platformAdmin';

export interface NavEntry {
  /** Sidebar label — also the palette's primary text. */
  label: string;
  /** Terminal-safe glyph (no emoji, no macOS-specific symbols). */
  icon: string;
  /** Route; `:pid` is replaced with the active project id. */
  path: string;
  section: 'main' | 'reports' | 'admin';
  /** Highlight only on exact match (pages that are prefixes of others). */
  exact?: boolean;
  /** `g`+key jump. Keep the whole vocabulary at a dozen keys or fewer. */
  gKey?: string;
  /** Extra palette search terms (synonyms users actually type). */
  keywords?: string[];
  gate: NavGate;
}

export const NAV_ENTRIES: NavEntry[] = [
  // ── main ──────────────────────────────────────────────────────────────
  { label: 'Dashboard', icon: '◈', path: '/projects/:pid', section: 'main', exact: true, gKey: 'd', keywords: ['home', 'overview'], gate: 'project' },
  { label: 'Start a test', icon: '★', path: '/projects/:pid/scenarios', section: 'main', gKey: 'n', keywords: ['new', 'create', 'scenario', 'probe', 'network', 'benchmark', 'full stack', 'application', 'wizard'], gate: 'project' },
  { label: 'Runs', icon: '▶', path: '/projects/:pid/runs', section: 'main', gKey: 'r', keywords: ['results', 'history', 'tests'], gate: 'project' },
  { label: 'URL Probe', icon: '✓', path: '/projects/:pid/probe', section: 'main', gKey: 'p', keywords: ['diagnostics', 'url', 'watch', 'tls', 'certificate'], gate: 'project' },
  { label: 'Infrastructure', icon: '▣', path: '/projects/:pid/vms', section: 'main', gKey: 'i', keywords: ['vms', 'runners', 'targets', 'deploy', 'testers'], gate: 'project' },
  { label: 'SDK Endpoints', icon: '✦', path: '/projects/:pid/sdk-endpoints', section: 'main', gKey: 'e', keywords: ['sdk', 'embed', 'diagnostic endpoint'], gate: 'project' },
  { label: 'Schedules', icon: '↻', path: '/projects/:pid/schedules', section: 'main', gKey: 's', keywords: ['cron', 'recurring'], gate: 'project' },
  { label: 'Alerts', icon: '⚠', path: '/projects/:pid/alerts', section: 'main', gKey: 'a', keywords: ['notifications', 'rules', 'channels'], gate: 'project' },

  // ── reports ───────────────────────────────────────────────────────────
  { label: 'Value', icon: '$', path: '/projects/:pid/reports/value', section: 'reports', gKey: 'v', keywords: ['cost', 'price', 'performance per cost', 'provider'], gate: 'project' },
  { label: 'App Network', icon: '◐', path: '/projects/:pid/reports/app-network', section: 'reports', keywords: ['application network performance', 'report'], gate: 'project' },
  { label: 'Leaderboard', icon: '♖', path: '/leaderboard', section: 'reports', gKey: 'l', keywords: ['benchmark', 'ranking', 'languages', 'comparison'], gate: 'project' },
  { label: 'Regressions', icon: '≠', path: '/projects/:pid/benchmark-regressions', section: 'reports', gKey: 'x', keywords: ['benchmark regressions', 'slower', 'detection'], gate: 'project' },

  // ── admin (platform scope) ────────────────────────────────────────────
  { label: 'System', icon: '▦', path: '/admin/system', section: 'admin', keywords: ['health', 'usage', 'logs', 'auth'], gate: 'platformAdmin' },
  { label: 'Tokens', icon: '⚿', path: '/bench-tokens', section: 'admin', keywords: ['bench tokens', 'api keys'], gate: 'platformAdmin' },
  { label: 'Perf Log', icon: '⏱', path: '/admin/perf-log', section: 'admin', keywords: ['performance', 'latency', 'slow requests'], gate: 'platformAdmin' },
  { label: 'Users', icon: '♟', path: '/users', section: 'admin', keywords: ['accounts', 'members', 'pending'], gate: 'admin' },
];

/** Deeper pages the palette can jump to that don't earn a sidebar slot. */
export const PALETTE_EXTRA_ENTRIES: NavEntry[] = [
  { label: 'New network test', icon: '▷', path: '/projects/:pid/tests/new', section: 'main', keywords: ['tcp', 'dns', 'udp', 'http', 'throughput', 'modes'], gate: 'project' },
  { label: 'New full-stack benchmark', icon: '▤', path: '/projects/:pid/benchmarks/full-stack/new', section: 'main', keywords: ['stack', 'nginx', 'iis', 'caddy', 'workload'], gate: 'project' },
  { label: 'New application benchmark', icon: '▥', path: '/projects/:pid/benchmarks/application/new', section: 'main', keywords: ['languages', 'matrix', 'apibench', 'rust', 'go', 'python'], gate: 'project' },
  { label: 'TLS profiles', icon: '≡', path: '/projects/:pid/tls-profiles', section: 'main', keywords: ['tls', 'certificates', 'handshake', 'profile history'], gate: 'project' },
  { label: 'VM catalog', icon: '□', path: '/projects/:pid/benchmark-catalog', section: 'main', keywords: ['vm sizes', 'benchmark catalog', 'pricing', 'skus'], gate: 'project' },
  { label: 'VM history', icon: '☷', path: '/projects/:pid/vms/history', section: 'main', keywords: ['lifecycle', 'deleted vms', 'audit'], gate: 'project' },
  { label: 'Project settings', icon: '⚙', path: '/projects/:pid/settings', section: 'main', keywords: ['members', 'cloud accounts', 'share links', 'approvals'], gate: 'project' },
  { label: 'Projects', icon: '○', path: '/projects', section: 'main', keywords: ['switch project', 'workspaces'], gate: 'project' },
  { label: 'Token history', icon: '⚿', path: '/bench-tokens?tab=history', section: 'admin', keywords: ['revoked', 'expired tokens'], gate: 'platformAdmin' },
];

export function resolveNavPath(path: string, pid: string | null): string | null {
  if (path.includes(':pid')) {
    if (!pid) return null;
    return path.replace(':pid', pid);
  }
  return path;
}

export function navEntryVisible(
  entry: NavEntry,
  ctx: { pid: string | null; isAdmin: boolean; isPlatformAdmin: boolean },
): boolean {
  if (entry.gate === 'platformAdmin' && !ctx.isPlatformAdmin) return false;
  if (entry.gate === 'admin' && !(ctx.isAdmin || ctx.isPlatformAdmin)) return false;
  if (entry.path.includes(':pid') && !ctx.pid) return false;
  return true;
}
