import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { RunErrorBanner } from './RunErrorBanner';

// #791 slice (c): failed runs hid error_message entirely — it only rendered
// inside the live-progress block for queued/running runs. The banner renders
// the message whenever present: red for terminal failures, muted otherwise.
describe('RunErrorBanner', () => {
  it('renders a red error block for a failed run, ANSI-stripped', () => {
    render(<RunErrorBanner status="failed" message={'Provisioning failed: [31mInvalid access key ID[0m'} />);

    const banner = screen.getByTestId('run-error-banner');
    expect(banner).toHaveTextContent('Provisioning failed: Invalid access key ID');
    expect(banner.textContent).not.toContain('');
    expect(banner.className).toContain('text-red-300');
    expect(screen.getByText('error')).toBeInTheDocument();
  });

  it('renders the red treatment for cancelled runs too', () => {
    render(<RunErrorBanner status="cancelled" message="Provisioning cancelled" />);
    expect(screen.getByTestId('run-error-banner').className).toContain('text-red-300');
  });

  it('renders muted context for a non-terminal status', () => {
    render(<RunErrorBanner status="running" message="transient: retrying provision" />);
    const banner = screen.getByTestId('run-error-banner');
    expect(banner.className).not.toContain('text-red-300');
    expect(screen.getByText('last error')).toBeInTheDocument();
  });

  it('renders nothing without a message', () => {
    const { container } = render(<RunErrorBanner status="failed" message={null} />);
    expect(container).toBeEmptyDOMElement();
  });
});
