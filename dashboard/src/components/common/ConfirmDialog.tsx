import { useId, type ReactNode } from 'react';
import { Button } from './Button';
import { Modal } from './Modal';

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  loading?: boolean;
  layer?: 'default' | 'nested';
  onConfirm: () => void;
  onClose: () => void;
}

/** Accessible confirmation shell for destructive or irreversible actions. */
export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  danger = false,
  loading = false,
  layer = 'default',
  onConfirm,
  onClose,
}: ConfirmDialogProps) {
  const instanceId = useId();
  if (!open) return null;
  const titleId = `${instanceId}-confirm-title`;
  const descriptionId = `${instanceId}-confirm-description`;

  return (
    <Modal
      onClose={onClose}
      labelledBy={titleId}
      describedBy={descriptionId}
      role="alertdialog"
      closeDisabled={loading}
      rootClassName={layer === 'nested' ? '!z-[60]' : undefined}
    >
      <h2 id={titleId} className="text-sm font-bold text-gray-100">{title}</h2>
      <div id={descriptionId} className="mt-2 text-sm text-gray-400">{description}</div>
      <div className="mt-5 flex justify-end gap-2">
        <Button onClick={onClose} disabled={loading}>{cancelLabel}</Button>
        <Button
          variant={danger ? 'danger' : 'primary'}
          onClick={onConfirm}
          loading={loading}
          loadingLabel="Working…"
        >
          {confirmLabel}
        </Button>
      </div>
    </Modal>
  );
}
