import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ErrorBoundary } from './ErrorBoundary';

function Bomb({ explode }: { explode: boolean }) {
  if (explode) throw new Error('Cannot read properties of undefined (reading \'filter\')');
  return <div>alive</div>;
}

beforeEach(() => {
  // React logs boundary-caught errors; keep test output clean.
  vi.spyOn(console, 'error').mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ErrorBoundary', () => {
  it('renders children when nothing throws', () => {
    render(
      <ErrorBoundary>
        <Bomb explode={false} />
      </ErrorBoundary>,
    );
    expect(screen.getByText('alive')).toBeInTheDocument();
  });

  it('catches render throws and shows the terminal-styled fallback with a Reload action', () => {
    render(
      <ErrorBoundary>
        <Bomb explode />
      </ErrorBoundary>,
    );
    // No black screen: fallback panel is visible with the error message and actions.
    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('something broke')).toBeInTheDocument();
    expect(screen.getByTestId('error-boundary-message').textContent).toContain('filter');
    expect(screen.getByRole('button', { name: 'Reload' })).toBeInTheDocument();
  });

  it('resets when resetKey changes (navigation recovers a crashed route)', () => {
    const { rerender } = render(
      <ErrorBoundary resetKey="/runs/broken">
        <Bomb explode />
      </ErrorBoundary>,
    );
    expect(screen.getByRole('alert')).toBeInTheDocument();

    rerender(
      <ErrorBoundary resetKey="/runs">
        <Bomb explode={false} />
      </ErrorBoundary>,
    );
    expect(screen.getByText('alive')).toBeInTheDocument();
  });
});
