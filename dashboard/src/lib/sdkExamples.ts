import type { SdkEndpointCreate } from '../api/types';
import { normalizeHttpUrl } from './url';

export interface SdkExample {
  id: 'csharp' | 'rust';
  language: string;
  runtime: string;
  description: string;
  sourceUrl: string;
  liveUrl?: string;
}

export const PUBLIC_SDK_DEMO_TOKEN =
  import.meta.env.VITE_LAGHOUND_PUBLIC_DEMO_TOKEN?.trim() || 'demo-token-laghound';

export const SDK_EXAMPLES: readonly SdkExample[] = [
  {
    id: 'csharp',
    language: 'C#',
    runtime: '.NET 10 · ASP.NET Core',
    description: 'Minimal service using LagHound.Endpoint middleware.',
    sourceUrl: 'https://github.com/irlm/networker-tester/tree/main/sdk/csharp/Example',
    liveUrl: normalizeHttpUrl(import.meta.env.VITE_LAGHOUND_CSHARP_DEMO_URL),
  },
  {
    id: 'rust',
    language: 'Rust',
    runtime: 'axum · tower',
    description: 'Minimal service merging the laghound router.',
    sourceUrl: 'https://github.com/irlm/networker-tester/tree/main/sdk/rust/example',
    liveUrl: normalizeHttpUrl(import.meta.env.VITE_LAGHOUND_RUST_DEMO_URL),
  },
] as const;

export function endpointDraftForExample(example: SdkExample): Partial<SdkEndpointCreate> | undefined {
  if (!example.liveUrl) return undefined;
  return {
    name: `LagHound ${example.language} reference`,
    description: `Public ${example.runtime} SDK reference on Azure Container Apps`,
    url: example.liveUrl,
    route: '/laghound/echo',
    token: PUBLIC_SDK_DEMO_TOKEN,
  };
}
