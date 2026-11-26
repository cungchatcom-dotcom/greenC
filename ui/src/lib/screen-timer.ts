export interface ScreenTimer {
  shown(fresh: boolean): void;
}

/**
 * Measures time from screen mount until its first rendered data, reporting once.
 * `cached` is `!fresh`.
 */
export function createScreenTimer(
  report: (ms: number, cached: boolean) => void,
  now: () => number = () => performance.now()
): ScreenTimer {
  const startedAt = now();
  let reported = false;

  return {
    shown(fresh: boolean): void {
      if (reported) return;
      reported = true;
      const ms = Math.max(0, Math.round(now() - startedAt));
      report(ms, !fresh);
    },
  };
}
