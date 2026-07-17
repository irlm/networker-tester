// RBAC decision layer: useProject() role derivation.
//
// This is THE gating function — every isOperator/isProjectAdmin check in the
// app derives from these two lines. admin > operator > viewer.

import { renderHook } from '@testing-library/react';
import { describe, it, expect, afterEach } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { useProject } from './useProject';
import { setProjectRole, resetRoleStores, type ProjectRole } from '../test/rbac-helpers';

function deriveFor(role: ProjectRole) {
  setProjectRole(role);
  const { result } = renderHook(() => useProject(), {
    wrapper: ({ children }) => <MemoryRouter>{children}</MemoryRouter>,
  });
  return result.current;
}

describe('useProject role derivation (RBAC decision layer)', () => {
  afterEach(resetRoleStores);

  it('viewer: no operator rights, no admin rights', () => {
    const p = deriveFor('viewer');
    expect(p.isOperator).toBe(false);
    expect(p.isProjectAdmin).toBe(false);
  });

  it('operator: operator rights but NOT project admin', () => {
    const p = deriveFor('operator');
    expect(p.isOperator).toBe(true);
    expect(p.isProjectAdmin).toBe(false);
  });

  it('admin: both operator and project admin rights (admin ⊇ operator)', () => {
    const p = deriveFor('admin');
    expect(p.isOperator).toBe(true);
    expect(p.isProjectAdmin).toBe(true);
  });

  it('no role (logged out / no project): fails closed', () => {
    resetRoleStores();
    const { result } = renderHook(() => useProject(), {
      wrapper: ({ children }) => <MemoryRouter>{children}</MemoryRouter>,
    });
    expect(result.current.isOperator).toBe(false);
    expect(result.current.isProjectAdmin).toBe(false);
  });

  it('unknown role string: fails closed (no substring/casing tricks)', () => {
    setProjectRole('viewer');
    // Simulate a malformed/unexpected role coming back from the API.
    for (const bogus of ['Admin', 'ADMIN', 'administrator', 'operator ', 'op']) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      setProjectRole(bogus as any);
      const { result } = renderHook(() => useProject(), {
        wrapper: ({ children }) => <MemoryRouter>{children}</MemoryRouter>,
      });
      expect(result.current.isOperator, `role "${bogus}" must not grant operator`).toBe(false);
      expect(result.current.isProjectAdmin, `role "${bogus}" must not grant admin`).toBe(false);
    }
  });
});
