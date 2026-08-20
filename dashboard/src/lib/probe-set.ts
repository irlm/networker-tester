/**
 * URL-set selection and multi-URL entry logic (#782 P1) — pure, so the probe
 * page's set behaviour is testable without rendering it.
 *
 * Two entry points feed the same set:
 *  - the multi-URL textarea (one URL per line, paste-friendly), parsed by
 *    {@link parseUrlSetInput};
 *  - the watchlist checkboxes, resolved by {@link hostsForSelection}.
 *
 * Both end at `buildDiagRequest` (lib/diag-request.ts), which turns the entries
 * into ONE `endpoint.hosts[]` config → one run → repeated tester `--target`
 * flags. Nothing here silently drops a line: every rejected entry comes back in
 * `invalid` / `overflow` with a reason the UI shows, because a set that quietly
 * probes 3 of the 4 URLs you pasted is exactly the dishonest state #782 is
 * trying to remove.
 */

import { dedupeProbeEntries, toProbeUrl } from './diag-request';

/**
 * Members allowed in one set. A set is ONE tester process probing every URL in
 * sequence, so members multiply wall clock: the page's own watchdog headroom
 * (PRESET_EST_SECS × samples × URLs, capped at 7200s in buildDiagRequest) stops
 * buying headroom somewhere around a Full×5 over 32 URLs. 25 keeps the largest
 * offered workload inside its own ceiling; past that the honest answer is to
 * split the set, not to launch a run the watchdog will kill halfway.
 * Mirrored server-side by TestConfigEndpointNormalizer.MaxSetHosts.
 */
export const MAX_SET_URLS = 25;

/** One accepted member of a parsed set. */
export interface UrlSetEntry {
  /** The line exactly as the user typed it (what the launch path re-parses). */
  raw: string;
  /** Normalized probe URL — the de-duplication identity. */
  url: string;
  /** Hostname, for display and watchlist grouping. */
  host: string;
}

/** A line that will NOT be probed, and why. */
export interface UrlSetReject {
  raw: string;
  reason: string;
}

export interface UrlSetParse {
  /** Accepted members, de-duplicated, in first-seen order. */
  entries: UrlSetEntry[];
  /** Lines dropped because an earlier line resolves to the same probe URL. */
  duplicates: UrlSetReject[];
  /** Lines that are not probeable URLs at all. */
  invalid: UrlSetReject[];
  /** Valid, unique lines past {@link MAX_SET_URLS}. */
  overflow: UrlSetReject[];
}

/**
 * Schemes the probe path can actually drive. The tester speaks HTTP over
 * TCP/QUIC and derives DNS/TCP/TLS/ping/path targets from the same URL; an
 * `ftp://` or `ssh://` line has no probe to run, so it is rejected here rather
 * than dispatched to fail per-attempt on the runner.
 */
const PROBEABLE_SCHEMES = new Set(['http:', 'https:']);

/**
 * Split a multi-URL entry box into lines/tokens. Newlines are the documented
 * separator (one URL per line), but commas, tabs and spaces are accepted too so
 * a comma-separated paste or a single-line entry parses identically — the
 * single-URL input and the textarea must never disagree about what was typed.
 */
export function splitUrlSetInput(text: string): string[] {
  return text.split(/[\s,]+/).map(token => token.trim()).filter(Boolean);
}

/**
 * Classify one raw entry. Returns the normalized member, or the reason it
 * cannot be probed.
 *
 * Deliberately permissive about SHAPE: a single-label hostname (`intranet`,
 * `wiki`) is a real target on a corporate network, and this page exists to
 * probe arbitrary URLs — so it is accepted, and a name that does not resolve
 * fails per-URL on the runner where the error says so. Rejecting a legitimate
 * internal host to catch a stray word from a paste would be the worse trade.
 * The one shape rule is that a hostname must contain a letter or a digit,
 * which drops list bullets and punctuation runs (`-`, `.`, `..`) without
 * touching anything that could name a machine.
 */
function classifyEntry(raw: string): UrlSetEntry | UrlSetReject {
  let parsed: URL;
  try {
    parsed = raw.includes('://') ? new URL(raw) : new URL(`https://${raw}`);
  } catch {
    return { raw, reason: 'not a URL or hostname' };
  }
  if (!PROBEABLE_SCHEMES.has(parsed.protocol)) {
    return { raw, reason: `unsupported scheme "${parsed.protocol.replace(':', '')}" — use http or https` };
  }
  const host = parsed.hostname;
  if (!host) return { raw, reason: 'no hostname' };
  if (!/[a-z0-9]/i.test(host)) return { raw, reason: 'not a hostname' };
  return { raw, url: toProbeUrl(raw), host };
}

/**
 * Parse the multi-URL entry box into a set, reporting every line that will not
 * be probed. Blank lines are the one silent drop — they carry no intent.
 */
export function parseUrlSetInput(text: string): UrlSetParse {
  const entries: UrlSetEntry[] = [];
  const duplicates: UrlSetReject[] = [];
  const invalid: UrlSetReject[] = [];
  const overflow: UrlSetReject[] = [];
  const seen = new Set<string>();

  for (const raw of splitUrlSetInput(text)) {
    const classified = classifyEntry(raw);
    if ('reason' in classified) {
      invalid.push(classified);
      continue;
    }
    if (seen.has(classified.url)) {
      duplicates.push({ raw, reason: `already covered by ${classified.url}` });
      continue;
    }
    seen.add(classified.url);
    if (entries.length >= MAX_SET_URLS) {
      overflow.push({ raw, reason: `over the ${MAX_SET_URLS}-URL set limit` });
      continue;
    }
    entries.push(classified);
  }

  return { entries, duplicates, invalid, overflow };
}

/**
 * The launch input for a parsed set — the raw spellings of the entries that
 * will actually be probed, space-joined for `buildDiagRequest`. Returns '' when
 * nothing survived parsing, which the caller must treat as "nothing to probe"
 * rather than launching an empty run.
 */
export function launchInputFor(parse: UrlSetParse): string {
  return parse.entries.map(e => e.raw).join(' ');
}

/** Textarea content for a list of hosts — one per line, the documented shape. */
export function formatUrlSetInput(hosts: readonly string[]): string {
  return dedupeProbeEntries([...hosts]).join('\n');
}

/** Minimal watchlist-row shape the selection helpers need. */
export interface SelectableRow {
  /** Row identity (host under host grouping, host × runner axes otherwise). */
  key: string;
  host: string;
}

/**
 * Hosts behind a checkbox selection, de-duplicated in row order.
 *
 * Under the provider / capacity grouping (lib/probe-grouping.ts) ONE host owns
 * several rows — example.com × azure and example.com × gcp. Ticking both must
 * probe example.com once, not twice: the set is a list of URLs, and the runner
 * axis is a property of the run, not of the target. Rows whose key is no longer
 * present are ignored, so a stale selection can never smuggle a host back in.
 */
export function hostsForSelection(
  rows: readonly SelectableRow[],
  selectedKeys: ReadonlySet<string>,
): string[] {
  const hosts: string[] = [];
  const seen = new Set<string>();
  for (const row of rows) {
    if (!selectedKeys.has(row.key) || seen.has(row.host)) continue;
    seen.add(row.host);
    hosts.push(row.host);
  }
  return hosts;
}

/** Toggle one row key, returning a NEW set (state stays immutable). */
export function setSelectionKey(
  selectedKeys: ReadonlySet<string>,
  key: string,
  selected: boolean,
): Set<string> {
  const next = new Set(selectedKeys);
  if (selected) next.add(key);
  else next.delete(key);
  return next;
}
