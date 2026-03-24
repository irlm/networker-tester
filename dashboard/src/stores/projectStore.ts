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

export const useProjectStore = create<ProjectState>((set) => ({
  projects: [],
  activeProjectId: localStorage.getItem('activeProjectId'),
  activeProjectSlug: localStorage.getItem('activeProjectSlug'),
  activeProjectRole: localStorage.getItem('activeProjectRole'),
  setProjects: (projects) => set({ projects }),
  setActiveProject: (project) => {
    localStorage.setItem('activeProjectId', project.project_id);
    localStorage.setItem('activeProjectSlug', project.slug);
    localStorage.setItem('activeProjectRole', project.role);
    set({
      activeProjectId: project.project_id,
      activeProjectSlug: project.slug,
      activeProjectRole: project.role,
    });
  },
  clearActiveProject: () => {
    localStorage.removeItem('activeProjectId');
    localStorage.removeItem('activeProjectSlug');
    localStorage.removeItem('activeProjectRole');
    set({ activeProjectId: null, activeProjectSlug: null, activeProjectRole: null });
  },
  clear: () => {
    localStorage.removeItem('activeProjectId');
    localStorage.removeItem('activeProjectSlug');
    localStorage.removeItem('activeProjectRole');
    set({ projects: [], activeProjectId: null, activeProjectSlug: null, activeProjectRole: null });
  },
}));
