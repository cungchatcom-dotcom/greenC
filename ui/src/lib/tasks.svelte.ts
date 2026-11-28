// ui/src/lib/tasks.svelte.ts
// Global singleton store for long-running file tasks (Make Green Now, Relocate, Cleanup, Compact).
// Lives outside page components so minimizing to background or switching tabs never loses progress,
// and strictly ignores read-only scan events (`relocate_scan`, `space_lens`, `compact_scan`).

import { controlTask, listenTaskProgress, type TaskProgressEvent } from './api';

export type ManagedTaskKind =
  | 'make_green'
  | 'external_backup'
  | 'relocate'
  | 'relocate_batch'
  | 'cleanup_execute'
  | 'compact'
  | 'zalo_move';

export interface ActiveTaskState {
  kind: ManagedTaskKind;
  taskId: string | null;
  title: string;
  phase: string;
  percent: number;
  doneBytes: number;
  totalBytes: number;
  doneFiles: number;
  totalFiles: number;
  currentFile: string;
  minimized: boolean;
  steps?: ReadonlyArray<{ key: string; label: string }>;
}

export interface TaskToast {
  kind: 'ok' | 'err' | 'info';
  title: string;
  detail?: string;
}

const IGNORED_SCAN_KINDS = new Set(['relocate_scan', 'space_lens', 'compact_scan']);

class GlobalTaskStore {
  active = $state<ActiveTaskState | null>(null);
  toast = $state<TaskToast | null>(null);
  private toastTimer: number | null = null;
  private listenerAttached = false;

  initListener(): () => void {
    if (this.listenerAttached) {
      return () => {};
    }
    this.listenerAttached = true;

    const unlistenPromise = listenTaskProgress((event: TaskProgressEvent) => {
      if (IGNORED_SCAN_KINDS.has(event.kind)) return;
      const cur = this.active;
      if (!cur) return;
      if (event.kind !== cur.kind) return;
      if (cur.taskId && event.task_id && cur.taskId !== event.task_id) return;

      if (!cur.taskId && event.task_id) {
        cur.taskId = event.task_id;
      }

      // If backend already emitted terminal completion, auto-close immediately.
      if (event.state === 'completed' || event.state === 'cancelled' || event.state === 'failed') {
        this.active = null;
        return;
      }

      cur.percent = Math.min(100, Math.max(0, event.percent));
      cur.phase = event.phase || cur.phase;
      if (event.title) cur.title = event.title;
      cur.doneBytes = event.done_bytes;
      if (event.total_bytes > 0) cur.totalBytes = event.total_bytes;
      cur.doneFiles = event.done_files;
      if (event.total_files > 0) cur.totalFiles = event.total_files;
      if (event.detail) cur.currentFile = event.detail;
    });

    return () => {
      this.listenerAttached = false;
      unlistenPromise.then((u) => u()).catch(() => {});
    };
  }

  showToast(toast: TaskToast, durationMs = 6000) {
    this.toast = toast;
    if (this.toastTimer !== null && typeof window !== 'undefined') {
      window.clearTimeout(this.toastTimer);
    }
    if (typeof window !== 'undefined') {
      this.toastTimer = window.setTimeout(() => {
        this.toast = null;
        this.toastTimer = null;
      }, durationMs);
    }
  }

  dismissToast() {
    this.toast = null;
    if (this.toastTimer !== null && typeof window !== 'undefined') {
      window.clearTimeout(this.toastTimer);
      this.toastTimer = null;
    }
  }

  minimize() {
    if (this.active) {
      this.active.minimized = true;
    }
  }

  restore() {
    if (this.active) {
      this.active.minimized = false;
    }
  }

  async cancel() {
    const id = this.active?.taskId;
    if (id) {
      try {
        await controlTask(id, 'cancel');
      } catch (err) {
        console.error('Failed to cancel task:', err);
      }
    }
  }

  async run<T>(
    config: {
      kind: ManagedTaskKind;
      title: string;
      phase?: string;
      totalBytes?: number;
      totalFiles?: number;
      steps?: ReadonlyArray<{ key: string; label: string }>;
    },
    work: () => Promise<T>,
    onDoneToast?: (result: T, wasMinimized: boolean) => TaskToast | null
  ): Promise<T> {
    this.active = {
      kind: config.kind,
      taskId: null,
      title: config.title,
      phase: config.phase ?? 'preparing',
      percent: 2,
      doneBytes: 0,
      totalBytes: config.totalBytes ?? 0,
      doneFiles: 0,
      totalFiles: config.totalFiles ?? 0,
      currentFile: '',
      minimized: false,
      steps: config.steps,
    };

    try {
      const result = await work();
      const wasMinimized = Boolean(this.active?.minimized);
      // Auto-close modal BEFORE returning so follow-up scans never hijack it.
      this.active = null;
      if (onDoneToast) {
        const toast = onDoneToast(result, wasMinimized);
        if (toast) this.showToast(toast);
      }
      return result;
    } catch (err) {
      this.active = null;
      this.showToast({
        kind: 'err',
        title: config.title,
        detail: err instanceof Error ? err.message : String(err),
      });
      throw err;
    }
  }
}

export const taskStore = new GlobalTaskStore();
