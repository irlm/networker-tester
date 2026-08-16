import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { WorkloadPanel } from './WorkloadPanel';
import { unsupportedReason } from '../../lib/mode-capabilities';
import type { ModeGroup } from '../../api/types';

// A trimmed GET /api/modes catalog: the HTTP group + the browser page-load
// group — enough to exercise the HTTP/3-by-stack and runner-Chrome gates the
// Full Stack / Network Test wizards feed into the picker.
const GROUPS: ModeGroup[] = [
  {
    label: 'HTTP',
    detail: '',
    modes: [
      { id: 'http1', name: 'HTTP/1.1', desc: 'Single request', detail: '' },
      { id: 'http2', name: 'HTTP/2', desc: 'Multiplexed', detail: '' },
      { id: 'http3', name: 'HTTP/3', desc: 'QUIC', detail: '' },
    ],
  },
  {
    label: 'Page Load (Browser)',
    detail: '',
    modes: [
      { id: 'browser1', name: 'H1', desc: 'Chrome HTTP/1.1', detail: '' },
      { id: 'browser3', name: 'H3', desc: 'Chrome QUIC', detail: '' },
    ],
  },
];

const noop = () => {};
const baseProps = {
  modeGroups: GROUPS,
  runs: 10, onRunsChange: noop,
  concurrency: 1, onConcurrencyChange: noop,
  timeoutMs: 5000, onTimeoutChange: noop,
  selectedPayloads: new Set<string>(), onPayloadsChange: noop,
  showAdvanced: false,
};

const rowFor = (name: string) => screen.getByText(name).closest('label')!;
const checkboxFor = (name: string) => rowFor(name).querySelector('input') as HTMLInputElement;

describe('WorkloadPanel — target capability gating', () => {
  it('greys out the h3 modes with the stack reason when the target is apache, and pre-unchecks them', () => {
    const onModesChange = vi.fn();
    render(
      <WorkloadPanel
        {...baseProps}
        selectedModes={new Set(['http1', 'http3'])}
        onModesChange={onModesChange}
        unsupported={id => unsupportedReason(id, { kind: 'endpoint', stack: 'apache' })}
      />,
    );

    const h3 = checkboxFor('HTTP/3');
    expect(h3.disabled).toBe(true);
    // Pre-unchecked even though the (stale) selection still names it.
    expect(h3.checked).toBe(false);
    expect(rowFor('HTTP/3').getAttribute('title')).toContain('apache has no HTTP/3');
    expect(rowFor('HTTP/3').getAttribute('title')).toContain('shared/http-stacks.json');
    expect(rowFor('HTTP/3').textContent).toContain('unsupported');
    // browser3 is Chrome QUIC — same stack rule.
    expect(checkboxFor('H3').disabled).toBe(true);

    // The H1/H2 siblings stay live.
    expect(checkboxFor('HTTP/1.1').disabled).toBe(false);
    expect(checkboxFor('HTTP/1.1').checked).toBe(true);
    expect(checkboxFor('HTTP/2').disabled).toBe(false);
    expect(checkboxFor('H1').disabled).toBe(false);

    // Clicking a disabled row must not toggle it into the selection.
    fireEvent.click(h3);
    expect(onModesChange).not.toHaveBeenCalled();
  });

  it('keeps the h3 modes live for nginx / caddy and for a mixed matrix', () => {
    for (const stack of ['nginx', 'caddy', ['apache', 'nginx']] as const) {
      const { unmount } = render(
        <WorkloadPanel
          {...baseProps}
          selectedModes={new Set(['http3'])}
          onModesChange={noop}
          unsupported={id => unsupportedReason(id, { kind: 'endpoint', stack })}
        />,
      );
      expect(checkboxFor('HTTP/3').disabled, String(stack)).toBe(false);
      expect(checkboxFor('HTTP/3').checked, String(stack)).toBe(true);
      unmount();
    }
  });

  it('greys out only the browser modes when the pinned runner has no Chrome', () => {
    render(
      <WorkloadPanel
        {...baseProps}
        selectedModes={new Set(['http3', 'browser1'])}
        onModesChange={noop}
        unsupported={id => unsupportedReason(id, { kind: 'endpoint', stack: 'nginx', runner: { chrome: false, tshark: true } })}
      />,
    );
    expect(checkboxFor('H1').disabled).toBe(true);
    expect(rowFor('H1').getAttribute('title')).toContain('Chrome');
    expect(checkboxFor('H3').disabled).toBe(true);
    expect(checkboxFor('HTTP/3').disabled).toBe(false);
    expect(checkboxFor('HTTP/3').checked).toBe(true);
  });

  it('the group toggle only ever selects the supported modes', () => {
    const onModesChange = vi.fn();
    render(
      <WorkloadPanel
        {...baseProps}
        selectedModes={new Set()}
        onModesChange={onModesChange}
        unsupported={id => unsupportedReason(id, { kind: 'endpoint', stack: 'haproxy' })}
      />,
    );
    // The HTTP group header checkbox — the first checkbox in the HTTP card.
    const httpGroup = screen.getByText('HTTP').closest('label')!.querySelector('input') as HTMLInputElement;
    fireEvent.click(httpGroup);
    expect(onModesChange).toHaveBeenCalledTimes(1);
    const next = onModesChange.mock.calls[0][0] as Set<string>;
    expect([...next].sort()).toEqual(['http1', 'http2']);
  });
});
