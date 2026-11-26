// Notices from crash recovery, one per record: a later report replaces the earlier one.
export function mergeRecoveries<T extends { id?: number | null }>(kept: T[], found: T[]): T[] {
  const reported = new Set(found.map((r) => r.id).filter((id): id is number => id != null));
  return [...kept.filter((r) => r.id == null || !reported.has(r.id)), ...found];
}
