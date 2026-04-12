import { useState, useEffect } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { SsoProviderInfo } from '../api/client';
import { useAuthStore } from '../stores/authStore';
import { useProjectStore } from '../stores/projectStore';

const SSO_ERRORS: Record<string, string> = {
  sso_misconfigured: 'SSO is not properly configured. Contact your admin.',
  id_token_invalid: 'SSO authentication failed. Try again or use email login.',
  admin_link_blocked: 'Admin accounts cannot be linked to SSO automatically. Use email login.',
  sso_failed: 'SSO sign-in failed. Try again.',
};

function MicrosoftIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 21 21">
      <rect x="1" y="1" width="9" height="9" fill="#f25022" />
      <rect x="11" y="1" width="9" height="9" fill="#7fba00" />
      <rect x="1" y="11" width="9" height="9" fill="#00a4ef" />
      <rect x="11" y="11" width="9" height="9" fill="#ffb900" />
    </svg>
  );
}

function GoogleIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24">
      <path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 01-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z" fill="#4285F4" />
      <path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853" />
      <path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z" fill="#FBBC05" />
      <path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335" />
    </svg>
  );
}

function KeyIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-gray-400">
      <path d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4" />
    </svg>
  );
}

export function LoginPage() {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [checking, setChecking] = useState(false);
  const [providers, setProviders] = useState<SsoProviderInfo[]>([]);
  const login = useAuthStore((s) => s.login);
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  // Check for SSO error in URL
  useEffect(() => {
    const ssoError = searchParams.get('error');
    if (ssoError && SSO_ERRORS[ssoError]) {
      setError(SSO_ERRORS[ssoError]);
    }
  }, [searchParams]);

  // Load available SSO providers
  useEffect(() => {
    api.getProviders?.()
      .then(r => setProviders(r.providers || []))
      .catch(() => {});
  }, []);

  const handleEmailContinue = async () => {
    if (!email.trim()) return;
    setChecking(true);
    setError('');
    try {
      const result = await api.checkEmail?.(email.trim());
      if (result?.provider) {
        // Redirect to SSO
        window.location.href = `/api/auth/sso/init?provider=${result.provider}`;
      } else {
        setShowPassword(true);
      }
    } catch {
      setShowPassword(true);
    } finally {
      setChecking(false);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!showPassword) {
      handleEmailContinue();
      return;
    }
    setError('');
    setLoading(true);
    try {
      const res = await api.login(email, password);
      const isPlatformAdmin = res.is_platform_admin ?? res.role === 'admin';
      login(res.token, res.email, res.role, res.must_change_password, res.status, isPlatformAdmin);
      if (res.status === 'pending') {
        navigate('/pending');
      } else if (res.must_change_password) {
        navigate('/change-password');
      } else {
        // Fetch projects and navigate appropriately
        try {
          const projects = await api.getProjects();
          useProjectStore.getState().setProjects(projects);
          if (projects.length === 1) {
            useProjectStore.getState().setActiveProject(projects[0]);
            navigate(`/projects/${projects[0].project_id}`);
          } else {
            navigate('/projects');
          }
        } catch {
          navigate('/');
        }
      }
    } catch {
      setError('Wrong email or password');
    } finally {
      setLoading(false);
    }
  };

  const hasSso = providers.length > 0;

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex flex-col items-center pt-[15vh] p-4">
      <div className="w-full max-w-xs">
        {/* Brand */}
        <div className="text-center mb-8">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            AletheDash
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            network diagnostics
          </p>
        </div>

        {/* Card container */}
        <div className={hasSso ? 'border border-[var(--border-default)] rounded-lg p-6 bg-[var(--bg-surface)]' : ''}>
          {/* SSO Buttons */}
          {providers.map(p => (
            <a
              key={p.id}
              href={`/api/auth/sso/init?provider=${p.id}`}
              className="w-full flex items-center gap-3 px-4 py-3 bg-[var(--bg-raised)] border border-[var(--border-default)] rounded-md text-sm text-gray-200 hover:border-gray-600 transition-colors mb-2"
            >
              {p.type === 'microsoft' && <MicrosoftIcon />}
              {p.type === 'google' && <GoogleIcon />}
              {p.type === 'oidc_generic' && <KeyIcon />}
              Continue with {p.name}
            </a>
          ))}

          {/* Divider */}
          {hasSso && (
            <div className="flex items-center gap-3 my-4">
              <div className="flex-1 h-px bg-[var(--border-default)]" />
              <span className="text-xs text-gray-600 uppercase tracking-wider">or</span>
              <div className="flex-1 h-px bg-[var(--border-default)]" />
            </div>
          )}

          {/* Email + Password form */}
          <form onSubmit={handleSubmit}>
            {error && (
              <div className="text-red-400 text-xs mb-4 flex items-center gap-2">
                <span className="text-red-500">err</span>
                <span>{error}</span>
              </div>
            )}

            <div className="mb-4">
              <label htmlFor="login-email" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                Email
              </label>
              <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
                <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                <input
                  id="login-email"
                  type="email"
                  value={email}
                  onChange={(e) => { setEmail(e.target.value); setShowPassword(false); }}
                  className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                  placeholder="you@company.com"
                  autoFocus
                />
              </div>
            </div>

            {showPassword && (
              <div className="mb-4">
                <label htmlFor="login-password" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                  Password
                </label>
                <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
                  <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                  <input
                    id="login-password"
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                    placeholder="••••••••"
                    autoFocus
                  />
                </div>
              </div>
            )}

            <button
              type="submit"
              disabled={loading || checking}
              className="w-full bg-cyan-600 hover:bg-cyan-500 text-white py-2.5 rounded text-sm font-medium transition-colors disabled:opacity-50 mt-2"
            >
              {loading ? 'Signing in...' : checking ? 'Checking...' : showPassword ? 'Sign in' : 'Continue'}
            </button>

            <div className="mt-4 text-center">
              <Link to="/forgot-password" className="text-xs text-gray-600 hover:text-gray-400 transition-colors">
                Forgot password?
              </Link>
            </div>
          </form>
        </div>
      </div>
    </div>
  );
}
