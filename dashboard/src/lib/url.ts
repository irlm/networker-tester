/** Normalize an absolute HTTP(S) URL for shared validation and configuration. */
export function normalizeHttpUrl(raw: string | undefined): string | undefined {
  if (!raw?.trim()) return undefined;
  try {
    const url = new URL(raw.trim());
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return undefined;
    return url.toString().replace(/\/$/, '');
  } catch {
    return undefined;
  }
}

export function isAbsoluteHttpUrl(raw: string): boolean {
  return normalizeHttpUrl(raw) !== undefined;
}
