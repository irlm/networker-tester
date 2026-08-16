import { useState } from 'react';
import { api } from '../api/client';
import { useAsyncEffect } from './useAsyncEffect';

/**
 * Is the feature-flagged **Docker (local)** provider enabled on this control
 * plane? (`GET /api/version` → `docker_provider`, driven by
 * `DASHBOARD_DOCKER_PROVIDER=1`; production never sets it.) The create-tester
 * modal and the deploy wizards show the "Docker (local)" cloud choice only
 * when this is true — no cloud account, region fixed to `local`, the control
 * plane provisions containers on its own Docker daemon.
 *
 * Fails closed: any error (older control plane without the field, mocked api
 * in tests, network) reads as `false`.
 */
export function useDockerProvider(): boolean {
  const [enabled, setEnabled] = useState(false);
  useAsyncEffect(async (cancelled) => {
    try {
      const v = await api.getVersionInfo();
      if (!cancelled()) setEnabled(v?.docker_provider === true);
    } catch {
      if (!cancelled()) setEnabled(false);
    }
  }, []);
  return enabled;
}

/** Wire value of the docker cloud on tester rows / deploy configs. */
export const DOCKER_CLOUD = 'docker';
/** The single region the docker provider reports. */
export const DOCKER_REGION = 'local';
/** Display label used everywhere the choice is offered. */
export const DOCKER_LABEL = 'Docker (local)';
