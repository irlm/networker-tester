import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Methodology, Workload } from '../../api/types';
import { launchOutcomeToast, useComparisonSubmit } from './useComparisonSubmit';

const mocks = vi.hoisted(() => ({
  navigate: vi.fn(),
  addToast: vi.fn(),
  createComparisonGroup: vi.fn(),
  launchComparisonGroup: vi.fn(),
}));

vi.mock('react-router', () => ({ useNavigate: () => mocks.navigate }));
vi.mock('../../hooks/useToast', async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useToast: () => mocks.addToast,
}));
vi.mock('../../features/runs/api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../features/runs/api')>();
  return {
    ...actual,
    runsApi: {
      ...actual.runsApi,
      createComparisonGroup: mocks.createComparisonGroup,
      launchComparisonGroup: mocks.launchComparisonGroup,
    },
  };
});

// #793 P1-3: the submit hook toasted `Launched ${cells.length} runs` from the
// REQUESTED count — a fully-failed matrix launch showed a success toast. The
// toast now derives from the server's actual {launched, total, failed, errors}.
describe('launchOutcomeToast', () => {
  it('error toast listing the per-cell errors when nothing launched', () => {
    const t = launchOutcomeToast({
      launched: 0,
      total: 2,
      failed: 2,
      errors: [
        "rust @ AWS: cloud account 'AWS' is in error state: Invalid access key ID",
        'go @ AWS: same',
      ],
    });
    expect(t.type).toBe('error');
    expect(t.message).toContain('0 of 2 runs launched');
    expect(t.message).toContain("cloud account 'AWS' is in error state: Invalid access key ID");
    expect(t.message).toContain('go @ AWS: same');
  });

  it('partial launch is not a success — counts and errors are surfaced', () => {
    const t = launchOutcomeToast({
      launched: 3,
      total: 4,
      failed: 1,
      errors: ['win · haproxy: HAProxy has no native Windows build'],
    });
    expect(t.type).toBe('info');
    expect(t.message).toContain('Launched 3 of 4 runs');
    expect(t.message).toContain('1 failed');
    expect(t.message).toContain('HAProxy has no native Windows build');
  });

  it('success uses the RETURNED launched count, with singular/plural', () => {
    expect(launchOutcomeToast({ launched: 4, total: 4, failed: 0 })).toEqual({
      type: 'success',
      message: 'Launched 4 runs',
    });
    expect(launchOutcomeToast({ launched: 1, total: 1, failed: 0, errors: null })).toEqual({
      type: 'success',
      message: 'Launched 1 run',
    });
  });

  it('tolerates a null errors field on a failed launch', () => {
    const t = launchOutcomeToast({ launched: 0, total: 1, failed: 1, errors: null });
    expect(t.type).toBe('error');
    expect(t.message).toBe('Launch failed: 0 of 1 runs launched');
  });
});

// #803: a matrix launch is ONE experiment — success lands on the group's
// compare page (which live-polls), not on the filtered runs list.
describe('useComparisonSubmit matrix redirect', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.createComparisonGroup.mockResolvedValue({ id: 'group-123' });
    mocks.launchComparisonGroup.mockResolvedValue({ launched: 2, total: 2, failed: 0 });
  });

  function renderSubmit() {
    return renderHook(() =>
      useComparisonSubmit({
        projectId: 'p-1',
        buildCells: () => [
          { label: 'go @ az', endpoint: { kind: 'network', host: 'a.example.com' } },
          { label: 'py @ az', endpoint: { kind: 'network', host: 'b.example.com' } },
        ],
        buildWorkload: () => ({ modes: ['apibench'], runs: 10 } as unknown as Workload),
        methodology: { warmup_runs: 1 } as unknown as Methodology,
        effectiveName: () => 'go vs py',
        addSchedule: false,
        cronExpr: '',
        selectedTesterId: null,
        isMatrixRun: true,
        emptyCellsError: 'no cells',
      }),
    );
  }

  it('navigates to the compare page after a matrix launch', async () => {
    const { result } = renderSubmit();
    await act(() => result.current.handleSubmit(true));

    expect(mocks.launchComparisonGroup).toHaveBeenCalledWith('group-123');
    expect(mocks.navigate).toHaveBeenCalledWith('/projects/p-1/benchmarks/compare/group-123');
  });

  it('still redirects to the compare page on a partial launch (the banner shows the failures)', async () => {
    mocks.launchComparisonGroup.mockResolvedValue({
      launched: 1, total: 2, failed: 1, errors: ['py @ az: quota'],
    });
    const { result } = renderSubmit();
    await act(() => result.current.handleSubmit(true));

    expect(mocks.addToast).toHaveBeenCalledWith('info', expect.stringContaining('1 failed'));
    expect(mocks.navigate).toHaveBeenCalledWith('/projects/p-1/benchmarks/compare/group-123');
  });
});
