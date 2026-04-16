import { NavLink, Outlet } from 'react-router-dom';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';

/**
 * Unified Cloud VMs page (v0.27.22).
 *
 * Previously this project had three disconnected top-level entries —
 * `Infra` (endpoints), `Testers` (probe-running clients), and `VM History`
 * (audit log). Same underlying concept ("VM you own in a cloud") but three
 * different URLs and mental models. New users couldn't tell why their
 * `networker-endpoint` lived in Infra and their `networker-agent` tester
 * lived under Testers.
 *
 * This layout wraps the three existing pages in a tabbed shell so the nav
 * sidebar shows one entry, users land on Testers by default, and the
 * tab bar makes the three sub-resources discoverable in one click. The
 * child components are unchanged — we deliberately avoided refactoring
 * them so this PR stays a pure navigation restructure, easy to revert and
 * easy to review.
 */
export function CloudVmsLayout() {
  usePageTitle('Infrastructure');
  const { projectId } = useProject();
  const base = `/projects/${projectId}/vms`;

  const tabClass = ({ isActive }: { isActive: boolean }) =>
    `px-4 py-2 text-xs tracking-wider uppercase font-mono border-b-2 transition-colors ${
      isActive
        ? 'text-cyan-400 border-cyan-500'
        : 'text-gray-500 hover:text-gray-300 border-transparent'
    }`;

  return (
    <div>
      {/* Tab bar — uses NavLink so the active tab is picked up from the URL.
          Keeping it outside the inner page's padding block means each child
          page keeps its own heading + subtitle without extra work. */}
      <div className="border-b border-gray-800 flex items-center gap-1 px-4 md:px-6 bg-[var(--bg-surface)]">
        <NavLink end to={`${base}/testers`} className={tabClass}>
          Runners
        </NavLink>
        <NavLink end to={`${base}/endpoints`} className={tabClass}>
          Targets
        </NavLink>
        <NavLink end to={`${base}/history`} className={tabClass}>
          History
        </NavLink>
      </div>

      <Outlet />
    </div>
  );
}

export default CloudVmsLayout;
