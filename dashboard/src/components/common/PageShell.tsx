import type { ReactNode } from 'react';
import { PageHeader } from './PageHeader';

interface PageShellProps {
  title?: string;
  subtitle?: string;
  action?: ReactNode;
  children: ReactNode;
  className?: string;
  /** Content rendered before the header, normally breadcrumbs. */
  before?: ReactNode;
}

/** Canonical route-page spacing and heading composition. */
export function PageShell({ title, subtitle, action, children, className, before }: PageShellProps) {
  return (
    <div className={`page-shell ${className ?? ''}`}>
      {before}
      {title && <PageHeader title={title} subtitle={subtitle} action={action} />}
      {children}
    </div>
  );
}
