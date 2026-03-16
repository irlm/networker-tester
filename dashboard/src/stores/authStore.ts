import { create } from 'zustand';

interface AuthState {
  token: string | null;
  username: string | null;
  role: string | null;
  isAuthenticated: boolean;
  mustChangePassword: boolean;
  login: (token: string, username: string, role: string, mustChangePassword?: boolean) => void;
  clearPasswordChange: () => void;
  logout: () => void;
}

export const useAuthStore = create<AuthState>((set) => ({
  token: localStorage.getItem('token'),
  username: localStorage.getItem('username'),
  role: localStorage.getItem('role'),
  isAuthenticated: !!localStorage.getItem('token'),
  mustChangePassword: localStorage.getItem('mustChangePassword') === 'true',
  login: (token, username, role, mustChangePassword = false) => {
    localStorage.setItem('token', token);
    localStorage.setItem('username', username);
    localStorage.setItem('role', role);
    if (mustChangePassword) {
      localStorage.setItem('mustChangePassword', 'true');
    } else {
      localStorage.removeItem('mustChangePassword');
    }
    set({ token, username, role, isAuthenticated: true, mustChangePassword });
  },
  clearPasswordChange: () => {
    localStorage.removeItem('mustChangePassword');
    set({ mustChangePassword: false });
  },
  logout: () => {
    localStorage.removeItem('token');
    localStorage.removeItem('username');
    localStorage.removeItem('role');
    localStorage.removeItem('mustChangePassword');
    set({ token: null, username: null, role: null, isAuthenticated: false, mustChangePassword: false });
  },
}));
