// ProbeComparePage: the honesty rules the report exists to enforce — a thin
// overlap is GREYED and explained instead of ranked, an under-sampled URL is
// named rather than scored, modes are never pooled, and crowns only appear on
// a ranking the server stood behind.

import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import { ProbeComparePage } from './ProbeComparePage';
import { resetRoleStores, setProjectRole } from '../test/rbac-helpers';
import type {
  ProbeComparisonMode,
  ProbeComparisonReport,
  ProbeComparisonScore,
} from '../api/types';

const A = 'https://a.example/';
const B = 'https://b.example/';
const C = 'https://c.example/';

function score(over: Partial<ProbeComparisonScore> & { url: string }): ProbeComparisonScore {
  return {
    shared_buckets: 60,
    samples: 300,
    median_p50_ms: 100,
    median_p95_ms: 150,
    success_rate: 0.99,
    jitter_ratio: 1.5,
    median_dns_ms: 10,
    median_tcp_ms: 20,
    median_tls_ms: 30,
    median_ttfb_ms: 80,
    dominant_error_category: null,
    ...over,
  };
}

function mode(over: Partial<ProbeComparisonMode> = {}): ProbeComparisonMode {
  return {
    mode: 'http2',
    ranked: true,
    ranking_verdict: 'ranked',
    shared_buckets: 60,
    coverage_ratio: 0.3571,
    coverage: [
      { url: A, qualifying_buckets: 90, total_samples: 450, eligible: true, excluded_reason: null },
      { url: B, qualifying_buckets: 80, total_samples: 400, eligible: true, excluded_reason: null },
    ],
    scores: [score({ url: A, median_p50_ms: 100 }), score({ url: B, median_p50_ms: 200 })],
    head_to_head: [{ a: A, b: B, buckets: 60, a_wins: 41, b_wins: 18, ties: 1 }],
    crowns: {
      fastest: A,
      most_reliable: A,
      most_consistent: B,
      best_dns: A,
      best_tcp: null,
      best_tls: null,
      best_ttfb: B,
    },
    series: [
      {
        url: A, bucket: '2026-08-20T00:00:00Z', sample_count: 5, success_count: 5, shared: true,
        p50_total_ms: 100, p95_total_ms: 150, p50_dns_ms: 10, p50_tcp_ms: 20,
        p50_tls_ms: 30, p50_ttfb_ms: 80, dominant_error_category: null,
      },
    ],
    ...over,
  };
}

function report(over: Partial<ProbeComparisonReport> = {}): ProbeComparisonReport {
  return {
    generated_at: '2026-08-20T12:00:00Z',
    from: '2026-08-13T12:00:00Z',
    to: '2026-08-20T12:00:00Z',
    window: '7d',
    bucket: '1h',
    bucket_seconds: 3600,
    window_buckets: 168,
    min_samples: 3,
    min_coverage_ratio: 0.3,
    methodology: {
      buckets: 'bucket rule', shared: 'shared rule', eligibility: 'eligibility rule',
      ranking: 'ranking rule', crowns: 'crown rule', modes: 'mode rule',
    },
    available: [
      { url: A, sample_count: 450, mode_count: 1, last_seen: '2026-08-20T11:00:00Z' },
      { url: B, sample_count: 400, mode_count: 1, last_seen: '2026-08-20T11:00:00Z' },
      { url: C, sample_count: 20, mode_count: 1, last_seen: '2026-08-19T11:00:00Z' },
    ],
    modes: [mode()],
    ...over,
  };
}

const getProbeComparison = vi.fn(() => Promise.resolve(report()));

vi.mock('../api/client', () => ({
  errorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
  api: {
    getProbeComparison: (...a: unknown[]) => getProbeComparison(...(a as [])),
  },
}));

const addToast = vi.fn();
vi.mock('../hooks/useToast', () => ({ useToast: () => addToast }));

function renderPage(query = `?urls=${encodeURIComponent(`${A},${B}`)}`) {
  return render(
    <MemoryRouter initialEntries={[`/projects/p-1/probe/compare${query}`]}>
      <ProbeComparePage />
    </MemoryRouter>,
  );
}

describe('ProbeComparePage', () => {
  // useProject() reads the project from the route params, and these tests
  // mount the page without a <Routes> — the store is the app's own fallback.
  beforeEach(() => setProjectRole('operator'));
  afterEach(() => {
    resetRoleStores();
    vi.clearAllMocks();
  });

  it('asks the server for exactly the selected URLs, window and bucket', async () => {
    renderPage(`?urls=${encodeURIComponent(`${A},${B}`)}&window=24h&bucket=15m`);

    await waitFor(() =>
      expect(getProbeComparison).toHaveBeenCalledWith('p-1', {
        urls: [A, B],
        window: '24h',
        bucket: '15m',
        // The axis defaults to comparing URLs, and hidden entries stay hidden
        // until the picker asks for them.
        groupBy: 'url',
        includeHidden: false,
      }),
    );
  });

  it('leads with the shared coverage, not with the ranking', async () => {
    renderPage();

    // The licence to believe the scoreboard is stated before the scoreboard.
    expect(await screen.findByText('60 / 168 buckets')).toBeInTheDocument();
    expect(screen.getByText('35.71%'.replace('35.71%', '36%'))).toBeInTheDocument();
  });

  it('shows the crowns and the head-to-head claim when the mode is ranked', async () => {
    renderPage();

    // The summary cards name each category's winner...
    const winners = await screen.findByRole('list', { name: 'Category winners' });
    const cards = within(winners).getAllByRole('listitem');
    expect(cards.map((c) => c.textContent)).toEqual([
      'Fastesta.example',
      'Most reliablea.example',
      'Most consistentb.example',
      'Best TTFBb.example',
    ]);

    // ...and the same crowns ride on the scoreboard row that earned them, so a
    // reader scanning the table does not have to look back up.
    const table = screen.getByRole('table');
    const rowA = within(table).getAllByRole('row')[1];
    expect(within(rowA).getByText('Fastest')).toBeInTheDocument();
    expect(within(rowA).getByText('Most reliable')).toBeInTheDocument();
    expect(within(rowA).queryByText('Most consistent')).not.toBeInTheDocument();

    expect(screen.getByText(/faster in 41 of 60/)).toBeInTheDocument();
  });

  it('greys the scoreboard and explains itself when overlap is too thin', async () => {
    getProbeComparison.mockResolvedValueOnce(
      report({
        modes: [mode({
          ranked: false,
          ranking_verdict: 'insufficient_overlap',
          shared_buckets: 42,
          coverage_ratio: 0.25,
        })],
      }),
    );
    renderPage();

    expect(await screen.findByText(/Insufficient overlap/)).toBeInTheDocument();
    expect(screen.getByText(/42 of 168 buckets/)).toBeInTheDocument();
    // No crown may be claimed on a ranking the server withheld.
    expect(screen.queryByText('Fastest')).not.toBeInTheDocument();
    expect(screen.queryByText('Most reliable')).not.toBeInTheDocument();
  });

  it('names an under-sampled URL instead of scoring it', async () => {
    getProbeComparison.mockResolvedValueOnce(
      report({
        modes: [mode({
          coverage: [
            { url: A, qualifying_buckets: 90, total_samples: 450, eligible: true, excluded_reason: null },
            { url: B, qualifying_buckets: 80, total_samples: 400, eligible: true, excluded_reason: null },
            { url: C, qualifying_buckets: 4, total_samples: 20, eligible: false, excluded_reason: 'under_sampled' },
          ],
        })],
      }),
    );
    renderPage();

    expect(await screen.findByText(/Not compared/)).toBeInTheDocument();
    expect(screen.getByText(/c\.example \(4\/168\)/)).toBeInTheDocument();
  });

  it('keeps modes apart instead of pooling them', async () => {
    getProbeComparison.mockResolvedValueOnce(
      report({
        modes: [
          mode({ mode: 'http2', shared_buckets: 60 }),
          mode({ mode: 'http3', shared_buckets: 20 }),
        ],
      }),
    );
    renderPage();

    const tabs = await screen.findAllByRole('tab');
    expect(tabs.map((t) => t.textContent)).toEqual([
      expect.stringContaining('http2'),
      expect.stringContaining('http3'),
    ]);
    // The mode with the most shared coverage opens first — its ranking is
    // worth the most.
    expect(tabs[0]).toHaveAttribute('aria-selected', 'true');

    await userEvent.click(tabs[1]);
    expect(tabs[1]).toHaveAttribute('aria-selected', 'true');
  });

  it('refuses to compare a single URL and says how to fix it', async () => {
    getProbeComparison.mockResolvedValueOnce(report({ modes: [] }));
    renderPage(`?urls=${encodeURIComponent(A)}`);

    expect(await screen.findByText('Pick one more URL to compare.')).toBeInTheDocument();
  });

  it('offers every probed URL in the picker, with what it has', async () => {
    renderPage();

    await screen.findByText('60 / 168 buckets');
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes).toHaveLength(3);
    // The two in the query string are ticked; the third is not.
    expect(boxes[0]).toBeChecked();
    expect(boxes[1]).toBeChecked();
    expect(boxes[2]).not.toBeChecked();
    expect(screen.getByText(/20 samples/)).toBeInTheDocument();
  });

  it('scores every eligible URL with its phase medians', async () => {
    renderPage();

    const table = await screen.findByRole('table');
    const rows = within(table).getAllByRole('row');
    // header + 2 scored URLs
    expect(rows).toHaveLength(3);
    expect(within(rows[1]).getByText('a.example')).toBeInTheDocument();
    expect(within(rows[2]).getByText('b.example')).toBeInTheDocument();
  });

  it('surfaces the dominant failure category on the row it belongs to', async () => {
    getProbeComparison.mockResolvedValueOnce(
      report({
        modes: [mode({
          scores: [
            score({ url: A }),
            score({ url: B, success_rate: 0.72, dominant_error_category: 'timeout' }),
          ],
        })],
      }),
    );
    renderPage();

    expect(await screen.findByText('mostly timeout')).toBeInTheDocument();
  });

  // ── Comparison axis ───────────────────────────────────────────────────
  // Same measurements, different question: which SITE is faster, or which
  // VANTAGE POINT. Until v0.28.298 the report pooled every runner into one
  // series, so the second question could not be asked at all.

  it('asks for the runner axis when group_by=runner', async () => {
    renderPage(`?urls=${encodeURIComponent(A)}&group_by=runner`);

    await waitFor(() =>
      expect(getProbeComparison).toHaveBeenCalledWith(
        'p-1',
        expect.objectContaining({ groupBy: 'runner', urls: [A] }),
      ),
    );
  });

  it('does NOT call the server for a runner comparison of two URLs', async () => {
    // The server 400s on that by design (pooling two sites per runner is the
    // same blending inverted). Asking anyway would render an error where the
    // picker should be telling the user what to do.
    renderPage(`?urls=${encodeURIComponent(`${A},${B}`)}&group_by=runner`);

    await waitFor(() =>
      expect(getProbeComparison).toHaveBeenCalledWith(
        'p-1',
        expect.objectContaining({ groupBy: 'runner', urls: [] }),
      ),
    );
    // Match the WARNING's unique phrase: the section header also says
    // "exactly one URL", so a looser matcher finds two elements.
    expect(await screen.findByText(/deselect the others/i)).toBeInTheDocument();
  });

  it('offers to hide a URL from the picker', async () => {
    renderPage();
    // Hiding is presentation-only and reversible — the control says "hide",
    // never "delete", because the probe history is untouched.
    expect(await screen.findByLabelText(/^Hide a\.example$/)).toBeInTheDocument();
  });
});
