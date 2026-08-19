import { useEffect } from 'react';
import { useDocsStore } from '../../stores/docsStore';
import type { ScenarioIntentId } from './model';

interface ScenarioKeyboardOptions {
  enabled: boolean;
  activeIntent: ScenarioIntentId;
  scenarioIds: string[];
  selectedScenarioId: string | null;
  setSelectedScenarioId: (id: string) => void;
  setIntent: (intent: ScenarioIntentId) => void;
  toggleMethod: () => void;
  toggleReadiness: () => void;
}

const INTENT_KEYS: Record<string, ScenarioIntentId> = {
  '1': 'url',
  '2': 'route',
  '3': 'endpoint',
  '4': 'benchmark',
};

function focusScenario(id: string) {
  requestAnimationFrame(() => {
    document.querySelector<HTMLElement>(`[data-scenario-id="${id}"] [data-scenario-action]`)?.focus();
  });
}

export function useScenarioKeyboardNavigation({
  enabled,
  activeIntent,
  scenarioIds,
  selectedScenarioId,
  setSelectedScenarioId,
  setIntent,
  toggleMethod,
  toggleReadiness,
}: ScenarioKeyboardOptions) {
  useEffect(() => {
    if (!enabled) return;

    function onKeyDown(event: KeyboardEvent) {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target?.matches('input, textarea, select, [contenteditable="true"]')) return;
      const docs = useDocsStore.getState();
      if (docs.paletteOpen || docs.helpOpen) return;

      const intent = INTENT_KEYS[event.key];
      if (intent) {
        event.preventDefault();
        setIntent(intent);
        return;
      }

      if (event.key === 'h' || event.key === 'l') {
        event.preventDefault();
        const intents: ScenarioIntentId[] = ['url', 'route', 'endpoint', 'benchmark'];
        const current = Math.max(0, intents.indexOf(activeIntent));
        const next = event.key === 'h'
          ? (current - 1 + intents.length) % intents.length
          : (current + 1) % intents.length;
        setIntent(intents[next]);
        return;
      }

      if ((event.key === 'j' || event.key === 'k') && scenarioIds.length) {
        event.preventDefault();
        const delta = event.key === 'j' ? 1 : -1;
        const current = Math.max(0, scenarioIds.indexOf(selectedScenarioId ?? scenarioIds[0]));
        const next = (current + delta + scenarioIds.length) % scenarioIds.length;
        setSelectedScenarioId(scenarioIds[next]);
        focusScenario(scenarioIds[next]);
        return;
      }

      if (event.key === 'r' && scenarioIds[0]) {
        event.preventDefault();
        setSelectedScenarioId(scenarioIds[0]);
        focusScenario(scenarioIds[0]);
      } else if (event.key === 's') {
        event.preventDefault();
        toggleReadiness();
      } else if (event.key === 'm') {
        event.preventDefault();
        toggleMethod();
      } else if (event.key === 'Enter' && selectedScenarioId) {
        if (target?.closest('a, button')) return;
        event.preventDefault();
        document.querySelector<HTMLElement>(
          `[data-scenario-id="${selectedScenarioId}"] [data-scenario-action]`,
        )?.click();
      }
    }

    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [activeIntent, enabled, scenarioIds, selectedScenarioId, setIntent, setSelectedScenarioId, toggleMethod, toggleReadiness]);
}
