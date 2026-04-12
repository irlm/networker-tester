import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { PhaseBar } from './PhaseBar';

type Phase = 'queued' | 'starting' | 'deploy' | 'running' | 'collect' | 'done';

const allStages: Phase[] = [
  'queued',
  'starting',
  'deploy',
  'running',
  'collect',
  'done',
];

describe('PhaseBar', () => {
  it('renders a segment for each applied stage', () => {
    render(
      <PhaseBar phase="running" outcome={null} appliedStages={allStages} />,
    );
    expect(screen.getAllByRole('progressbar')).toHaveLength(1);
    allStages.forEach((s) => {
      expect(screen.getByTestId(`phase-segment-${s}`)).toBeInTheDocument();
    });
  });

  it('marks active stage with pulse + purple fill', () => {
    render(
      <PhaseBar phase="running" outcome={null} appliedStages={allStages} />,
    );
    const seg = screen.getByTestId('phase-segment-running');
    expect(seg.className).toMatch(/animate-pulse/);
    expect(seg.className).toMatch(/bg-purple-500/);
  });

  it('earlier stages render in cyan when a later stage is active', () => {
    render(
      <PhaseBar phase="running" outcome={null} appliedStages={allStages} />,
    );
    const earlier = screen.getByTestId('phase-segment-starting');
    expect(earlier.className).toMatch(/bg-cyan-600/);
  });

  it('later stages render gray when current phase is not done', () => {
    render(
      <PhaseBar phase="starting" outcome={null} appliedStages={allStages} />,
    );
    const later = screen.getByTestId('phase-segment-running');
    expect(later.className).toMatch(/bg-gray-800/);
  });

  it('colors final stage by outcome on done (success -> emerald)', () => {
    render(
      <PhaseBar phase="done" outcome="success" appliedStages={allStages} />,
    );
    const seg = screen.getByTestId('phase-segment-done');
    expect(seg.className).toMatch(/bg-emerald-600/);
  });

  it('colors final stage by outcome on done (failure -> rose)', () => {
    render(
      <PhaseBar phase="done" outcome="failure" appliedStages={allStages} />,
    );
    const seg = screen.getByTestId('phase-segment-done');
    expect(seg.className).toMatch(/bg-rose-600/);
  });

  it('honors reduced appliedStages (skipping deploy)', () => {
    const stages: Phase[] = ['queued', 'starting', 'running', 'collect', 'done'];
    render(<PhaseBar phase="running" outcome={null} appliedStages={stages} />);
    expect(screen.queryByTestId('phase-segment-deploy')).toBeNull();
    expect(screen.getByTestId('phase-segment-running').className).toMatch(
      /bg-purple-500/,
    );
  });
});
