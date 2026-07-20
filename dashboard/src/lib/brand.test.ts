import { describe, expect, it } from 'vitest';
import { PRODUCT_NAME } from './brand';

// Pins the product brand (LagHound rename, 2026-07 — see docs/branding.md).
// If this fails you are renaming the product: update this constant, the
// static dashboard/index.html <title>, and docs/branding.md together.
describe('brand', () => {
  it('product name is LagHound', () => {
    expect(PRODUCT_NAME).toBe('LagHound');
  });
});
