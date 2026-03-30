import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';
import { useProjectStore } from '../stores/projectStore';

function createStorageMock() {
  let store = new Map<string, string>();

  return {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => {
      store.set(key, value);
    },
    removeItem: (key: string) => {
      store.delete(key);
    },
    clear: () => {
      store = new Map<string, string>();
    },
  };
}

const storage = createStorageMock();

Object.defineProperty(window, 'localStorage', {
  value: storage,
  configurable: true,
});

Object.defineProperty(globalThis, 'localStorage', {
  value: storage,
  configurable: true,
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  useProjectStore.getState().clear();
  document.title = 'AletheDash';
});
