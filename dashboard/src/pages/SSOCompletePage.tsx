import { useEffect, useRef, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { PendingProject } from '../api/client';
import { useAuthStore } from '../stores/authStore';
import { useProjectStore } from '../stores/projectStore';
import { PendingProjectsModal } from '../components/PendingProjectsModal';

export function SSOCompletePage() {
  const [searchParams] = useSearchParams();
  const code = searchParams.get('code');
  const [error, setError] = useState(() => code ? '' : 'Missing SSO exchange code');
  const [pendingProjects, setPendingProjects] = useState<PendingProject[]>([]);
  const login = useAuthStore((s) => s.login);
  const navigate = useNavigate();
  const exchanged = useRef(false);

  const navigateToProject = async () => {
    try {
      const projects = await api.getProjects();
      useProjectStore.getState().setProjects(projects);
      if (projects.length === 1) {
        useProjectStore.getState().setActiveProject(projects[0]);
        navigate(`/projects/${projects[0].project_id}`, { replace: true });
      } else {
        navigate('/projects', { replace: true });
      }
    } catch {
      navigate('/', { replace: true });
    }
  };

  useEffect(() => {
    if (!code || exchanged.current) return;
    exchanged.current = true;

    let cancelled = false;

    api.ssoExchange(code)
      .then(async (res) => {
        if (cancelled) return;
        const isPlatformAdmin = res.is_platform_admin ?? res.role === 'admin';
        login(res.token, res.email, res.role, res.must_change_password, res.status, isPlatformAdmin);

        if (res.must_change_password) {
          navigate('/change-password', { replace: true });
        } else if (res.status === 'pending') {
          navigate('/pending', { replace: true });
        } else {
          // Check for pending project invitations before navigating
          try {
            const { pending } = await api.getPendingProjects();
            if (pending.length > 0) {
              if (!cancelled) setPendingProjects(pending);
              return; // Show modal — navigation happens in onComplete
            }
          } catch { /* ignore — proceed to navigate */ }
          await navigateToProject();
        }
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err?.message || 'SSO sign-in failed');
      });

    return () => { cancelled = true; };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code, login, navigate]);

  if (pendingProjects.length > 0) {
    return (
      <PendingProjectsModal
        projects={pendingProjects}
        onComplete={() => {
          setPendingProjects([]);
          navigateToProject();
        }}
      />
    );
  }

  if (error) {
    return (
      <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
        <div className="w-72 text-center">
          <div className="mb-8">
            <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
              AletheDash
            </h1>
            <p className="text-gray-600 text-xs uppercase tracking-widest">
              network diagnostics
            </p>
          </div>
          <div className="text-red-400 text-sm mb-4">
            <span className="text-red-500">err</span>{' '}
            <span>{error}</span>
          </div>
          <a
            href="/login"
            className="text-xs text-gray-500 hover:text-gray-300 transition-colors"
          >
            Return to login
          </a>
        </div>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
      <div className="w-72 text-center">
        <div className="mb-8">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            AletheDash
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            network diagnostics
          </p>
        </div>
        <p className="text-gray-400 text-sm">Completing sign-in...</p>
      </div>
    </div>
  );
}
