import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { usePhaseSubscription } from './usePhaseSubscription';
import { useTesterSubscription } from './useTesterSubscription';

/**
 * Minimal WebSocket mock. Each instance is captured in `sockets` so tests
 * can drive messages and close events.
 */
class MockWebSocket {
  static OPEN = 1;
  static CLOSED = 3;

  public readyState = 0;
  public url: string;
  public sent: string[] = [];
  public onclose: ((ev: unknown) => void) | null = null;
  private listeners: Record<string, Array<(ev: unknown) => void>> = {};

  constructor(url: string) {
    this.url = url;
    sockets.push(this);
  }

  addEventListener(type: string, cb: (ev: unknown) => void) {
    (this.listeners[type] ??= []).push(cb);
  }

  send(data: string) {
    this.sent.push(data);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
    this.dispatch('close', { code: 1000 });
    if (this.onclose) this.onclose({ code: 1000 });
  }

  // Test helpers ----------------------------------------------------------
  open() {
    this.readyState = MockWebSocket.OPEN;
    this.dispatch('open', {});
  }

  emit(msg: unknown) {
    this.dispatch('message', { data: JSON.stringify(msg) });
  }

  private dispatch(type: string, ev: unknown) {
    (this.listeners[type] ?? []).forEach((cb) => cb(ev));
  }
}

const sockets: MockWebSocket[] = [];

beforeEach(() => {
  sockets.length = 0;
  vi.stubGlobal('WebSocket', MockWebSocket as unknown as typeof WebSocket);
  localStorage.setItem('token', 'test-token');
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('useTesterSubscription', () => {
  it('sends subscribe on open and stores snapshot/update messages', async () => {
    const { result } = renderHook(() =>
      useTesterSubscription('proj-1', ['t-a', 't-b']),
    );

    // Connection opened
    await waitFor(() => expect(sockets.length).toBe(1));
    const ws = sockets[0];
    expect(ws.url).toContain('/ws/testers');
    expect(ws.url).toContain('token=test-token');

    act(() => ws.open());

    expect(ws.sent).toHaveLength(1);
    expect(JSON.parse(ws.sent[0])).toEqual({
      type: 'subscribe_tester_queue',
      project_id: 'proj-1',
      tester_ids: ['t-a', 't-b'],
    });

    act(() => {
      ws.emit({
        type: 'tester_queue_snapshot',
        project_id: 'proj-1',
        tester_id: 't-a',
        seq: 5,
        running: { config_id: 'c1', name: 'bench' },
        queued: [],
      });
    });

    expect(result.current['t-a']).toEqual({
      running: { config_id: 'c1', name: 'bench' },
      queued: [],
      seq: 5,
    });

    // Stale update dropped
    act(() => {
      ws.emit({
        type: 'tester_queue_update',
        project_id: 'proj-1',
        tester_id: 't-a',
        seq: 4,
        trigger: 'x',
        running: null,
        queued: [],
      });
    });
    expect(result.current['t-a'].seq).toBe(5);

    // Newer update applied
    act(() => {
      ws.emit({
        type: 'tester_queue_update',
        project_id: 'proj-1',
        tester_id: 't-a',
        seq: 6,
        trigger: 'benchmark_completed',
        running: null,
        queued: [{ config_id: 'c2', name: 'bench2', position: 1 }],
      });
    });
    expect(result.current['t-a']).toEqual({
      running: null,
      queued: [{ config_id: 'c2', name: 'bench2', position: 1 }],
      seq: 6,
    });
  });

  it('is a no-op when projectId or testerIds is empty', () => {
    renderHook(() => useTesterSubscription('', []));
    expect(sockets.length).toBe(0);
  });
});

describe('usePhaseSubscription', () => {
  it('filters phase_update by entity_type + entity_id', async () => {
    const { result } = renderHook(() =>
      usePhaseSubscription('proj-1', 't-a', 'benchmark', 'cfg-1'),
    );

    await waitFor(() => expect(sockets.length).toBe(1));
    const ws = sockets[0];
    act(() => ws.open());

    expect(JSON.parse(ws.sent[0])).toEqual({
      type: 'subscribe_tester_queue',
      project_id: 'proj-1',
      tester_ids: ['t-a'],
    });

    // Different entity_id → ignored
    act(() => {
      ws.emit({
        type: 'phase_update',
        project_id: 'proj-1',
        entity_type: 'benchmark',
        entity_id: 'cfg-other',
        seq: 1,
        phase: 'running',
        applied_stages: ['queued', 'starting', 'deploy', 'running'],
      });
    });
    expect(result.current).toBeNull();

    // Matching entity → applied
    act(() => {
      ws.emit({
        type: 'phase_update',
        project_id: 'proj-1',
        entity_type: 'benchmark',
        entity_id: 'cfg-1',
        seq: 2,
        phase: 'running',
        applied_stages: ['queued', 'starting', 'deploy', 'running'],
      });
    });
    expect(result.current).toEqual({
      phase: 'running',
      outcome: null,
      message: null,
      appliedStages: ['queued', 'starting', 'deploy', 'running'],
      seq: 2,
    });

    // Older seq dropped
    act(() => {
      ws.emit({
        type: 'phase_update',
        project_id: 'proj-1',
        entity_type: 'benchmark',
        entity_id: 'cfg-1',
        seq: 1,
        phase: 'queued',
        applied_stages: ['queued'],
      });
    });
    expect(result.current?.seq).toBe(2);

    // Terminal phase with outcome
    act(() => {
      ws.emit({
        type: 'phase_update',
        project_id: 'proj-1',
        entity_type: 'benchmark',
        entity_id: 'cfg-1',
        seq: 3,
        phase: 'done',
        outcome: 'success',
        message: 'all good',
        applied_stages: ['queued', 'starting', 'deploy', 'running', 'collect', 'done'],
      });
    });
    expect(result.current).toMatchObject({
      phase: 'done',
      outcome: 'success',
      message: 'all good',
      seq: 3,
    });
  });
});
