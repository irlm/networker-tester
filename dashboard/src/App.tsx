import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
import { useAuthStore } from './stores/authStore';
import { useWebSocket, type ConnectionStatus } from './hooks/useWebSocket';
import { Sidebar } from './components/layout/Sidebar';
import { LoginPage } from './pages/LoginPage';
import { DashboardPage } from './pages/DashboardPage';
import { AgentsPage } from './pages/AgentsPage';
import { JobsPage } from './pages/JobsPage';
import { JobDetailPage } from './pages/JobDetailPage';
import { RunsPage } from './pages/RunsPage';

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

function AuthenticatedApp() {
  const status = useWebSocket();

  return (
    <div className="flex min-h-screen bg-[#0a0b0f]">
      <Sidebar connectionDot={<ConnectionDot status={status} />} />
      <main className="flex-1 overflow-auto">
        <Routes>
          <Route path="/" element={<DashboardPage />} />
          <Route path="/agents" element={<AgentsPage />} />
          <Route path="/jobs" element={<JobsPage />} />
          <Route path="/jobs/:jobId" element={<JobDetailPage />} />
          <Route path="/runs" element={<RunsPage />} />
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
