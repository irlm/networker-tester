/**
 * Single source of truth for the user-facing product name.
 *
 * Brand ≠ infrastructure: crate/binary names (`networker-tester`), C#
 * namespaces (`Networker.*`), env vars (`DASHBOARD_*`), wire headers
 * (`X-Networker-Signature`), and deployment identifiers are NOT brand and
 * must not follow this constant — see docs/branding.md.
 *
 * Static files that cannot import TypeScript (dashboard/index.html) hardcode
 * the name and must be updated by hand on any future rename.
 */
export const PRODUCT_NAME = 'LagHound';
