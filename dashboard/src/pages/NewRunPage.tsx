import { Navigate } from 'react-router-dom';
import { useProject } from '../hooks/useProject';

export function NewRunPage() {
  const { projectId } = useProject();
  return <Navigate to={`/projects/${projectId}/tests/new`} replace />;
}
