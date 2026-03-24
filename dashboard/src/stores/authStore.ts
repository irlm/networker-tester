import { create } from 'zustand';

interface AuthState {
  token: string | null;
  email: string | null;
  role: string | null;
  status: string | null;
  isPlatformAdmin: boolean;
  isAuthenticated: boolean;
  mustChangePassword: boolean;
  login: (token: string, email: string, role: string, mustChangePassword?: boolean, status?: string, isPlatformAdmin?: boolean) => void;
  updateStatus: (status: string) => void;
  clearPasswordChange: () => void;
  logout: () => void;
}

export const useAuthStore = create<AuthState>((set) => ({
  token: localStorage.getItem('token'),
  email: localStorage.getItem('email'),
  role: localStorage.getItem('role'),
  status: localStorage.getItem('status'),
  isPlatformAdmin: localStorage.getItem('isPlatformAdmin') === 'true',
  isAuthenticated: !!localStorage.getItem('token'),
  mustChangePassword: localStorage.getItem('mustChangePassword') === 'true',
  login: (token, email, role, mustChangePassword = false, status = 'active', isPlatformAdmin = false) => {
    localStorage.setItem('token', token);
    localStorage.setItem('email', email);
    localStorage.setItem('role', role);
    localStorage.setItem('status', status);
    localStorage.setItem('isPlatformAdmin', isPlatformAdmin ? 'true' : 'false');
    if (mustChangePassword) {
      localStorage.setItem('mustChangePassword', 'true');
    } else {
      localStorage.removeItem('mustChangePassword');
    }
    set({ token, email, role, status, isPlatformAdmin, isAuthenticated: true, mustChangePassword });
  },
  updateStatus: (status) => {
    localStorage.setItem('status', status);
    set({ status });
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
    localStorage.removeItem('isPlatformAdmin');
    set({ token: null, email: null, role: null, status: null, isPlatformAdmin: false, isAuthenticated: false, mustChangePassword: false });
  },
}));
