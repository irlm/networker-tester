import type { ReactNode } from 'react';

/**
 * The canonical data table (table/v2 pick): one render for every viewport.
 *
 * Before this existed, 31 pages hand-rolled their tables (the same header
 * class string appeared 66×, with four competing padding scales) and five
 * pages shipped every row TWICE — a `md:hidden` card list plus a
 * `hidden md:block` table — so each column/copy change had to be made in
 * two places and they drifted. Here narrow viewports keep the SAME table:
 * the container scrolls horizontally (data density is the product's
 * aesthetic; a scrolling table preserves column alignment where cards
 * destroy it) and columns marked `hideBelow` drop out first.
 */
export interface DataTableColumn<T> {
  key: string;
  label: string;
  align?: 'left' | 'right';
  /** Drop this column below the given breakpoint — for secondary detail. */
  hideBelow?: 'md' | 'lg';
  /** Extra cell classes (e.g. `max-w-72 truncate`). */
  cellClass?: string;
  render: (row: T) => ReactNode;
  /** Native tooltip (e.g. the full value behind a truncated cell). */
  titleOf?: (row: T) => string | undefined;
}

interface DataTableProps<T> {
  columns: DataTableColumn<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  /** Extra classes for a row (e.g. a severity background tint). */
  rowClass?: (row: T) => string | undefined;
  /** Rendered centred inside the container when there are no rows. */
  empty?: ReactNode;
  /** Rendered inside the container below the table (pagers, load-more). */
  footer?: ReactNode;
  className?: string;
}

const HIDE = {
  md: 'hidden md:table-cell',
  lg: 'hidden lg:table-cell',
} as const;

export function DataTable<T>({ columns, rows, rowKey, rowClass, empty, footer, className }: DataTableProps<T>) {
  return (
    <div className={`table-container ${className ?? ''}`}>
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-gray-800/50 text-gray-400 text-xs bg-[var(--bg-surface)]">
            {columns.map(col => (
              <th
                key={col.key}
                className={`px-4 py-2.5 font-medium ${col.align === 'right' ? 'text-right' : 'text-left'} ${col.hideBelow ? HIDE[col.hideBelow] : ''}`}
              >
                {col.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map(row => (
            <tr key={rowKey(row)} className={`border-b border-gray-800/50 hover:bg-gray-800/20 ${rowClass?.(row) ?? ''}`}>
              {columns.map(col => (
                <td
                  key={col.key}
                  title={col.titleOf?.(row)}
                  className={`px-4 py-3 text-xs ${col.align === 'right' ? 'text-right' : ''} ${col.hideBelow ? HIDE[col.hideBelow] : ''} ${col.cellClass ?? ''}`}
                >
                  {col.render(row)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {rows.length === 0 && empty && <div className="py-10 text-center">{empty}</div>}
      {footer}
    </div>
  );
}
