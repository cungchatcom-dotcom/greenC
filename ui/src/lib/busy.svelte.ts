// ui/src/lib/busy.svelte.ts: one place that knows an action on the user's files is running.
//
// It lives outside the screens, so leaving a screen and coming back cannot start a second
// action, and every screen scans again once an action is over.

import { untrack } from 'svelte';
import { markFilesChanged } from './cache';
import { mergeRecoveries } from './recoveries';
import { recoverInterruptedRelocations, type RelocationResultDto } from './api';

class FileActions {
  running = $state(0);
  finished = $state(0);
}

export const fileActions = new FileActions();

export async function changingFiles<T>(action: () => Promise<T>): Promise<T> {
  fileActions.running++;
  markFilesChanged();
  try {
    return await action();
  } finally {
    markFilesChanged();
    fileActions.running--;
    fileActions.finished++;
  }
}

/** Call while a component starts: runs `reload` after each file action, failed ones included. */
export function onFilesChanged(reload: () => void): void {
  let seen = fileActions.finished;
  $effect(() => {
    if (fileActions.finished === seen) return;
    seen = fileActions.finished;
    untrack(reload);
  });
}

/** Moves that crash recovery put back, kept until the user has read the notice. */
export const recoveries = $state<{ list: RelocationResultDto[] }>({ list: [] });

let recovering: Promise<void> | null = null;

/** Runs crash recovery once at a time and keeps what it reports. */
export function recoverOnce(): Promise<void> {
  recovering ??= recoverInterruptedRelocations()
    .then((found) => {
      if (found.length > 0) recoveries.list = mergeRecoveries(recoveries.list, found);
    })
    .finally(() => (recovering = null));
  return recovering;
}
