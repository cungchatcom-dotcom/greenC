<!-- ui/src/lib/components/SavingsScorecard.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import { getSavingsStats, type SavingsPeriod, type SavingsStatsDto } from '$lib/api';
  import { onFilesChanged } from '$lib/busy.svelte';
  import { BarChart3, Sparkles, TrendingUp, Flame, Calendar } from 'lucide-svelte';

  interface Props {
    refreshKey?: number;
  }

  let { refreshKey = 0 }: Props = $props();

  let period = $state<SavingsPeriod>('day');
  let stats = $state<SavingsStatsDto | null>(null);
  let loading = $state(false);
  let hoveredIdx = $state<number | null>(null);

  const periods: ReadonlyArray<{ id: SavingsPeriod; labelKey: string }> = [
    { id: 'day', labelKey: 'Ngày' },
    { id: 'week', labelKey: 'Tuần' },
    { id: 'month', labelKey: 'Tháng' },
    { id: 'year', labelKey: 'Năm' },
  ];

  async function loadStats(p: SavingsPeriod = period) {
    loading = true;
    try {
      stats = await getSavingsStats(p);
    } catch (err) {
      console.error('Failed to load savings stats:', err);
    } finally {
      loading = false;
    }
  }

  function selectPeriod(p: SavingsPeriod) {
    period = p;
    hoveredIdx = null;
    loadStats(p);
  }

  onFilesChanged(() => loadStats(period));

  $effect(() => {
    void refreshKey;
    loadStats(period);
  });

  onMount(() => {
    loadStats(period);
  });

  let maxBucketBytes = $derived(
    stats && stats.buckets.length > 0
      ? Math.max(1, ...stats.buckets.map((b) => b.bytes_freed))
      : 1
  );

  let periodTitle = $derived.by(() => {
    switch (period) {
      case 'day':
        return t('14 ngày gần đây');
      case 'week':
        return t('8 tuần gần đây');
      case 'month':
        return t('12 tháng gần đây');
      case 'year':
        return t('Theo năm');
    }
  });
</script>

<section class="rounded-2xl border border-border bg-card p-5 shadow-sm space-y-5">
  <!-- Header + Period Switcher -->
  <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-3">
    <div class="flex items-center gap-2.5">
      <div class="h-9 w-9 rounded-xl bg-accent/15 text-accent flex items-center justify-center shrink-0">
        <BarChart3 class="h-5 w-5" />
      </div>
      <div>
        <h2 class="text-base font-bold text-primary">{t('Hiệu quả của GreenC')}</h2>
        <p class="text-xs text-secondary">
          {t('Dung lượng đã giải phóng thực tế theo thời gian và các ứng dụng sinh rác nhiều nhất')}
        </p>
      </div>
    </div>

    <div class="inline-flex items-center gap-1 p-1 rounded-xl border border-border bg-subtle self-start">
      {#each periods as item}
        <button
          type="button"
          onclick={() => selectPeriod(item.id)}
          class={`px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors ${
            period === item.id
              ? 'bg-card text-accent shadow-sm'
              : 'text-secondary hover:text-primary'
          }`}
        >
          {t(item.labelKey)}
        </button>
      {/each}
    </div>
  </div>

  <!-- 3 Concise KPI Tiles -->
  <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
    <div class="rounded-xl border border-border/80 bg-subtle/50 p-4">
      <div class="flex items-center justify-between text-xs text-secondary">
        <span>{t('Hôm nay đã giải phóng')}</span>
        <Sparkles class="h-4 w-4 text-accent" />
      </div>
      <div class="mt-2 text-2xl font-bold text-accent tabular-numbers">
        {formatSize(stats?.today_freed_bytes ?? 0)}
      </div>
    </div>

    <div class="rounded-xl border border-border/80 bg-subtle/50 p-4">
      <div class="flex items-center justify-between text-xs text-secondary">
        <span>{t('Luỹ kế ({period})', { period: periodTitle })}</span>
        <Calendar class="h-4 w-4 text-blue-500" />
      </div>
      <div class="mt-2 text-2xl font-bold text-primary tabular-numbers">
        {formatSize(stats?.period_freed_bytes ?? 0)}
      </div>
    </div>

    <div class="rounded-xl border border-border/80 bg-subtle/50 p-4">
      <div class="flex items-center justify-between text-xs text-secondary">
        <span>{t('Tổng đã cứu từ trước tới nay')}</span>
        <TrendingUp class="h-4 w-4 text-emerald-500" />
      </div>
      <div class="mt-2 text-2xl font-bold text-emerald-600 dark:text-emerald-400 tabular-numbers">
        {formatSize(stats?.all_time_freed_bytes ?? 0)}
      </div>
    </div>
  </div>

  <!-- Chart + Top Junk Apps -->
  {#if !stats || stats.all_time_freed_bytes === 0}
    <div class="rounded-xl border border-dashed border-border bg-subtle/40 p-6 text-center space-y-1.5">
      <p class="text-sm font-semibold text-primary">
        {loading ? t('Đang tải thống kê...') : t('Chưa có lịch sử giải phóng dung lượng')}
      </p>
      <p class="text-xs text-secondary">
        {t('Bấm "Làm xanh ổ C: ngay" ở trên để GreenC bắt đầu dọn dẹp và ghi nhận kết quả theo ngày, tuần, tháng, năm.')}
      </p>
    </div>
  {:else}
    <div class="grid grid-cols-1 lg:grid-cols-5 gap-5">
      <!-- Left: Periodic Bar Chart (3 cols) -->
      <div class="lg:col-span-3 rounded-xl border border-border/70 bg-subtle/30 p-4 flex flex-col justify-between gap-3">
        <div class="flex items-center justify-between text-xs">
          <span class="font-semibold text-primary">{periodTitle}</span>
          {#if hoveredIdx !== null && stats.buckets[hoveredIdx]}
            <span class="font-mono font-semibold text-accent tabular-numbers">
              {stats.buckets[hoveredIdx].label}: {formatSize(stats.buckets[hoveredIdx].bytes_freed)}
            </span>
          {:else}
            <span class="text-secondary">
              {t('{count} lần dọn dẹp', { count: stats.total_events })}
            </span>
          {/if}
        </div>

        <div class="pt-3 space-y-1.5">
          <div class="relative h-40 flex items-end gap-1.5 border-b border-zinc-300 dark:border-zinc-700 pb-0.5">
            <!-- Subtle horizontal guide lines -->
            <div class="pointer-events-none absolute inset-x-0 top-0 border-t border-dashed border-zinc-300/70 dark:border-zinc-700/70"></div>
            <div class="pointer-events-none absolute inset-x-0 top-1/2 border-t border-dashed border-zinc-300/70 dark:border-zinc-700/70"></div>

            {#each stats.buckets as bucket, idx}
              {@const barPx = bucket.bytes_freed > 0
                ? Math.max(10, Math.round((bucket.bytes_freed / maxBucketBytes) * 116))
                : 3}
              <button
                type="button"
                onmouseenter={() => (hoveredIdx = idx)}
                onmouseleave={() => (hoveredIdx = null)}
                onfocus={() => (hoveredIdx = idx)}
                class="relative z-10 flex-1 h-full flex flex-col items-center justify-end group outline-none"
                title={`${bucket.label}: ${formatSize(bucket.bytes_freed)}`}
                aria-label={`${bucket.label}: ${formatSize(bucket.bytes_freed)}`}
              >
                {#if bucket.bytes_freed > 0}
                  <span class="text-[10px] font-mono font-semibold text-accent mb-1 truncate max-w-full leading-tight">
                    {formatSize(bucket.bytes_freed)}
                  </span>
                {/if}
                <div
                  class={`w-full max-w-[36px] min-w-[6px] rounded-t-md transition-all duration-200 ${
                    bucket.bytes_freed > 0
                      ? 'bg-accent group-hover:bg-accent-hover shadow-sm'
                      : 'bg-zinc-300/60 dark:bg-zinc-700/60 group-hover:bg-zinc-400 dark:group-hover:bg-zinc-600'
                  }`}
                  style:height={`${barPx}px`}
                ></div>
              </button>
            {/each}
          </div>

          <div class="flex items-center gap-1.5">
            {#each stats.buckets as bucket, idx}
              <span
                class={`flex-1 text-center text-[10px] truncate tabular-numbers ${
                  hoveredIdx === idx ? 'text-accent font-semibold' : 'text-secondary'
                }`}
              >
                {bucket.label}
              </span>
            {/each}
          </div>
        </div>
      </div>

      <!-- Right: Top Junk-Generating Apps (2 cols) -->
      <div class="lg:col-span-2 rounded-xl border border-border/70 bg-subtle/30 p-4 space-y-3">
        <div class="flex items-center gap-1.5 text-xs font-semibold text-primary">
          <Flame class="h-4 w-4 text-amber-500" />
          <span>{t('Sinh nhiều rác nhất')}</span>
        </div>

        {#if stats.top_apps.length === 0}
          <p class="text-xs text-secondary py-6 text-center">
            {t('Không có dữ liệu trong khoảng thời gian này.')}
          </p>
        {:else}
          <div class="space-y-2.5">
            {#each stats.top_apps as app, rank}
              <div class="space-y-1">
                <div class="flex items-center justify-between text-xs gap-2">
                  <span class="font-medium text-primary truncate">
                    <span class="text-disabled mr-1">{rank + 1}.</span>
                    {app.app_name}
                  </span>
                  <span class="font-mono font-semibold text-primary shrink-0 tabular-numbers">
                    {formatSize(app.bytes_freed)}
                    <span class="text-[10px] text-secondary font-normal">({app.share_pct}%)</span>
                  </span>
                </div>
                <div class="w-full h-1.5 rounded-full bg-zinc-200 dark:bg-zinc-700/60 overflow-hidden">
                  <div
                    class="h-full rounded-full bg-accent transition-all duration-300"
                    style={`width: ${Math.max(4, Math.min(100, app.share_pct))}%`}
                  ></div>
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  {/if}
</section>
