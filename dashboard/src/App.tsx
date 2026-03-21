import { BrowserRouter, Routes, Route, Navigate, useLocation } from 'react-router-dom';
import { useAuthStore } from './stores/authStore';
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

function AuthenticatedApp() {
  const status = useWebSocket();
  const mustChangePassword = useAuthStore((s) => s.mustChangePassword);
  const userStatus = useAuthStore((s) => s.status);
  const role = useAuthStore((s) => s.role);
  const location = useLocation();

  // Pending users can only access /pending and /change-password
  if (userStatus === 'pending' && location.pathname !== '/pending' && location.pathname !== '/change-password') {
    return <Navigate to="/pending" />;
  }

  if (mustChangePassword && location.pathname !== '/change-password') {
    return <Navigate to="/change-password" />;
  }

  return (
    <div className="flex min-h-screen bg-[var(--bg-base)]">
      <Sidebar connectionDot={<ConnectionDot status={status} />} />
      <main className="flex-1 overflow-auto pt-12 md:pt-0">
        <ConnectionBanner status={status} />
        <ToastContainer />
        <Routes>
          <Route path="/" element={<DashboardPage />} />
          <Route path="/pending" element={<PendingPage />} />
          <Route path="/change-password" element={<ChangePasswordPage />} />
          <Route path="/deploy" element={<DeployPage />} />
          <Route path="/deploy/:deploymentId" element={<DeployDetailPage />} />
          <Route path="/tests" element={<JobsPage />} />
          <Route path="/schedules" element={<SchedulesPage />} />
          <Route path="/tests/:jobId" element={<JobDetailPage />} />
          {/* Backward compat redirects */}
          <Route path="/jobs" element={<Navigate to="/tests" />} />
          <Route path="/jobs/:jobId" element={<Navigate to="/tests" />} />
          <Route path="/agents" element={<Navigate to="/tests" />} />
          {role === 'admin' && <Route path="/users" element={<UsersPage />} />}
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="/runs" element={<RunsPage />} />
          <Route path="/runs/:runId" element={<RunDetailPage />} />
          <Route path="*" element={<Navigate to="/" />} />
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
