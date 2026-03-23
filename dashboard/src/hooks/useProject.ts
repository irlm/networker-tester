import { useParams } from 'react-router-dom';
import { useProjectStore } from '../stores/projectStore';
import { useEffect } from 'react';

export function useProject() {
  const { projectId } = useParams<{ projectId: string }>();
  const { projects, activeProjectId, setActiveProject } = useProjectStore();

  useEffect(() => {
    if (projectId && projectId !== activeProjectId) {
      const project = projects.find(p => p.project_id === projectId);
      if (project) setActiveProject(project);
    }
  }, [projectId, activeProjectId, projects, setActiveProject]);

  const role = useProjectStore(s => s.activeProjectRole);
  return {
    projectId: projectId || activeProjectId || '',
    projectRole: role,
    isProjectAdmin: role === 'admin',
    isOperator: role === 'admin' || role === 'operator',
  };
}
