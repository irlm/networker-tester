import { useEffect, useState, useMemo } from 'react';
import { useSearchParams, useNavigate, Link } from 'react-router-dom';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';

export function SSOCompletePage() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const login = useAuthStore((s) => s.login);

  const code = useMemo(() => searchParams.get('code'), [searchParams]);
  const [error, setError] = useState<string | null>(
    code ? null : 'Missing authorization code.'
  );
  const [loading, setLoading] = useState(!!code);

  useEffect(() => {
    if (!code) return;

    let cancelled = false;

    api
      .exchangeCode(code)
      .then((res) => {
        if (cancelled) return;
        login(res.token, res.email, res.role, false, res.status, 'sso');
        navigate('/', { replace: true });
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : 'Failed to complete SSO sign-in.');
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [code, login, navigate]);

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
      <div className="w-80 text-center">
        <div className="mb-6">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            Networker
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            diagnostics platform
          </p>
        </div>

        <div className="border border-[var(--border-default)] rounded-lg p-6 bg-[var(--bg-surface)]">
          {loading && !error && (
            <div className="flex flex-col items-center gap-3">
              <div className="w-6 h-6 border-2 border-green-500 border-t-transparent rounded-full animate-spin" />
              <p className="text-sm text-gray-400">Signing you in...</p>
            </div>
          )}

          {error && (
            <div className="flex flex-col items-center gap-4">
              <p className="text-red-400 text-sm">{error}</p>
              <Link
                to="/login"
                className="text-sm text-green-500 hover:text-green-400 transition-colors"
              >
                Back to login
              </Link>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
