import { useState, useRef, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useProjectStore, type ProjectSummary } from '../stores/projectStore';
import { useAuthStore } from '../stores/authStore';
import { RoleBadge } from './common/RoleBadge';

interface ProjectSwitcherProps {
  collapsed: boolean;
  connectionDot?: React.ReactNode;
}

function zoneLabel(projectId: string): string {
  if (projectId.length < 2) return '';
  return projectId.slice(0, 2).toUpperCase();
}

export function ProjectSwitcher({ collapsed, connectionDot }: ProjectSwitcherProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const navigate = useNavigate();
  const { projects, activeProjectId, setActiveProject } = useProjectStore();
  const isPlatformAdmin = useAuthStore(s => s.isPlatformAdmin);

  const activeProject = projects.find(p => p.project_id === activeProjectId);
  const displayName = activeProject?.name || 'Select workspace';

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [open]);

  const selectProject = (project: ProjectSummary) => {
    setActiveProject(project);
    setOpen(false);
    navigate(`/projects/${project.project_id}`);
  };

  if (collapsed) {
    return (
      <div className="flex justify-center">
        <button
          onClick={() => setOpen(!open)}
          className="text-green-400 text-lg font-bold"
          title={displayName}
        >
          {activeProject?.name?.charAt(0)?.toUpperCase() || 'A'}
        </button>
      </div>
    );
  }

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen(!open)}
        className="w-full text-left flex items-center gap-2"
      >
        <h1 className="text-green-400 text-lg font-bold tracking-tight truncate">
          {displayName}
        </h1>
        {connectionDot}
        <span className="text-gray-600 text-xs ml-auto">{open ? '\u25B4' : '\u25BE'}</span>
      </button>
      <p className="text-gray-600 text-xs mt-0.5">network testing</p>

      {open && (
        <div className="absolute top-full left-0 right-0 mt-1 bg-[var(--bg-sidebar)] border border-gray-800 rounded shadow-lg z-50 max-h-64 overflow-y-auto">
          {projects.map(project => (
            <button
              key={project.project_id}
              onClick={() => selectProject(project)}
              className={`w-full text-left px-3 py-2 hover:bg-gray-800/50 transition-colors flex items-center justify-between ${
                project.project_id === activeProjectId ? 'bg-gray-800/30' : ''
              }`}
            >
              <div className="flex items-center gap-1 min-w-0">
                <span className="text-sm text-gray-200 truncate">{project.name}</span>
                <span className="text-[10px] text-gray-500 font-mono whitespace-nowrap">
                  {zoneLabel(project.project_id)}
                </span>
              </div>
              <RoleBadge role={project.role} />
            </button>
          ))}
          {projects.length === 0 && (
            <div className="px-3 py-2 text-xs text-gray-600">No workspaces</div>
          )}
          {isPlatformAdmin && (
            <button
              onClick={() => { setOpen(false); navigate('/projects'); }}
              className="w-full text-left px-3 py-2 border-t border-gray-800 text-xs text-cyan-400 hover:bg-gray-800/50 transition-colors"
            >
              Manage Workspaces
            </button>
          )}
        </div>
      )}
    </div>
  );
}
