import { useEffect, useState } from 'react';
import { api, type Agent } from '../api/client';
import { StatusBadge } from '../components/common/StatusBadge';

export function AgentsPage() {
  const [agents, setAgents] = useState<Agent[]>([]);

  useEffect(() => {
    api.getAgents().then((r) => setAgents(r.agents)).catch(console.error);
    const interval = setInterval(() => {
      api.getAgents().then((r) => setAgents(r.agents)).catch(console.error);
    }, 10000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="p-6">
      <h2 className="text-xl font-bold text-gray-100 mb-6">Agents</h2>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {agents.map((agent) => (
          <div
            key={agent.agent_id}
            className="bg-[#12131a] border border-gray-800 rounded-lg p-4"
          >
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-sm font-medium text-gray-200">{agent.name}</h3>
              <StatusBadge status={agent.status} />
            </div>
            <div className="space-y-1 text-xs text-gray-500">
              {agent.region && <p>Region: {agent.region}</p>}
              {agent.os && <p>OS: {agent.os} {agent.arch}</p>}
              {agent.version && <p>Version: {agent.version}</p>}
              {agent.last_heartbeat && (
                <p>
                  Last seen:{' '}
                  {new Date(agent.last_heartbeat).toLocaleTimeString()}
                </p>
              )}
            </div>
          </div>
        ))}

        {agents.length === 0 && (
          <p className="text-gray-600 text-sm col-span-full">
            No agents registered. Start an agent with AGENT_API_KEY to connect.
          </p>
        )}
      </div>
    </div>
  );
}
