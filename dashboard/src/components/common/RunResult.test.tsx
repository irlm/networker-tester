import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { RunResult } from './RunResult';

describe('RunResult (audit F4/F5 — one format, zero never red)', () => {
  it('renders the canonical ok/total · fail format', () => {
    const { container } = render(<RunResult ok={7} fail={2} />);
    expect(container.textContent).toBe('7/9 · 2 fail');
  });

  it('paints the fail segment red only when failures exist', () => {
    render(<RunResult ok={7} fail={2} />);
    expect(screen.getByText('2 fail')).toHaveClass('text-red-400');
  });

  it('never renders "0 fail" in red (the 0/0 regression)', () => {
    render(<RunResult ok={0} fail={0} />);
    const fail = screen.getByText('0 fail');
    expect(fail).not.toHaveClass('text-red-400');
    expect(fail).toHaveClass('text-gray-600');
  });
});
