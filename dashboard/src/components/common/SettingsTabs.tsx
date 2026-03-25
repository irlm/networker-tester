import { Link, useLocation } from 'react-router-dom';
import { useProject } from '../../hooks/useProject';

export function SettingsTabs() {
  const { projectId, isProjectAdmin, isOperator } = useProject();
  const location = useLocation();

  if (!projectId) return null;

  const tabs = [
    { path: `/projects/${projectId}/settings`, label: 'General', exact: true },
    ...(isProjectAdmin ? [{ path: `/projects/${projectId}/members`, label: 'Members' }] : []),
    ...(isOperator ? [{ path: `/projects/${projectId}/cloud-accounts`, label: 'Cloud' }] : []),
    ...(isProjectAdmin ? [{ path: `/projects/${projectId}/share-links`, label: 'Share Links' }] : []),
    ...(isProjectAdmin ? [{ path: `/projects/${projectId}/approvals`, label: 'Approvals' }] : []),
  ];

  return (
    <div className="flex gap-1 mb-6 border-b border-gray-800/50 pb-2">
      {tabs.map(tab => {
        const active = 'exact' in tab && tab.exact
          ? location.pathname === tab.path
          : location.pathname.startsWith(tab.path);
        return (
          <Link
            key={tab.path}
            to={tab.path}
            className={`px-3 py-1.5 rounded text-sm transition-colors ${
              active
                ? 'bg-gray-800/40 text-gray-100'
                : 'text-gray-500 hover:text-gray-300'
            }`}
          >
            {tab.label}
          </Link>
        );
      })}
    </div>
  );
}
