import { useEffect } from 'react';
import { BrowserRouter, Routes, Route, Navigate, useLocation } from 'react-router-dom';
import { useAuthStore } from './stores/authStore';
import { useProjectStore } from './stores/projectStore';
import { api } from './api/client';
import { useWebSocket, type ConnectionStatus } from './hooks/useWebSocket';
import { Sidebar } from './components/layout/Sidebar';
import { ToastContainer } from './components/common/Toast';
import { LoginPage } from './pages/LoginPage';
import { ForgotPasswordPage } from './pages/ForgotPasswordPage';
import { ResetPasswordPage } from './pages/ResetPasswordPage';
import { ChangePasswordPage } from './pages/ChangePasswordPage';
import { DashboardPage } from './pages/DashboardPage';
import { JobsPage } from './pages/JobsPage';
import { JobDetailPage } from './pages/JobDetailPage';
import { RunsPage } from './pages/RunsPage';
import { RunDetailPage } from './pages/RunDetailPage';
import { DeployPage } from './pages/DeployPage';
import { DeployDetailPage } from './pages/DeployDetailPage';
import { SchedulesPage } from './pages/SchedulesPage';
import { SettingsPage } from './pages/SettingsPage';
import { UsersPage } from './pages/UsersPage';
import { PendingPage } from './pages/PendingPage';
import { SSOCompletePage } from './pages/SSOCompletePage';
import { ProjectsPage } from './pages/ProjectsPage';
import { ProjectMembersPage } from './pages/ProjectMembersPage';
import { CloudAccountsPage } from './pages/CloudAccountsPage';
import { ShareLinksPage } from './pages/ShareLinksPage';
import { ShareViewPage } from './pages/ShareViewPage';
import { AcceptInvitePage } from './pages/AcceptInvitePage';
import { CommandApprovalsPage } from './pages/CommandApprovalsPage';
import { SystemDashboardPage } from './pages/SystemDashboardPage';
import { BenchmarksPage } from './pages/BenchmarksPage';

const statusColors: Record<ConnectionStatus, string> = {
  connected: 'bg-green-400',
  connecting: 'bg-yellow-400 motion-safe:animate-pulse',
  disconnected: 'bg-red-400',
};

const statusLabels: Record<ConnectionStatus, string> = {
  connected: 'WebSocket connected',
  connecting: 'WebSocket connecting',
  disconnected: 'WebSocket disconnected',
};

function ConnectionDot({ status }: { status: ConnectionStatus }) {
  return (
    <span
      className={`inline-block w-2 h-2 rounded-full ${statusColors[status]}`}
      title={statusLabels[status]}
      aria-label={statusLabels[status]}
      role="status"
    />
  );
}

function ConnectionBanner({ status }: { status: ConnectionStatus }) {
  if (status === 'connected') return null;

  return (
    <div
      className="bg-yellow-500/15 border-b border-yellow-500/30 px-4 py-2 flex items-center gap-2 text-sm text-yellow-400"
      role="alert"
    >
      <span className="w-2 h-2 rounded-full bg-yellow-400 motion-safe:animate-pulse" />
      {status === 'connecting'
        ? 'Live updates reconnecting...'
        : 'Live updates disconnected. Reconnecting...'}
    </div>
  );
}

function ProjectRedirect() {
  const activeProjectId = useProjectStore(s => s.activeProjectId);
  if (activeProjectId) return <Navigate to={`/projects/${activeProjectId}`} replace />;
  return <Navigate to="/projects" replace />;
}

function AuthenticatedApp() {
  const status = useWebSocket();
  const mustChangePassword = useAuthStore((s) => s.mustChangePassword);
  const userStatus = useAuthStore((s) => s.status);
  const role = useAuthStore((s) => s.role);
  const isPlatformAdmin = useAuthStore((s) => s.isPlatformAdmin);
  const location = useLocation();

  // Fetch projects on mount
  useEffect(() => {
    api.getProjects().then(projects => {
      useProjectStore.getState().setProjects(projects);
      // Auto-select if only one project and no active
      if (!useProjectStore.getState().activeProjectId && projects.length === 1) {
        useProjectStore.getState().setActiveProject(projects[0]);
      }
    }).catch(() => {});
  }, []);

  // Pending users can only access /pending and /change-password
  if (userStatus === 'pending' && location.pathname !== '/pending' && location.pathname !== '/change-password') {
    return <Navigate to="/pending" />;
  }

  if (mustChangePassword && location.pathname !== '/change-password') {
    return <Navigate to="/change-password" />;
  }

  const isAdmin = role === 'admin' || isPlatformAdmin;

  return (
    <div className="flex min-h-screen bg-[var(--bg-base)]">
      <Sidebar connectionDot={<ConnectionDot status={status} />} />
      <main className="flex-1 overflow-auto pt-12 md:pt-0">
        <ConnectionBanner status={status} />
        <ToastContainer />
        <Routes>
          {/* Project list */}
          <Route path="/projects" element={<ProjectsPage />} />

          {/* Project-scoped routes */}
          <Route path="/projects/:projectId" element={<DashboardPage />} />
          <Route path="/projects/:projectId/tests" element={<JobsPage />} />
          <Route path="/projects/:projectId/tests/:jobId" element={<JobDetailPage />} />
          <Route path="/projects/:projectId/runs" element={<RunsPage />} />
          <Route path="/projects/:projectId/runs/:runId" element={<RunDetailPage />} />
          <Route path="/projects/:projectId/deploy" element={<DeployPage />} />
          <Route path="/projects/:projectId/deploy/:deploymentId" element={<DeployDetailPage />} />
          <Route path="/projects/:projectId/schedules" element={<SchedulesPage />} />
          <Route path="/projects/:projectId/settings" element={<SettingsPage />} />
          <Route path="/projects/:projectId/members" element={<ProjectMembersPage />} />
          <Route path="/projects/:projectId/cloud-accounts" element={<CloudAccountsPage />} />
          <Route path="/projects/:projectId/share-links" element={<ShareLinksPage />} />
          <Route path="/projects/:projectId/approvals" element={<CommandApprovalsPage />} />

          {/* Platform routes */}
          <Route path="/benchmarks" element={<BenchmarksPage />} />
          {isPlatformAdmin && <Route path="/admin/system" element={<SystemDashboardPage />} />}
          {isAdmin && <Route path="/users" element={<UsersPage />} />}
          <Route path="/change-password" element={<ChangePasswordPage />} />
          <Route path="/pending" element={<PendingPage />} />

          {/* Root redirect */}
          <Route path="/" element={<ProjectRedirect />} />

          {/* Catch-all: redirect unknown routes to project list */}
          <Route path="*" element={<Navigate to="/projects" replace />} />
        </Routes>
      </main>
    </div>
  );
}

function App() {
  const isAuthenticated = useAuthStore((s) => s.isAuthenticated);

  return (
    <BrowserRouter>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/sso-complete" element={<SSOCompletePage />} />
        <Route path="/forgot-password" element={<ForgotPasswordPage />} />
        <Route path="/reset-password" element={<ResetPasswordPage />} />
        <Route path="/share/:token" element={<ShareViewPage />} />
        <Route path="/invite/:token" element={<AcceptInvitePage />} />
        <Route
          path="/*"
          element={
            isAuthenticated ? <AuthenticatedApp /> : <Navigate to="/login" />
          }
        />
      </Routes>
    </BrowserRouter>
  );
}

export default App;
