import { createRef } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ErrorState, LoadingState } from './AsyncState';
import { Button } from './Button';
import { buttonClassName } from './button-styles';
import { FormField, Input, Select, Textarea } from './FormControls';
import { PageShell } from './PageShell';

describe('Button', () => {
  it('defaults to a non-submitting secondary action and composes custom styles', () => {
    render(<Button className="custom-action">Inspect</Button>);
    const button = screen.getByRole('button', { name: 'Inspect' });

    expect(button).toHaveAttribute('type', 'button');
    expect(button).toHaveClass('btn', 'btn-secondary', 'btn-sm', 'custom-action');
  });

  it('exposes a stable busy state and prevents duplicate activation', async () => {
    const onClick = vi.fn();
    render(<Button loading onClick={onClick}>Deploy</Button>);
    const button = screen.getByRole('button', { name: 'Working…' });

    expect(button).toBeDisabled();
    expect(button).toHaveAttribute('aria-busy', 'true');
    await userEvent.click(button);
    expect(onClick).not.toHaveBeenCalled();
  });

  it('shares the same treatment with action links through buttonClassName', () => {
    expect(buttonClassName({ variant: 'primary', size: 'xs', className: 'route-link' }))
      .toBe('btn btn-primary btn-xs route-link');
  });
});

describe('Form controls', () => {
  it('connects label, hint, required marker, and validation error accessibly', () => {
    render(
      <FormField
        label="Target URL"
        htmlFor="target-url"
        hint="Use an HTTPS endpoint."
        error="The endpoint is unreachable."
        required
      >
        <Input />
      </FormField>,
    );

    const input = screen.getByRole('textbox', { name: 'Target URL' });
    expect(input).toHaveAttribute('id', 'target-url');
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAccessibleDescription('Use an HTTPS endpoint. The endpoint is unreachable.');
    expect(screen.getByRole('alert')).toHaveTextContent('The endpoint is unreachable.');
    expect(screen.getByText('*')).toHaveAttribute('aria-hidden', 'true');
    expect(input.parentElement).toHaveAttribute('data-field-invalid', 'true');
  });

  it('preserves caller ids, classes, props, and forwarded refs', () => {
    const inputRef = createRef<HTMLInputElement>();
    render(
      <>
        <p id="name-policy">Visible to project members.</p>
        <FormField label="Name" htmlFor="generated-name" hint="Use a recognizable name.">
          <Input ref={inputRef} id="custom-name" className="wide" aria-describedby="name-policy" />
        </FormField>
        <Select aria-label="Region" defaultValue="us"><option value="us">US</option></Select>
        <Textarea aria-label="Notes" rows={4} />
      </>,
    );

    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveAttribute('id', 'custom-name');
    expect(inputRef.current).toBe(screen.getByRole('textbox', { name: 'Name' }));
    expect(inputRef.current).toHaveClass('control', 'wide');
    expect(inputRef.current).toHaveAccessibleDescription('Visible to project members. Use a recognizable name.');
    expect(screen.getByRole('combobox', { name: 'Region' })).toHaveClass('control');
    expect(screen.getByRole('textbox', { name: 'Notes' })).toHaveAttribute('rows', '4');
  });
});

describe('Page and async states', () => {
  it('composes breadcrumbs, title, subtitle, actions, and content in one page shell', () => {
    render(
      <PageShell
        before={<nav aria-label="Breadcrumbs">Projects / Runs</nav>}
        title="Runs"
        subtitle="Recent network diagnostics"
        action={<Button>New run</Button>}
        className="runs-page"
      >
        <p>Run table</p>
      </PageShell>,
    );

    expect(screen.getByRole('heading', { name: 'Runs' })).toBeInTheDocument();
    expect(screen.getByText('Recent network diagnostics')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'New run' })).toBeInTheDocument();
    expect(screen.getByRole('navigation', { name: 'Breadcrumbs' })).toBeInTheDocument();
    expect(screen.getByText('Run table').parentElement).toHaveClass('page-shell', 'runs-page');
  });

  it('announces loading and supports custom visible placeholders', () => {
    const { rerender } = render(<LoadingState label="Loading runs" />);
    expect(screen.getByRole('status')).toHaveTextContent('Loading runs');

    rerender(<LoadingState label="Loading chart"><div data-testid="loading-placeholder">Chart skeleton</div></LoadingState>);
    expect(screen.getByRole('status')).toHaveTextContent('Loading chart');
    expect(screen.getByTestId('loading-placeholder')).toBeInTheDocument();
  });

  it('renders retry and secondary recovery actions', async () => {
    const user = userEvent.setup();
    const onRetry = vi.fn();
    const onDetails = vi.fn();
    render(
      <ErrorState
        title="Runs unavailable"
        message="The server did not respond."
        onRetry={onRetry}
        secondaryAction={<Button onClick={onDetails}>Details</Button>}
      />,
    );

    expect(screen.getByRole('alert')).toHaveTextContent('Runs unavailable');
    expect(screen.getByRole('alert')).toHaveTextContent('The server did not respond.');
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    await user.click(screen.getByRole('button', { name: 'Details' }));
    expect(onRetry).toHaveBeenCalledOnce();
    expect(onDetails).toHaveBeenCalledOnce();
  });
});
