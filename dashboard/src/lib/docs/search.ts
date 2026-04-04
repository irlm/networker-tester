import type { DocEntry, DocCategory } from './content';

export function searchDocs(entries: DocEntry[], query: string): DocEntry[] {
  let q = query.trim().toLowerCase();
  if (!q) return entries;

  // Strip terminal-style prefixes
  for (const prefix of ['man ', 'help ', 'explain ', 'what is ', 'show ']) {
    if (q.startsWith(prefix)) {
      q = q.slice(prefix.length).trim();
      break;
    }
  }

  if (!q) return entries;

  // Easter egg: "man man" returns the meta entry
  if (q === 'man') {
    return [META_MAN_ENTRY, ...entries];
  }

  const terms = q.split(/\s+/);

  const scored: { entry: DocEntry; score: number }[] = [];

  for (const entry of entries) {
    let totalScore = 0;
    let allTermsMatch = true;

    for (const term of terms) {
      let termScore = 0;

      // Title match (highest weight)
      termScore = Math.max(termScore, fieldScore(entry.title, term, 10));

      // Aliases match
      for (const alias of entry.aliases) {
        termScore = Math.max(termScore, fieldScore(alias, term, 8));
      }

      // Brief match
      termScore = Math.max(termScore, fieldScore(entry.brief, term, 4));

      // Detail match (lowest weight)
      termScore = Math.max(termScore, fieldScore(entry.detail, term, 1));

      if (termScore === 0) {
        allTermsMatch = false;
        break;
      }
      totalScore += termScore;
    }

    if (allTermsMatch) {
      scored.push({ entry, score: totalScore });
    }
  }

  scored.sort((a, b) => {
    if (b.score !== a.score) return b.score - a.score;
    return a.entry.title.localeCompare(b.entry.title);
  });

  return scored.map((s) => s.entry);
}

function fieldScore(field: string, term: string, weight: number): number {
  const lower = field.toLowerCase();
  const idx = lower.indexOf(term);
  if (idx === -1) return 0;
  // Word-start match scores higher
  if (idx === 0 || /\s/.test(lower[idx - 1]) || /[/(\-_]/.test(lower[idx - 1])) {
    return weight;
  }
  return weight * 0.6;
}

/** Self-referential man page, like typing `man man` in a real terminal. */
const META_MAN_ENTRY: DocEntry = {
  id: 'meta-man',
  category: 'data-flow',
  title: 'man(1) — Documentation Viewer',
  aliases: ['man', 'manual', 'help', 'rtfm'],
  brief: 'You are reading the manual for the manual.',
  detail: `NETWORKER-DOCS(1)       Networker Manual       NETWORKER-DOCS(1)

NAME
    docs — interactive documentation for networker-tester

SYNOPSIS
    ?           Open full documentation browser
    /           Open quick search palette
    man <topic> Search by topic (e.g. "man p95")

NAVIGATION
    j/k         Move selection down/up
    gg/G        Jump to first/last entry
    l/Enter     Expand entry
    h           Collapse entry
    0-5         Filter by category
    /  or  i    Focus search input
    Tab         Toggle INSERT/NORMAL mode
    q           Close panel or go back

SEARCH PREFIXES
    man <topic>     Search (e.g. "man throughput")
    help <topic>    Search (e.g. "help p95")
    explain <topic> Search (e.g. "explain jitter")

CATEGORIES
    1 Protocols     Network test modes and what they measure
    2 Metrics       Individual measurement fields
    3 Statistics    How numbers are computed
    4 Benchmarks    Benchmark methodology and quality
    5 Data Flow     Collection, processing, and display

SEE ALSO
    Press ? for full docs, / for quick search.`,
};

export function filterByCategory(entries: DocEntry[], category: DocCategory | null): DocEntry[] {
  if (!category) return entries;
  return entries.filter((e) => e.category === category);
}
