<!-- ui/src/lib/components/DailyBarChart.svelte (Task T057) -->
<script lang="ts">
  import { untrack } from 'svelte';
  import { t, formatSignedSize } from '$lib/i18n.svelte';
  import { createLoader } from '$lib/cache';
  import UpdateStatus from './UpdateStatus.svelte';
  import {
    scanDriveHistory,
    getDailyDrilldown,
    revealInExplorer,
    type DailySnapshotDto,
    type DailyDrilldownDto,
    type ActorWriteDto,
    type FolderGrowthDto,
    type DriveHistoryDto
  } from '$lib/api';
  import QuotaRuleModal, { type QuotaRulePrefill } from './QuotaRuleModal.svelte';
  import {
    BarChart3,
    RefreshCw,
    FolderOpen,
    Cpu,
    PlusCircle
  } from 'lucide-svelte';

  interface Props {
    drive: string;
  }

  let { drive }: Props = $props();

  let daysBack = $state<7 | 14 | 30>(14);
  let snapshots = $state<DailySnapshotDto[]>([]);
  let selectedDate = $state<string | null>(null);
  let drilldown = $state<DailyDrilldownDto | null>(null);
  let isLoadingHistory = $state(false);
  let isLoadingDrilldown = $state(false);
  let ruleModalPrefill = $state<QuotaRulePrefill | null>(null);
  let ruleSavedToast = $state<string | null>(null);

  let maxAbsDelta = $derived.by(() => {
    let peak = 1024 * 1024; // 1 MiB floor so zero bars still align cleanly
    for (const s of snapshots) {
      const abs = Math.abs(s.net_change_bytes);
      if (abs > peak) peak = abs;
    }
    return peak;
  });

  let savedAt = $state<number | null>(null);
  let loadFailed = $state(false);
  // The day the user chose stays chosen when the newer history arrives.
  let pickedDay: string | null = null;
  let shownKey = '';

  const loadScan = createLoader<DriveHistoryDto>({
    show(data, at) {
      snapshots = data?.snapshots ?? [];
      savedAt = at;
      if (pickedDay && snapshots.some((s) => s.date === pickedDay)) {
        // Read the chosen day again: the scan may have changed it.
        if (data && !isLoadingDrilldown) selectDay(pickedDay);
        return;
      }
      selectedDate = data?.selected_date ?? null;
      drilldown = data?.latest_drilldown ?? null;
    },
    status(loading, failed) {
      isLoadingHistory = loading;
      loadFailed = failed;
    }
  });

  function loadHistory(days: 7 | 14 | 30 = daysBack, force = false) {
    daysBack = days;
    const target = drive;
    const key = `history.${target}.${days}`;
    if (key !== shownKey) {
      shownKey = key;
      pickedDay = null;
    }
    return loadScan(key, () => scanDriveHistory(target, days), { force });
  }

  async function selectDay(date: string) {
    pickedDay = date;
    selectedDate = date;
    isLoadingDrilldown = true;
    try {
      drilldown = await getDailyDrilldown(drive, date);
    } catch (err) {
      console.error('Failed to load daily drilldown:', err);
    } finally {
      isLoadingDrilldown = false;
    }
  }

  function openRuleFromActor(actor: ActorWriteDto) {
    const topFolder = drilldown?.folders?.[0]?.folder_path;
    ruleModalPrefill = {
      app_name: actor.process_name || actor.service_name || 'App Quota',
      process_name: actor.process_name !== 'Unknown' ? actor.process_name : '',
      service_name: actor.service_name,
      target_paths: topFolder ? [topFolder] : [],
      max_bytes: 5 * 1024 * 1024 * 1024,
      watermark_ratio: 0.85,
      retention_days: 7,
      enabled: true
    };
  }

  function openRuleFromFolder(folder: FolderGrowthDto) {
    const leaf = folder.folder_path.split('\\').filter(Boolean).pop() || 'Folder Quota';
    ruleModalPrefill = {
      app_name: leaf,
      target_paths: [folder.folder_path],
      max_bytes: 5 * 1024 * 1024 * 1024,
      watermark_ratio: 0.85,
      retention_days: 7,
      enabled: true
    };
  }

  $effect(() => {
    if (drive) {
      untrack(() => loadHistory(daysBack));
    }
  });
</script>

<div class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-4">
  <!-- Header & 7/14/30-day selector -->
  <div class="flex items-center justify-between flex-wrap gap-2">
    <div class="flex items-center gap-2">
      <BarChart3 class="h-5 w-5 text-accent" />
      <div>
        <h2 class="text-base font-bold text-primary">
          {t('Biến động dung lượng theo ngày')}
        </h2>
        <p class="text-[11px] text-secondary">
          {t('Bấm vào một cột ngày để xem thư mục phình to và tiến trình/dịch vụ ghi đĩa thực tế')}
        </p>
      </div>
    </div>

    <div class="flex items-center gap-2">
      <div class="inline-flex rounded-lg border border-border bg-subtle p-0.5 text-xs">
        {#each [7, 14, 30] as const as d}
          <button
            type="button"
            onclick={() => loadHistory(d)}
            class={`px-2.5 py-1 rounded-md font-medium transition-colors tabular-numbers ${
              daysBack === d
                ? 'bg-card text-accent shadow-sm'
                : 'text-secondary hover:text-primary'
            }`}
          >
            {d} {t('ngày')}
          </button>
        {/each}
      </div>

      <button
        type="button"
        onclick={() => loadHistory(daysBack, true)}
        disabled={isLoadingHistory}
        class="p-1.5 rounded-lg border border-border bg-card text-secondary hover:text-primary hover:bg-subtle disabled:opacity-50"
        title={t('Quét lại lịch sử')}
      >
        <RefreshCw class={`h-3.5 w-3.5 ${isLoadingHistory ? 'animate-spin' : ''}`} />
      </button>
    </div>
  </div>

  {#if ruleSavedToast}
    <div class="rounded-lg border border-accent/30 bg-accent/10 px-3 py-2 text-xs text-accent">
      {ruleSavedToast}
    </div>
  {/if}

  <!-- Interactive Signed Daily Bar Chart -->
  <UpdateStatus {savedAt} refreshing={isLoadingHistory} failed={loadFailed} />

  {#if snapshots.length === 0 && !isLoadingHistory}
    <div class="py-8 text-center text-xs text-secondary bg-subtle/50 rounded-lg border border-border/60">
      {t('Chưa có dữ liệu biến động theo ngày cho ổ này.')}
    </div>
  {:else}
    <div class="grid grid-flow-col auto-cols-fr gap-1.5 items-end h-44 pt-6 pb-2 px-2 rounded-lg bg-subtle/60 border border-border/60 overflow-x-auto">
      {#each snapshots as snap (snap.date)}
        {@const isSelected = selectedDate === snap.date}
        {@const ratio = Math.min(1, Math.abs(snap.net_change_bytes) / maxAbsDelta)}
        {@const barHeightPct = Math.max(8, Math.round(ratio * 100))}
        {@const isGrowth = snap.net_change_bytes > 0}
        {@const isShrink = snap.net_change_bytes < 0}
        {@const shortDate = snap.date.slice(5)}

        <button
          type="button"
          onclick={() => selectDay(snap.date)}
          class={`group flex flex-col items-center justify-end h-full rounded-md px-1 py-1 transition-colors cursor-pointer ${
            isSelected ? 'bg-accent/10 ring-1 ring-accent' : 'hover:bg-card/80'
          }`}
          title={`${snap.date}: ${formatSignedSize(snap.net_change_bytes)}`}
        >
          <span
            dir="ltr"
            class={`text-[10px] font-semibold tabular-numbers ltr-isolate mb-1 truncate max-w-full ${
              isGrowth ? 'text-danger' : isShrink ? 'text-accent' : 'text-disabled'
            }`}
          >
            {snap.net_change_bytes === 0 ? '0' : formatSignedSize(snap.net_change_bytes)}
          </span>

          <div class="w-full flex-1 flex items-end justify-center">
            <div
              class={`w-full max-w-[26px] rounded-t transition-all duration-300 ${
                isGrowth
                  ? 'bg-danger/85 group-hover:bg-danger'
                  : isShrink
                  ? 'bg-accent/85 group-hover:bg-accent'
                  : 'bg-border'
              }`}
              style={`height: ${barHeightPct}%`}
            ></div>
          </div>

          <span dir="ltr" class="mt-1.5 text-[10px] text-secondary tabular-numbers ltr-isolate">
            {shortDate}
          </span>
        </button>
      {/each}
    </div>
  {/if}

  <!-- Two-Column Per-Day Drilldown Tables -->
  {#if selectedDate}
    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4 pt-1">
      <!-- Column 1: Folders & Write Reason Codes -->
      <div class="rounded-lg border border-border bg-subtle/40 p-3.5 space-y-2.5">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-1.5 text-xs font-bold text-primary">
            <FolderOpen class="h-4 w-4 text-accent" />
            <span>{t('Thư mục & Nguyên nhân')} ({selectedDate})</span>
          </div>
          {#if isLoadingDrilldown}
            <span class="text-[11px] text-secondary">{t('Đang tải...')}</span>
          {/if}
        </div>

        {#if !drilldown || drilldown.folders.length === 0}
          <p class="text-xs text-secondary py-4 text-center">
            {t('Không ghi nhận thư mục biến động lớn trong ngày này.')}
          </p>
        {:else}
          <div class="space-y-1.5 max-h-60 overflow-y-auto">
            {#each drilldown.folders as row}
              <div class="flex items-center justify-between gap-2 rounded-md border border-border/70 bg-card px-2.5 py-2 text-xs">
                <div class="min-w-0 flex-1">
                  <button
                    type="button"
                    onclick={() => revealInExplorer(row.folder_path)}
                    dir="ltr"
                    class="block max-w-full truncate font-mono text-[11px] text-primary hover:text-accent ltr-isolate text-left"
                    title={row.folder_path}
                  >
                    {row.folder_path}
                  </button>
                  <div class="flex items-center gap-1.5 mt-0.5">
                    <span class="px-1.5 py-0.5 rounded bg-subtle border border-border text-[10px] text-secondary">
                      {row.reason_label}
                    </span>
                    <span class="text-[10px] text-disabled tabular-numbers">
                      {row.file_count} {t('tệp')}
                    </span>
                  </div>
                </div>

                <div class="flex items-center gap-2 shrink-0">
                  <span
                    dir="ltr"
                    class={`font-bold tabular-numbers ltr-isolate ${
                      row.total_delta_bytes >= 0 ? 'text-danger' : 'text-accent'
                    }`}
                  >
                    {formatSignedSize(row.total_delta_bytes)}
                  </span>
                  <button
                    type="button"
                    onclick={() => openRuleFromFolder(row)}
                    class="px-2 py-1 rounded border border-border bg-subtle hover:border-accent hover:text-accent text-[11px] font-medium"
                  >
                    {t('Tạo quy tắc')}
                  </button>
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <!-- Column 2: Attributed Processes & Windows Services -->
      <div class="rounded-lg border border-border bg-subtle/40 p-3.5 space-y-2.5">
        <div class="flex items-center gap-1.5 text-xs font-bold text-primary">
          <Cpu class="h-4 w-4 text-accent" />
          <span>{t('Ứng dụng & Windows Service ghi đĩa')} ({selectedDate})</span>
        </div>

        {#if !drilldown || drilldown.actors.length === 0}
          <p class="text-xs text-secondary py-4 text-center">
            {t('Chưa có tác nhân ghi đĩa đáng kể trong ngày này.')}
          </p>
        {:else}
          <div class="space-y-1.5 max-h-60 overflow-y-auto">
            {#each drilldown.actors as actor}
              <div class="flex items-center justify-between gap-2 rounded-md border border-border/70 bg-card px-2.5 py-2 text-xs">
                <div class="min-w-0 flex-1">
                  <div dir="ltr" class="font-semibold text-primary truncate ltr-isolate">
                    {actor.display_actor}
                  </div>
                  <div class="text-[10px] text-disabled tabular-numbers">
                    {actor.event_count} {t('đợt ghi')}
                  </div>
                </div>

                <div class="flex items-center gap-2 shrink-0">
                  <span
                    dir="ltr"
                    class={`font-bold tabular-numbers ltr-isolate ${
                      actor.total_delta_bytes >= 0 ? 'text-danger' : 'text-accent'
                    }`}
                  >
                    {formatSignedSize(actor.total_delta_bytes)}
                  </span>
                  <button
                    type="button"
                    onclick={() => openRuleFromActor(actor)}
                    class="inline-flex items-center gap-1 px-2.5 py-1 rounded bg-accent/10 hover:bg-accent hover:text-white text-accent text-[11px] font-semibold transition-colors"
                  >
                    <PlusCircle class="h-3 w-3" />
                    {t('Tạo quy tắc')}
                  </button>
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

{#if ruleModalPrefill}
  <QuotaRuleModal
    initialRule={ruleModalPrefill}
    onClose={() => (ruleModalPrefill = null)}
    onSaved={(saved) => {
      ruleModalPrefill = null;
      ruleSavedToast = t('Đã lưu quy tắc hạn mức cho {app}', { app: saved.app_name });
      setTimeout(() => {
        ruleSavedToast = null;
      }, 4000);
    }}
  />
{/if}
