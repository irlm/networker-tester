/** Shared Recharts tooltip style for dark theme */
export const TOOLTIP_STYLE = {
  background: '#0d0e14',
  border: '1px solid #1a1b25',
  borderRadius: 6,
  fontSize: 12,
} as const;

/** Mode IDs that involve throughput/payload transfer */
export const THROUGHPUT_IDS = [
  'download', 'upload', 'download1', 'download2', 'download3',
  'upload1', 'upload2', 'upload3', 'webdownload', 'webupload',
  'udpdownload', 'udpupload',
] as const;
