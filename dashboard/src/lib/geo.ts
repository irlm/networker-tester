import type { RunGeoInfo } from '../api/types';

/** Compact geo label mirroring Rust GeoInfo::label(): "US · Linköping · AS13335 Cloudflare".
 *  Returns null when every field is absent so callers can data-gate the line. */
export function geoLabel(geo: RunGeoInfo | null | undefined): string | null {
  if (!geo) return null;
  const parts: string[] = [];
  if (geo.country) parts.push(geo.country);
  if (geo.city) parts.push(geo.city);
  if (geo.asn != null && geo.as_org) parts.push(`AS${geo.asn} ${geo.as_org}`);
  else if (geo.asn != null) parts.push(`AS${geo.asn}`);
  else if (geo.as_org) parts.push(geo.as_org);
  return parts.length > 0 ? parts.join(' · ') : null;
}
