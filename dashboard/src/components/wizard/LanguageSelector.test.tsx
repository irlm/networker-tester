import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { LanguageSelector } from './LanguageSelector';
import type { LanguageCapability } from '../../api/types';
import { makeTestbed } from './testbed-constants';

// Trimmed capability matrix mirroring the control plane's
// BenchmarkLanguageCapabilities table (GET /api/modes → language_capabilities).
const CAPS: LanguageCapability[] = [
  { language: 'rust', http1: true, http2: true, http3: true, apibench: true },
  { language: 'go', http1: true, http2: true, http3: true, apibench: true },
  { language: 'java', http1: true, http2: false, http3: false, apibench: true },
  { language: 'python', http1: true, http2: false, http3: false, apibench: true },
  { language: 'nginx', http1: true, http2: true, http3: true, apibench: false },
];

const noop = () => {};
const baseProps = {
  onLangsChange: noop,
  testbeds: [makeTestbed(0)],
  capabilities: CAPS,
};

describe('LanguageSelector capability gating', () => {
  it('disables and tags nginx when apibench mode is selected', () => {
    render(
      <LanguageSelector
        {...baseProps}
        selectedLangs={new Set(['rust'])}
        selectedModes={new Set(['http1', 'apibench'])}
      />,
    );

    // The tag on the nginx entry plus the mono span inside the exclusion note.
    expect(screen.getAllByText('no /api/*')).toHaveLength(2);
    // nginx checkbox stays unselectable (it is baseline-disabled anyway) and
    // the apibench exclusion note is shown.
    expect(screen.getByText(/serve no measured API suite/)).toBeInTheDocument();
    expect(screen.queryByText('baseline')).not.toBeInTheDocument();
  });

  it('tags direct-h1-only languages when h2/h3 modes are selected', () => {
    render(
      <LanguageSelector
        {...baseProps}
        selectedLangs={new Set(['rust', 'java'])}
        selectedModes={new Set(['http1', 'http2', 'http3'])}
      />,
    );

    // java + python tags (h1-only in the trimmed matrix) plus the mono span
    // inside the footnote.
    expect(screen.getAllByText('h1 direct')).toHaveLength(3);
    expect(screen.getByText(/h2\/h3 modes measure the proxy/)).toBeInTheDocument();
  });

  it('shows no capability tags without apibench or h2/h3 modes', () => {
    render(
      <LanguageSelector
        {...baseProps}
        selectedLangs={new Set(['rust'])}
        selectedModes={new Set(['http1', 'download'])}
      />,
    );

    expect(screen.queryByText('no /api/*')).not.toBeInTheDocument();
    expect(screen.queryByText('h1 direct')).not.toBeInTheDocument();
    expect(screen.getByText('baseline')).toBeInTheDocument();
  });

  it('shows no tags when the capability matrix is unavailable (degrade open)', () => {
    render(
      <LanguageSelector
        onLangsChange={noop}
        testbeds={[makeTestbed(0)]}
        selectedLangs={new Set(['rust'])}
        selectedModes={new Set(['apibench', 'http2'])}
      />,
    );

    expect(screen.queryByText('no /api/*')).not.toBeInTheDocument();
    expect(screen.queryByText('h1 direct')).not.toBeInTheDocument();
  });

  it('shortcut buttons exclude apibench-incompatible languages', () => {
    const onLangsChange = vi.fn();
    render(
      <LanguageSelector
        {...baseProps}
        onLangsChange={onLangsChange}
        selectedLangs={new Set()}
        selectedModes={new Set(['apibench'])}
      />,
    );

    fireEvent.click(screen.getByText('Top 5'));
    const langs = onLangsChange.mock.calls[0][0] as Set<string>;
    expect(langs.has('nginx')).toBe(false);
    expect(langs.has('rust')).toBe(true);
  });
});
