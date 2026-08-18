import type { ButtonHTMLAttributes } from 'react';
import { buttonClassName, type ButtonSize, type ButtonVariant } from './button-styles';

export type { ButtonSize, ButtonVariant } from './button-styles';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Replaces the label while preserving the button width and semantics. */
  loading?: boolean;
  loadingLabel?: string;
}

/** Canonical tactile action key for the dashboard. */
export function Button({
  variant = 'secondary',
  size = 'sm',
  loading = false,
  loadingLabel = 'Working…',
  disabled,
  className,
  children,
  type = 'button',
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={buttonClassName({ variant, size, className })}
      {...props}
    >
      {loading ? loadingLabel : children}
    </button>
  );
}
