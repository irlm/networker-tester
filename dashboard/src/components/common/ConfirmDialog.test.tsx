import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ConfirmDialog } from './ConfirmDialog';

describe('ConfirmDialog', () => {
  it('renders nothing while closed', () => {
    render(
      <ConfirmDialog
        open={false}
        title="Delete endpoint?"
        description="This cannot be undone."
        onConfirm={vi.fn()}
        onClose={vi.fn()}
      />,
    );

    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  });

  it('wires alert-dialog semantics and invokes both actions', async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const onClose = vi.fn();
    render(
      <ConfirmDialog
        open
        title="Delete endpoint?"
        description="This cannot be undone."
        confirmLabel="Delete endpoint"
        cancelLabel="Keep endpoint"
        danger
        onConfirm={onConfirm}
        onClose={onClose}
      />,
    );

    const dialog = screen.getByRole('alertdialog', { name: 'Delete endpoint?' });
    expect(dialog).toHaveAccessibleDescription('This cannot be undone.');
    const confirm = screen.getByRole('button', { name: 'Delete endpoint' });
    expect(confirm).toHaveClass('btn-danger');

    await user.click(confirm);
    await user.click(screen.getByRole('button', { name: 'Keep endpoint' }));
    expect(onConfirm).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('locks its actions and dismissal behavior while loading', async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const onClose = vi.fn();
    render(
      <ConfirmDialog
        open
        title="Delete endpoint?"
        description="This cannot be undone."
        confirmLabel="Delete endpoint"
        loading
        onConfirm={onConfirm}
        onClose={onClose}
      />,
    );

    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Working…' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Working…' })).toHaveAttribute('aria-busy', 'true');

    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('alertdialog').parentElement as HTMLElement);
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it('uses the nested layer when a confirmation sits above another surface', () => {
    render(
      <ConfirmDialog
        open
        layer="nested"
        title="Rotate key?"
        description="Existing clients will stop working."
        onConfirm={vi.fn()}
        onClose={vi.fn()}
      />,
    );

    expect(screen.getByRole('alertdialog').parentElement).toHaveClass('!z-[60]');
  });

  it('keeps ARIA references unique when dialogs coexist during a transition', () => {
    render(
      <>
        <ConfirmDialog
          open
          title="Delete endpoint?"
          description="Endpoint description"
          onConfirm={vi.fn()}
          onClose={vi.fn()}
        />
        <ConfirmDialog
          open
          layer="nested"
          title="Rotate key?"
          description="Key description"
          onConfirm={vi.fn()}
          onClose={vi.fn()}
        />
      </>,
    );

    const dialogs = screen.getAllByRole('alertdialog');
    const titleIds = dialogs.map(dialog => dialog.getAttribute('aria-labelledby'));
    const descriptionIds = dialogs.map(dialog => dialog.getAttribute('aria-describedby'));
    expect(new Set(titleIds).size).toBe(2);
    expect(new Set(descriptionIds).size).toBe(2);
    expect(screen.getByRole('alertdialog', { name: 'Delete endpoint?' })).toHaveAccessibleDescription('Endpoint description');
    expect(screen.getByRole('alertdialog', { name: 'Rotate key?' })).toHaveAccessibleDescription('Key description');
  });

  it('lets only the topmost dialog handle Escape', async () => {
    const user = userEvent.setup();
    const onParentClose = vi.fn();
    const onNestedClose = vi.fn();
    render(
      <>
        <ConfirmDialog
          open
          title="Edit tester"
          description="Parent surface"
          onConfirm={vi.fn()}
          onClose={onParentClose}
        />
        <ConfirmDialog
          open
          layer="nested"
          title="Delete tester?"
          description="Nested confirmation"
          onConfirm={vi.fn()}
          onClose={onNestedClose}
        />
      </>,
    );

    await user.keyboard('{Escape}');
    expect(onNestedClose).toHaveBeenCalledOnce();
    expect(onParentClose).not.toHaveBeenCalled();
  });
});
