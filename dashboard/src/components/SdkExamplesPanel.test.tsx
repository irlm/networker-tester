import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { SdkExamplesPanel } from './SdkExamplesPanel';
import type { SdkExample } from '../lib/sdkExamples';

const liveExample: SdkExample = {
  id: 'rust',
  language: 'Rust',
  runtime: 'axum · tower',
  description: 'Minimal Rust reference service.',
  sourceUrl: 'https://github.com/example/rust',
  liveUrl: 'https://rust.example.test',
};

describe('SdkExamplesPanel', () => {
  it('shows a running example and gives operators a registration draft', async () => {
    const user = userEvent.setup();
    const onUseExample = vi.fn();
    render(<SdkExamplesPanel isOperator onUseExample={onUseExample} examples={[liveExample]} />);

    expect(screen.getByText('Live on Azure')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open live app' })).toHaveAttribute(
      'href',
      'https://rust.example.test',
    );

    await user.click(screen.getByRole('button', { name: 'Use example' }));
    expect(onUseExample).toHaveBeenCalledWith({
      name: 'LagHound Rust reference',
      description: 'Public axum · tower SDK reference on Azure Container Apps',
      url: 'https://rust.example.test',
      route: '/laghound/echo',
      token: 'demo-token-laghound',
    });
  });

  it('keeps registration actions hidden from viewers', () => {
    render(<SdkExamplesPanel isOperator={false} onUseExample={vi.fn()} examples={[liveExample]} />);
    expect(screen.queryByRole('button', { name: 'Use example' })).not.toBeInTheDocument();
  });
});
