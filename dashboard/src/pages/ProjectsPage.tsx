import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { useProjectStore, type ProjectSummary } from '../stores/projectStore';
import { useAuthStore } from '../stores/authStore';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { RoleBadge } from '../components/common/RoleBadge';

export function ProjectsPage() {
  const [loading, setLoading] = useState(true);
  const [showCreate, setShowCreate] = useState(false);
  const [newName, setNewName] = useState('');
  const [newDescription, setNewDescription] = useState('');
  const [creating, setCreating] = useState(false);
  const navigate = useNavigate();
  const addToast = useToast();
  const { projects, setProjects, setActiveProject } = useProjectStore();
  const isPlatformAdmin = useAuthStore(s => s.isPlatformAdmin);

  usePageTitle('Workspaces');

  useEffect(() => {
    api.getProjects()
      .then(p => { setProjects(p); setLoading(false); })
      .catch(() => setLoading(false));
  }, [setProjects]);

  const handleCreate = async () => {
    if (!newName.trim()) return;
    setCreating(true);
    try {
      const result = await api.createProject(newName.trim(), newDescription.trim() || undefined);
      addToast('success', `Workspace "${newName.trim()}" created`);
      setShowCreate(false);
      setNewName('');
      setNewDescription('');
      // Refresh and navigate
      const updated = await api.getProjects();
      setProjects(updated);
      const created = updated.find(p => p.project_id === result.project_id);
      if (created) {
        setActiveProject(created);
        navigate(`/projects/${result.project_id}`);
      }
    } catch {
      addToast('error', 'Failed to create workspace');
    } finally {
      setCreating(false);
    }
  };

  const selectProject = (project: ProjectSummary) => {
    setActiveProject(project);
    navigate(`/projects/${project.project_id}`);
  };

  if (loading) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Workspaces</h2>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {[1, 2, 3].map(i => (
            <div key={i} className="border border-gray-800 rounded p-4">
              <div className="h-5 w-32 bg-gray-800 rounded motion-safe:animate-pulse mb-2" />
              <div className="h-3 w-48 bg-gray-800/60 rounded motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Workspaces</h2>
        {isPlatformAdmin && (
          <button
            onClick={() => setShowCreate(true)}
            className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-2 rounded text-sm transition-colors"
          >
            Create Workspace
          </button>
        )}
      </div>

      {showCreate && (
        <div className="border border-gray-800 rounded p-4 mb-6">
          <h3 className="text-sm text-gray-200 font-medium mb-3">New Workspace</h3>
          <div className="space-y-3">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Name</label>
              <input
                value={newName}
                onChange={e => setNewName(e.target.value)}
                placeholder="My Workspace"
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                autoFocus
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Description (optional)</label>
              <input
                value={newDescription}
                onChange={e => setNewDescription(e.target.value)}
                placeholder="Production network diagnostics"
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              />
            </div>
            <div className="flex gap-2">
              <button
                onClick={handleCreate}
                disabled={creating || !newName.trim()}
                className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
              >
                {creating ? 'Creating...' : 'Create'}
              </button>
              <button
                onClick={() => { setShowCreate(false); setNewName(''); setNewDescription(''); }}
                className="text-gray-400 hover:text-gray-200 px-4 py-1.5 text-sm"
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}

      {projects.length === 0 ? (
        <div className="border border-gray-800 rounded p-8 text-center">
          <p className="text-gray-500 text-sm mb-2">No workspaces yet</p>
          {isPlatformAdmin && (
            <button
              onClick={() => setShowCreate(true)}
              className="text-xs text-cyan-400"
            >
              Create your first workspace
            </button>
          )}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {projects.map(project => (
            <button
              key={project.project_id}
              onClick={() => selectProject(project)}
              className="border border-gray-800 rounded p-4 text-left hover:border-gray-700 transition-colors"
            >
              <div className="flex items-center justify-between mb-2">
                <h3 className="text-sm text-gray-100 font-medium truncate">{project.name}</h3>
                <RoleBadge role={project.role} />
              </div>
              {project.description && (
                <p className="text-xs text-gray-500 truncate mb-2">{project.description}</p>
              )}
              <p className="text-xs text-gray-600">
                {project.slug}
              </p>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
