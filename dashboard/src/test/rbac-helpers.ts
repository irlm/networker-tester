// Shared helpers for the RBAC (role-gating) test suite.
//
// Role model under test:
//   - project role (projectStore.activeProjectRole): 'viewer' | 'operator' | 'admin'
//       isOperator     = admin | operator   (useProject.ts)
//       isProjectAdmin = admin              (useProject.ts)
//   - account role (authStore.role): 'user' | 'admin'
//   - platform admin (authStore.isPlatformAdmin): cross-account superuser
//       isAdmin (App.tsx / Sidebar.tsx) = account admin | platform admin

import { useProjectStore } from '../stores/projectStore';
import { useAuthStore } from '../stores/authStore';

export type ProjectRole = 'viewer' | 'operator' | 'admin';

/** Set the active project + role the way the app does after project selection. */
export function setProjectRole(role: ProjectRole, projectId = 'p-1') {
  useProjectStore.setState({
    activeProjectId: projectId,
    activeProjectSlug: 'test-project',
    activeProjectRole: role,
    projects: [
      {
        project_id: projectId,
        name: 'Test Project',
        slug: 'test-project',
        role,
      },
    ],
  });
}

/** Set the account-level auth state (Sidebar/App admin gating). */
export function setAuthRole(role: 'user' | 'admin', isPlatformAdmin = false) {
  useAuthStore.setState({
    token: 'test-token',
    email: 'rbac@test.dev',
    role,
    status: 'active',
    isPlatformAdmin,
    isAuthenticated: true,
    mustChangePassword: false,
  });
}

export function resetRoleStores() {
  useProjectStore.setState({
    projects: [],
    activeProjectId: null,
    activeProjectSlug: null,
    activeProjectRole: null,
  });
  useAuthStore.setState({
    token: null,
    email: null,
    role: null,
    status: null,
    isPlatformAdmin: false,
    isAuthenticated: false,
    mustChangePassword: false,
  });
}
