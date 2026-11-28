<!-- ui/src/lib/components/TaskProgressOverlay.svelte -->
<script lang="ts">
  import { taskStore } from '$lib/tasks.svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import { RefreshCw, Minimize2, X, CheckCircle2, AlertTriangle, Info } from 'lucide-svelte';

  function activeStepIndex(
    phase: string,
    steps?: ReadonlyArray<{ key: string; label: string }>
  ): number {
    if (!steps || steps.length === 0) return -1;
    const idx = steps.findIndex((s) => s.key === phase);
    if (idx >= 0) return idx;
    if (phase === 'linking') return steps.findIndex((s) => s.key === 'moving');
    if (phase === 'health_check' || phase === 'rolling_back') {
      return steps.findIndex((s) => s.key === 'verifying');
    }
    return 0;
  }

  function compactPath(raw: string): string {
    if (!raw) return '';
    const norm = raw.replaceAll('\\', '/');
    const parts = norm.split('/').filter(Boolean);
    if (parts.length <= 2) return parts.join('/');
    return `…/${parts.slice(-2).join('/')}`;
  }
</script>

<!-- Active Non-Minimized Progress Modal -->
{#if taskStore.active && !taskStore.active.minimized}
  {@const task = taskStore.active}
  {@const stepIdx = activeStepIndex(task.phase, task.steps)}
  <div class="fixed inset-0 z-50 bg-black/45 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-2xl border border-border bg-card p-6 shadow-2xl max-w-md w-full space-y-4">
      <div class="flex items-center justify-between gap-3">
        <h3 class="font-bold text-base text-primary flex items-center gap-2 min-w-0">
          <RefreshCw class="h-4 w-4 text-accent animate-spin shrink-0" />
          <span class="truncate">{t(task.title)}</span>
        </h3>
      </div>

      {#if task.steps && task.steps.length > 0}
        <div class="grid grid-cols-2 sm:grid-cols-4 gap-1.5 text-[11px]">
          {#each task.steps as step, idx}
            <div
              class={`px-2 py-1 rounded-md text-center font-medium truncate ${
                idx < stepIdx
                  ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300'
                  : idx === stepIdx
                    ? 'bg-accent text-white'
                    : 'bg-subtle text-secondary'
              }`}
            >
              {t(step.label)}
            </div>
          {/each}
        </div>
      {/if}

      <div class="space-y-1.5">
        <div class="w-full bg-subtle rounded-full h-2.5 overflow-hidden">
          <div
            class="bg-accent h-2.5 rounded-full transition-all duration-150"
            style={`width: ${task.percent}%`}
          ></div>
        </div>
        <div class="flex items-center justify-between text-xs text-secondary font-mono tabular-numbers">
          <span>{task.percent}%</span>
          {#if task.totalBytes > 0}
            <span>{formatSize(task.doneBytes)} / {formatSize(task.totalBytes)}</span>
          {:else if task.doneFiles > 0}
            <span>{t('{count} tệp', { count: task.doneFiles })}</span>
          {/if}
        </div>
      </div>

      <!-- Compact single-line current file indicator (read-only transparency) -->
      {#if task.currentFile}
        <div
          class="text-[11px] text-secondary font-mono bg-subtle/80 px-2.5 py-1.5 rounded-lg truncate"
          title={task.currentFile}
        >
          {compactPath(task.currentFile)}
        </div>
      {/if}

      <!-- Background mode + Cancel -->
      <div class="flex items-center justify-between gap-2 pt-2 border-t border-border/60">
        <button
          type="button"
          onclick={() => taskStore.minimize()}
          class="inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg border border-border bg-subtle hover:bg-card text-xs font-semibold text-primary transition-colors"
        >
          <Minimize2 class="h-3.5 w-3.5 text-accent" />
          {t('Ẩn chạy ngầm')}
        </button>

        <button
          type="button"
          onclick={() => taskStore.cancel()}
          class="px-3.5 py-2 rounded-lg text-xs font-medium text-rose-600 dark:text-rose-400 hover:bg-rose-500/10 transition-colors"
        >
          {t('Hủy')}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Global Floating Toast Notification (replaces browser alert()) -->
{#if taskStore.toast}
  <div class="fixed bottom-5 right-5 z-50 max-w-sm w-full">
    <div
      class={`rounded-xl border p-3.5 shadow-xl flex items-start gap-2.5 text-xs ${
        taskStore.toast.kind === 'ok'
          ? 'border-emerald-500/35 bg-card text-primary'
          : taskStore.toast.kind === 'err'
            ? 'border-rose-500/40 bg-card text-primary'
            : 'border-border bg-card text-primary'
      }`}
    >
      {#if taskStore.toast.kind === 'ok'}
        <CheckCircle2 class="h-4 w-4 text-emerald-500 shrink-0 mt-0.5" />
      {:else if taskStore.toast.kind === 'err'}
        <AlertTriangle class="h-4 w-4 text-rose-500 shrink-0 mt-0.5" />
      {:else}
        <Info class="h-4 w-4 text-accent shrink-0 mt-0.5" />
      {/if}
      <div class="flex-1 min-w-0">
        <div class="font-semibold text-primary">{taskStore.toast.title}</div>
        {#if taskStore.toast.detail}
          <div class="text-secondary mt-0.5 break-words">{taskStore.toast.detail}</div>
        {/if}
      </div>
      <button
        type="button"
        onclick={() => taskStore.dismissToast()}
        class="text-secondary hover:text-primary p-0.5"
        aria-label={t('Đóng')}
      >
        <X class="h-3.5 w-3.5" />
      </button>
    </div>
  </div>
{/if}
