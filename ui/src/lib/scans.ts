// ui/src/lib/scans.ts: cache keys shared by more than one screen.

/** The safe cleanup profiles that are on by default (orphan_radar is opt-in). */
export const DEFAULT_CLEANUP_PROFILES = [
  'windows_temp',
  'browser_cache',
  'chat_media',
  'crash_dumps',
  'package_managers',
  'app_logs',
  'old_versions',
];

export function cleanupKey(profileIds: string[]): string {
  return `cleanup.${[...profileIds].sort().join(',')}`;
}
