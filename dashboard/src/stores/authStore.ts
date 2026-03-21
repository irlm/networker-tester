import { create } from 'zustand';

interface AuthState {
  token: string | null;
  email: string | null;
  role: string | null;
  status: string | null;
  authProvider: string | null;
  isAuthenticated: boolean;
  mustChangePassword: boolean;
  login: (token: string, email: string, role: string, mustChangePassword?: boolean, status?: string, authProvider?: string) => void;
  clearPasswordChange: () => void;
  logout: () => void;
}

export const useAuthStore = create<AuthState>((set) => ({
  token: localStorage.getItem('token'),
  email: localStorage.getItem('email'),
  role: localStorage.getItem('role'),
  status: localStorage.getItem('status'),
  authProvider: localStorage.getItem('authProvider'),
  isAuthenticated: !!localStorage.getItem('token'),
  mustChangePassword: localStorage.getItem('mustChangePassword') === 'true',
  login: (token, email, role, mustChangePassword = false, status = 'active', authProvider = 'local') => {
    localStorage.setItem('token', token);
    localStorage.setItem('email', email);
    localStorage.setItem('role', role);
    localStorage.setItem('status', status);
    localStorage.setItem('authProvider', authProvider);
    if (mustChangePassword) {
      localStorage.setItem('mustChangePassword', 'true');
    } else {
      localStorage.removeItem('mustChangePassword');
    }
    set({ token, email, role, status, authProvider, isAuthenticated: true, mustChangePassword });
  },
  clearPasswordChange: () => {
    localStorage.removeItem('mustChangePassword');
    set({ mustChangePassword: false });
  },
  logout: () => {
    localStorage.removeItem('token');
    localStorage.removeItem('email');
    localStorage.removeItem('role');
    localStorage.removeItem('status');
    localStorage.removeItem('mustChangePassword');
    localStorage.removeItem('authProvider');
    set({ token: null, email: null, role: null, status: null, authProvider: null, isAuthenticated: false, mustChangePassword: false });
  },
}));
