/**
 * Cloud-provider brand colors — the ONE map. Before the 2026-08 UI-7 pass
 * three pages declared their own and disagreed (aws was orange on Cloud
 * Accounts and Settings but yellow on the VM catalog; gcp was green and
 * red). Brand colors are the deliberate exception to the status-hue rules:
 * azure=blue and aws=orange mean the VENDOR, never a status.
 *
 * Auth/SSO identity providers (UsersPage: microsoft/google/local) are a
 * different semantic and keep their own map.
 */
const TEXT: Record<string, string> = {
  azure: 'text-blue-400',
  aws: 'text-orange-400',
  gcp: 'text-green-400',
};

export function cloudProviderText(provider: string): string {
  return TEXT[provider.toLowerCase()] ?? 'text-gray-400';
}

export function cloudProviderBadge(provider: string): string {
  switch (provider.toLowerCase()) {
    case 'azure': return 'bg-blue-500/20 text-blue-400';
    case 'aws': return 'bg-orange-500/20 text-orange-400';
    case 'gcp': return 'bg-green-500/20 text-green-400';
    default: return 'bg-gray-500/20 text-gray-400';
  }
}
