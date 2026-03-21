import { create } from 'zustand';

interface AuthState {
  token: string | null;
  email: string | null;
  role: string | null;
  status: string | null;
  isAuthenticated: boolean;
  mustChangePassword: boolean;
  login: (token: string, email: string, role: string, mustChangePassword?: boolean, status?: string) => void;
  updateStatus: (status: string) => void;
  clearPasswordChange: () => void;
  logout: () => void;
}

export const useAuthStore = create<AuthState>((set) => ({
  token: localStorage.getItem('token'),
  email: localStorage.getItem('email'),
  role: localStorage.getItem('role'),
  status: localStorage.getItem('status'),
  isAuthenticated: !!localStorage.getItem('token'),
  mustChangePassword: localStorage.getItem('mustChangePassword') === 'true',
  login: (token, email, role, mustChangePassword = false, status = 'active') => {
    localStorage.setItem('token', token);
    localStorage.setItem('email', email);
    localStorage.setItem('role', role);
    localStorage.setItem('status', status);
    if (mustChangePassword) {
      localStorage.setItem('mustChangePassword', 'true');
    } else {
      localStorage.removeItem('mustChangePassword');
    }
    set({ token, email, role, status, isAuthenticated: true, mustChangePassword });
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
    set({ token: null, email: null, role: null, status: null, isAuthenticated: false, mustChangePassword: false });
  },
}));
