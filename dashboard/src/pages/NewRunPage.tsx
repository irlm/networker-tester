import { Navigate } from 'react-router';
import { useProject } from '../hooks/useProject';

export function NewRunPage() {
  const { projectId } = useProject();
  return <Navigate to={`/projects/${projectId}/tests/new`} replace />;
}
