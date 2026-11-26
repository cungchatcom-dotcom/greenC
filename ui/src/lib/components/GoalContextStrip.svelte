<!-- ui/src/lib/components/GoalContextStrip.svelte: Objective-driven banner (hero on Overview, compact strip on action/report tabs) -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import { listDriveQuotas, type DriveQuotaStatusDto } from '$lib/api';
  import { peek, refresh } from '$lib/cache';
  import { Target, CheckCircle2, AlertTriangle, Sparkles, SlidersHorizontal } from 'lucide-svelte';

  interface Props {
    mode?: 'hero' | 'strip';
    refreshKey?: number;
    onActionCleanup?: () => void;
    onConfigureGoal?: () => void;
  }

  let { mode = 'strip', refreshKey = 0, onActionCleanup, onConfigureGoal }: Props = $props();

  const pick = (list: DriveQuotaStatusDto[]) => list.find((q) => q.enabled) ?? list[0] ?? null;
  const last = peek<DriveQuotaStatusDto[]>('quotas');
  let primaryGoal = $state<DriveQuotaStatusDto | null>(last ? pick(last.data) : null);
  let loaded = $state(last !== null);

  async function loadGoal() {
    try {
      primaryGoal = pick(await refresh('quotas', listDriveQuotas, { force: true }));
    } catch {
      primaryGoal = null;
    } finally {
      loaded = true;
    }
  }

  onMount(() => {
    loadGoal();
  });

  $effect(() => {
    // Re-load whenever parent bumps refreshKey
    void refreshKey;
    loadGoal();
  });

  let usedPct = $derived(primaryGoal ? Math.min(100, Math.round(primaryGoal.usage_ratio * 100)) : 0);
  let maxPct = $derived(primaryGoal ? Math.min(100, Math.round(primaryGoal.clean_ratio * 100)) : 80);
  let targetPct = $derived(primaryGoal ? Math.min(100, Math.round(primaryGoal.target_ratio * 100)) : 70);
  let isOverGoal = $derived(primaryGoal ? primaryGoal.usage_ratio >= primaryGoal.clean_ratio : false);
  let isNearGoal = $derived(
    primaryGoal ? !isOverGoal && primaryGoal.usage_ratio >= primaryGoal.warn_ratio : false
  );
  let bytesOver = $derived(primaryGoal ? Math.max(0, primaryGoal.bytes_to_reclaim) : 0);
</script>

{#if loaded}
  {#if mode === 'hero'}
    {#if primaryGoal}
      <section
        class={`rounded-2xl border p-5 shadow-sm transition-colors ${
          isOverGoal
            ? 'border-danger/35 bg-danger/5'
            : isNearGoal
            ? 'border-warning/35 bg-warning/5'
            : 'border-accent/30 bg-accent/5'
        }`}
      >
        <div class="flex flex-col md:flex-row md:items-center justify-between gap-4">
          <div class="space-y-1.5 flex-1 min-w-0">
            <div class="flex items-center gap-2 flex-wrap">
              <span
                class={`inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full text-xs font-semibold ${
                  isOverGoal
                    ? 'bg-danger/15 text-danger'
                    : isNearGoal
                    ? 'bg-warning/15 text-warning'
                    : 'bg-accent/15 text-accent'
                }`}
              >
                {#if isOverGoal || isNearGoal}
                  <AlertTriangle class="h-3.5 w-3.5" />
                {:else}
                  <CheckCircle2 class="h-3.5 w-3.5" />
                {/if}
                {isOverGoal
                  ? t('Vượt mục tiêu')
                  : isNearGoal
                  ? t('Gần chạm mục tiêu')
                  : t('Đạt mục tiêu')}
              </span>
              <span class="text-xs font-semibold text-secondary">
                {t('Mục tiêu')}:
                <strong dir="ltr" class="font-mono text-primary ltr-isolate">{primaryGoal.drive_root}</strong>
                &lt; {maxPct}%
              </span>
            </div>

            <div class="text-base font-bold text-primary">
              {#if isOverGoal}
                {t('Cần giải phóng thêm {size} để đưa ổ về mức xanh ({pct}%)', {
                  size: formatSize(bytesOver),
                  pct: targetPct
                })}
              {:else if isNearGoal}
                {t('Ổ đĩa đang ở {pct}%, sắp chạm ngưỡng mục tiêu {max}%', {
                  pct: usedPct,
                  max: maxPct
                })}
              {:else}
                {t('Ổ đĩa đang xanh ({pct}% / mục tiêu < {max}%) — GreenC đang tự bảo vệ mỗi ngày', {
                  pct: usedPct,
                  max: maxPct
                })}
              {/if}
            </div>

            <!-- Goal progress bar with target threshold marker -->
            <div class="pt-1 space-y-1">
              <div class="relative h-2.5 rounded-full bg-border/80 overflow-hidden">
                <div
                  class={`h-full rounded-full transition-all duration-300 ${
                    isOverGoal ? 'bg-danger' : isNearGoal ? 'bg-warning' : 'bg-accent'
                  }`}
                  style="width: {usedPct}%"
                ></div>
                <div
                  class="absolute top-0 bottom-0 w-0.5 bg-primary"
                  style="left: {maxPct}%"
                  title="{t('Mục tiêu')}: {maxPct}%"
                ></div>
              </div>
              <div class="flex items-center justify-between text-[11px] text-secondary tabular-numbers">
                <span>
                  {t('Đã dùng')}: {formatSize(primaryGoal.used_bytes)} / {formatSize(primaryGoal.effective_limit_bytes)} ({usedPct}%)
                </span>
                <span>
                  {primaryGoal.auto_clean
                    ? t('Tự động dọn về {pct}%', { pct: targetPct })
                    : t('Chỉ cảnh báo')}
                </span>
              </div>
            </div>
          </div>

          <div class="flex items-center gap-2 shrink-0">
            {#if (isOverGoal || isNearGoal) && onActionCleanup}
              <button
                type="button"
                onclick={onActionCleanup}
                class="inline-flex items-center gap-1.5 px-4 py-2.5 rounded-xl bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm"
              >
                <Sparkles class="h-4 w-4" />
                {t('Đưa về mức xanh ngay')}
              </button>
            {/if}
            {#if onConfigureGoal}
              <button
                type="button"
                onclick={onConfigureGoal}
                class="inline-flex items-center gap-1.5 px-3 py-2.5 rounded-xl border border-border bg-card hover:bg-subtle text-xs font-semibold text-primary"
              >
                <SlidersHorizontal class="h-3.5 w-3.5" />
                {t('Chỉnh mục tiêu')}
              </button>
            {/if}
          </div>
        </div>
      </section>
    {:else}
      <section class="rounded-2xl border border-dashed border-accent/40 bg-accent/5 p-4 flex flex-col sm:flex-row sm:items-center justify-between gap-3">
        <div class="flex items-center gap-3">
          <div class="h-9 w-9 rounded-xl bg-accent/15 text-accent flex items-center justify-center shrink-0">
            <Target class="h-5 w-5" />
          </div>
          <div>
            <div class="text-sm font-bold text-primary">{t('Đặt mục tiêu giữ ổ đĩa luôn xanh')}</div>
            <div class="text-xs text-secondary">
              {t('Đặt hạn mức một lần (vd: < 80%), mọi hành động dọn dẹp và báo cáo sẽ tự động bám theo mục tiêu này.')}
            </div>
          </div>
        </div>
        {#if onConfigureGoal}
          <button
            type="button"
            onclick={onConfigureGoal}
            class="inline-flex items-center gap-1.5 px-4 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm shrink-0"
          >
            <Target class="h-3.5 w-3.5" />
            {t('Đặt mục tiêu ngay')}
          </button>
        {/if}
      </section>
    {/if}
  {:else if primaryGoal}
    <!-- Compact Goal Context Strip for Cleanup / Relocate / Compact / Vault -->
    <div
      class={`mb-4 rounded-xl border px-4 py-2.5 text-xs flex items-center justify-between gap-3 flex-wrap ${
        isOverGoal
          ? 'border-danger/30 bg-danger/5'
          : isNearGoal
          ? 'border-warning/30 bg-warning/5'
          : 'border-accent/25 bg-accent/5'
      }`}
    >
      <div class="flex items-center gap-2.5 min-w-0 flex-wrap">
        <Target class={`h-4 w-4 shrink-0 ${isOverGoal ? 'text-danger' : 'text-accent'}`} />
        <span class="font-semibold text-primary">
          {t('Mục tiêu')}:
          <span dir="ltr" class="font-mono ltr-isolate">{primaryGoal.drive_root}</span> &lt; {maxPct}%
        </span>
        <span class="text-secondary tabular-numbers">
          · {t('Hiện tại')}: <strong class="text-primary">{usedPct}%</strong>
          {#if isOverGoal}
            · <strong class="text-danger">{t('Cần giải phóng thêm {size}', { size: formatSize(bytesOver) })}</strong>
          {:else}
            · <strong class="text-accent">{t('Đạt mục tiêu')}</strong>
          {/if}
        </span>
      </div>
      {#if onConfigureGoal}
        <button
          type="button"
          onclick={onConfigureGoal}
          class="text-[11px] font-semibold text-accent hover:underline shrink-0"
        >
          {t('Chỉnh mục tiêu')}
        </button>
      {/if}
    </div>
  {/if}
{/if}
