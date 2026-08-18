import { useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Modal } from './Modal';

function ModalContent({ onClose, closeDisabled = false }: {
  onClose: () => void;
  closeDisabled?: boolean;
}) {
  return (
    <Modal
      onClose={onClose}
      labelledBy="test-modal-title"
      describedBy="test-modal-description"
      closeDisabled={closeDisabled}
    >
      <h2 id="test-modal-title">Edit tester</h2>
      <p id="test-modal-description">Update the selected tester.</p>
      <input aria-label="Tester name" />
      <button type="button">Cancel</button>
      <button type="button">Save</button>
    </Modal>
  );
}

describe('Modal', () => {
  it('exposes its accessible name and description and focuses the first field', async () => {
    render(<ModalContent onClose={vi.fn()} />);

    const dialog = screen.getByRole('dialog', { name: 'Edit tester' });
    expect(dialog).toHaveAccessibleDescription('Update the selected tester.');
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Tester name' })).toHaveFocus());
  });

  it('traps forward and backward tab navigation inside the dialog', async () => {
    const user = userEvent.setup();
    render(<ModalContent onClose={vi.fn()} />);

    const field = screen.getByRole('textbox', { name: 'Tester name' });
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    const save = screen.getByRole('button', { name: 'Save' });
    await waitFor(() => expect(field).toHaveFocus());

    await user.tab();
    expect(cancel).toHaveFocus();
    await user.tab();
    expect(save).toHaveFocus();
    await user.tab();
    expect(field).toHaveFocus();
    await user.tab({ shift: true });
    expect(save).toHaveFocus();
  });

  it('closes on Escape and backdrop clicks but not on panel clicks', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<ModalContent onClose={onClose} />);

    const dialog = screen.getByRole('dialog');
    await user.click(dialog);
    expect(onClose).not.toHaveBeenCalled();

    await user.click(dialog.parentElement as HTMLElement);
    expect(onClose).toHaveBeenCalledTimes(1);

    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('blocks every dismissal route while closing is disabled', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<ModalContent onClose={onClose} closeDisabled />);

    const backdrop = screen.getByRole('dialog').parentElement as HTMLElement;
    await user.click(backdrop);
    await user.keyboard('{Escape}');
    expect(onClose).not.toHaveBeenCalled();
  });

  it('restores focus to the opener after the dialog unmounts', async () => {
    const user = userEvent.setup();

    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>Open tester</button>
          {open && (
            <Modal onClose={() => setOpen(false)} labelledBy="restore-title">
              <h2 id="restore-title">Tester</h2>
              <input aria-label="Tester hostname" />
              <button type="button" onClick={() => setOpen(false)}>Done</button>
            </Modal>
          )}
        </>
      );
    }

    render(<Harness />);
    const opener = screen.getByRole('button', { name: 'Open tester' });
    await user.click(opener);
    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Tester hostname' })).toHaveFocus());
    await user.click(screen.getByRole('button', { name: 'Done' }));

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
  });

  it('keeps Escape wired to the latest close callback without resetting focus', async () => {
    const firstClose = vi.fn();
    const latestClose = vi.fn();
    const { rerender } = render(<ModalContent onClose={firstClose} />);
    const field = screen.getByRole('textbox', { name: 'Tester name' });
    await waitFor(() => expect(field).toHaveFocus());

    rerender(<ModalContent onClose={latestClose} />);
    fireEvent.keyDown(document, { key: 'Escape' });

    expect(firstClose).not.toHaveBeenCalled();
    expect(latestClose).toHaveBeenCalledOnce();
    expect(field).toHaveFocus();
  });
});
