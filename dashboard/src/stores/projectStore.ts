import { create } from 'zustand';

export interface ProjectSummary {
  project_id: string;
  name: string;
  slug: string;
  role: string;
  description?: string;
}

interface ProjectState {
  projects: ProjectSummary[];
  activeProjectId: string | null;
  activeProjectSlug: string | null;
  activeProjectRole: string | null;
  setProjects: (projects: ProjectSummary[]) => void;
  setActiveProject: (project: ProjectSummary) => void;
  clearActiveProject: () => void;
  clear: () => void;
}

function storageGet(key: string): string | null {
  if (typeof window === 'undefined' || typeof window.localStorage?.getItem !== 'function') {
    return null;
  }
  return window.localStorage.getItem(key);
}

function storageSet(key: string, value: string) {
  if (typeof window === 'undefined' || typeof window.localStorage?.setItem !== 'function') {
    return;
  }
  window.localStorage.setItem(key, value);
}

function storageRemove(key: string) {
  if (
    typeof window === 'undefined' ||
    typeof window.localStorage?.removeItem !== 'function'
  ) {
    return;
  }
  window.localStorage.removeItem(key);
}

export const useProjectStore = create<ProjectState>((set) => ({
  projects: [],
  activeProjectId: storageGet('activeProjectId'),
  activeProjectSlug: storageGet('activeProjectSlug'),
  activeProjectRole: storageGet('activeProjectRole'),
  setProjects: (projects) => set({ projects }),
  setActiveProject: (project) => {
    storageSet('activeProjectId', project.project_id);
    storageSet('activeProjectSlug', project.slug);
    storageSet('activeProjectRole', project.role);
    set({
      activeProjectId: project.project_id,
      activeProjectSlug: project.slug,
      activeProjectRole: project.role,
    });
  },
  clearActiveProject: () => {
    storageRemove('activeProjectId');
    storageRemove('activeProjectSlug');
    storageRemove('activeProjectRole');
    set({ activeProjectId: null, activeProjectSlug: null, activeProjectRole: null });
  },
  clear: () => {
    storageRemove('activeProjectId');
    storageRemove('activeProjectSlug');
    storageRemove('activeProjectRole');
    set({ projects: [], activeProjectId: null, activeProjectSlug: null, activeProjectRole: null });
  },
}));
