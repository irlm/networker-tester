import { useEffect, useRef, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';

export function SSOCompletePage() {
  const [searchParams] = useSearchParams();
  const code = searchParams.get('code');
  const [error, setError] = useState(() => code ? '' : 'Missing SSO exchange code');
  const login = useAuthStore((s) => s.login);
  const navigate = useNavigate();
  const exchanged = useRef(false);

  useEffect(() => {
    if (!code || exchanged.current) return;
    exchanged.current = true;

    let cancelled = false;

    api.ssoExchange(code)
      .then((res) => {
        if (cancelled) return;
        login(res.token, res.email, res.role, res.must_change_password, res.status);

        if (res.must_change_password) {
          navigate('/change-password', { replace: true });
        } else if (res.status === 'pending') {
          navigate('/pending', { replace: true });
        } else {
          navigate('/', { replace: true });
        }
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err?.message || 'SSO sign-in failed');
      });

    return () => { cancelled = true; };
  }, [code, login, navigate]);

  if (error) {
    return (
      <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
        <div className="w-72 text-center">
          <div className="mb-8">
            <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
              Networker
            </h1>
            <p className="text-gray-600 text-xs uppercase tracking-widest">
              diagnostics platform
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
            Networker
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            diagnostics platform
          </p>
        </div>
        <p className="text-gray-400 text-sm">Completing sign-in...</p>
      </div>
    </div>
  );
}
