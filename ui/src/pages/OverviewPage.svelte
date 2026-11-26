<!-- ui/src/pages/OverviewPage.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    getOverviewSummary,
    scanSpaceLens,
    scanCleanupCandidates,
    listenTaskProgress,
    getBootstrapState,
    diagnosticsScreen,
    type OverviewSummary,
    type SpaceNode,
    type TaskProgressEvent,
    type DriveInfo,
    type CleanupCandidateDto,
    type ReclaimableItemDto
  } from '$lib/api';
  import { createLoader, trimTree, pendingKeys } from '$lib/cache';
  import { createScreenTimer } from '$lib/screen-timer';
  import { onFilesChanged } from '$lib/busy.svelte';
  import { DEFAULT_CLEANUP_PROFILES, cleanupKey } from '$lib/scans';
  import UpdateStatus from '$lib/components/UpdateStatus.svelte';
  import SpaceTreemap from '$lib/components/SpaceTreemap.svelte';
  import DailyBarChart from '$lib/components/DailyBarChart.svelte';
  import ExpandableTableCard from '$lib/components/ExpandableTableCard.svelte';
  import GoalContextStrip from '$lib/components/GoalContextStrip.svelte';
  import {
    HardDrive,
    AlertCircle,
    CheckCircle2,
    TrendingUp,
    TrendingDown,
    Sparkles,
    RefreshCw
  } from 'lucide-svelte';

  interface Props {
    goalRefreshKey?: number;
    onNavigateCleanup?: () => void;
    onNavigateSettings?: () => void;
  }

  let { goalRefreshKey = 0, onNavigateCleanup, onNavigateSettings }: Props = $props();

  let selectedDrive = $state('C');
  let availableDrives = $state<DriveInfo[]>([]);
  let summary = $state<OverviewSummary | null>(null);
  let spaceNode = $state.raw<SpaceNode | null>(null);
  let spaceSavedAt = $state<number | null>(null);
  let spaceFailed = $state(false);
  let reclaimable = $state.raw<ReclaimableItemDto[]>([]);
  let reclaimableSavedAt = $state<number | null>(null);
  let isScanningReclaimable = $state(false);
  let reclaimableFailed = $state(false);

  // How old a result may get before this screen scans again by itself.
  const SPACE_MAX_AGE_MS = 6 * 60 * 60 * 1000;
  const CLEANUP_MAX_AGE_MS = 30 * 60 * 1000;
  let isLoading = $state(false);
  let isScanningSpace = $state(false);
  let progressState = $state<TaskProgressEvent | null>(null);

  async function initDrives() {
    try {
      const bootstrap = await getBootstrapState();
      if (bootstrap?.drives && bootstrap.drives.length > 0) {
        availableDrives = bootstrap.drives;
        if (!availableDrives.some(d => d.drive_letter === selectedDrive)) {
          selectedDrive = bootstrap.recommended_drive || availableDrives[0].drive_letter;
        }
      }
    } catch (err) {
      console.error('Failed to load bootstrap drives:', err);
    }
  }

  function onDriveChanged() {
    progressState = null;
    loadOverview();
    loadSpace();
  }

  const screenTimer = createScreenTimer((ms, cached) => {
    void diagnosticsScreen('overview', ms, cached);
  });

  const loadSummary = createLoader<OverviewSummary>({
    show: (data, _at, fresh) => {
      summary = data;
      if (data !== null) screenTimer.shown(fresh);
    },
    status: (loading) => (isLoading = loading)
  });

  const loadSpaceMap = createLoader<SpaceNode>({
    show(data, at) {
      spaceNode = data;
      spaceSavedAt = at;
    },
    status(loading, failed) {
      isScanningSpace = loading;
      spaceFailed = failed;
      if (!loading) progressState = null;
    }
  });

  const loadCleanupScan = createLoader<CleanupCandidateDto[]>({
    show(data, at) {
      reclaimable = (data ?? [])
        .filter((c) => c.reclaimable_bytes > 0)
        .map((c) => ({
          app_name: c.app_name,
          path: c.path,
          size_bytes: c.reclaimable_bytes,
          category: c.category,
          recommendation: t('Dọn dẹp')
        }));
      reclaimableSavedAt = at;
    },
    status(loading, failed) {
      isScanningReclaimable = loading;
      reclaimableFailed = failed;
    }
  });

  // A window hidden in the tray only shows what it has; it scans when somebody looks.
  const isHidden = () => document.visibilityState === 'hidden';

  function loadOverview() {
    const drive = selectedDrive;
    return loadSummary(`overview.${drive}`, () => getOverviewSummary(drive), { force: true });
  }

  function loadSpace(force = false) {
    const drive = selectedDrive;
    return loadSpaceMap(`space.${drive}`, () => scanSpaceLens(drive), {
      force,
      maxAgeMs: SPACE_MAX_AGE_MS,
      // A walk of another drive is still running: do not start a second one by itself.
      cacheOnly: !force && (isHidden() || pendingKeys('space.').some((k) => k !== `space.${drive}`)),
      compact: (tree) => trimTree(tree)
    });
  }

  function loadReclaimable() {
    return loadCleanupScan(
      cleanupKey(DEFAULT_CLEANUP_PROFILES),
      () => scanCleanupCandidates(DEFAULT_CLEANUP_PROFILES),
      { maxAgeMs: CLEANUP_MAX_AGE_MS, cacheOnly: isHidden() }
    );
  }

  function triggerSpaceScan() {
    return loadSpace(true);
  }

  onFilesChanged(() => {
    loadOverview();
    loadReclaimable();
  });

  function onVisibilityChange() {
    if (isHidden()) return;
    loadSpace();
    loadReclaimable();
  }

  onMount(() => {
    initDrives().then(() => {
      loadOverview();
      loadSpace();
      loadReclaimable();
    });
    document.addEventListener('visibilitychange', onVisibilityChange);

    // Listen to real-time progress events <= 100ms
    const unlistenPromise = listenTaskProgress((event) => {
      if (event.kind === 'space_lens' && event.task_id.startsWith(`space-lens-${selectedDrive}-`)) {
        progressState = event;
      }
    });

    return () => {
      document.removeEventListener('visibilitychange', onVisibilityChange);
      unlistenPromise.then((unlisten) => unlisten());
    };
  });

  // Health assessment
  let usageRatio = $derived(summary ? summary.usage_ratio : 0);
  let usagePercent = $derived(Math.round(usageRatio * 100));
  let isCritical = $derived(
    summary?.health_status === 'critical' ||
    usagePercent >= 90 ||
    (summary?.days_until_full !== null && summary?.days_until_full !== undefined && summary.days_until_full < 14)
  );
  let isWarning = $derived(
    summary?.health_status === 'warning' ||
    (usagePercent >= 80 && !isCritical) ||
    (summary?.days_until_full !== null && summary?.days_until_full !== undefined && summary.days_until_full <= 30 && !isCritical)
  );

  // Burn-Rate Velocity
  let burnRateStr = $derived.by(() => {
    if (!summary || summary.burn_rate_bps === 0) return '0 B / 24h';
    const absStr = formatSize(Math.abs(summary.burn_rate_bps));
    return summary.burn_rate_bps > 0 ? `+${absStr} / 24h` : `-${absStr} / 24h`;
  });
</script>

<div class="flex flex-col gap-6 p-6 max-w-7xl mx-auto overflow-y-auto">
  <!-- Objective-Driven Hero Card -->
  <GoalContextStrip
    mode="hero"
    refreshKey={goalRefreshKey}
    onActionCleanup={onNavigateCleanup}
    onConfigureGoal={onNavigateSettings}
  />

  <!-- Page Header -->
  <div class="flex items-center justify-between">
    <div>
      <h1 class="text-2xl font-bold text-primary">{t('Tổng quan')}</h1>
      <p class="text-xs text-secondary mt-0.5">
        {t('Theo dõi sức khỏe ổ đĩa hệ thống, tốc độ phình dung lượng và bản đồ phân bổ')}
      </p>
    </div>

    <div class="flex items-center gap-2">
      <!-- Dynamic Drive Selector Dropdown -->
      <select
        bind:value={selectedDrive}
        onchange={onDriveChanged}
        class="px-3 py-1.5 rounded-lg border border-border bg-card text-xs font-bold text-primary focus:border-accent focus:ring-1 focus:ring-accent outline-none shadow-sm cursor-pointer"
      >
        {#if availableDrives.length > 0}
          {#each availableDrives as drive}
            <option value={drive.drive_letter}>
              {t('Ổ đĩa')} {drive.drive_letter}: {drive.label ? `(${drive.label})` : ''}
            </option>
          {/each}
        {:else}
          <option value="C">{t('Ổ đĩa')} C:</option>
        {/if}
      </select>

      <button
        type="button"
        onclick={loadOverview}
        disabled={isLoading}
        class="p-2 rounded-lg border border-border bg-card text-secondary hover:text-primary hover:bg-subtle transition-colors disabled:opacity-50"
        title={t('Làm mới')}
      >
        <RefreshCw class={`h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
      </button>

      {#if onNavigateCleanup}
        <button
          type="button"
          onclick={onNavigateCleanup}
          class="px-4 py-2 rounded-lg font-medium text-sm bg-accent hover:bg-accent-hover text-white shadow-sm transition-colors flex items-center gap-1.5"
        >
          <Sparkles class="h-4 w-4" />
          {t('Dọn dẹp')}
        </button>
      {/if}
    </div>
  </div>

  <!-- Section 1: Drive Usage Card -->
  <div class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-4">
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-2.5">
        <HardDrive class="h-5 w-5 text-accent" />
        <span class="font-bold text-base text-primary">{t('Ổ đĩa')} {selectedDrive}:</span>
      </div>

      <!-- Capacity Stats -->
      {#if summary}
        <div class="text-xs text-secondary tabular-numbers">
          {t('Còn trống {free} / {total}', {
            free: formatSize(summary.free_bytes),
            total: formatSize(summary.total_bytes)
          })}
          <span class="ml-1 font-semibold text-primary">({usagePercent}%)</span>
        </div>
      {/if}
    </div>

    <!-- Progress Bar (Turns Danger Red >= 90%) -->
    <div class="w-full bg-border/40 h-3 rounded-full overflow-hidden">
      <div
        class={`h-full transition-all duration-500 rounded-full ${
          isCritical ? 'bg-danger' : isWarning ? 'bg-amber-500' : 'bg-accent'
        }`}
        style="width: {usagePercent}%"
      ></div>
    </div>

    <!-- Health Status & Forecast Badges Row -->
    {#if summary}
      <div class="flex items-center justify-between pt-1 text-xs flex-wrap gap-2">
        <div class="flex items-center gap-3">
          <!-- Burn-Rate Velocity Badge -->
          <div class="flex items-center gap-1 px-2.5 py-1 rounded-md bg-subtle border border-border text-secondary">
            {#if summary.burn_rate_bps > 0}
              <TrendingUp class="h-3.5 w-3.5 text-danger" />
            {:else}
              <TrendingDown class="h-3.5 w-3.5 text-accent" />
            {/if}
            <span>{t('Tốc độ tăng:')}</span>
            <span class="font-bold tabular-numbers {summary.burn_rate_bps > 0 ? 'text-danger' : 'text-accent'}">
              {burnRateStr}
            </span>
          </div>

          <!-- Forecast Days & Health Badge -->
          {#if summary.free_bytes === 0 || usagePercent >= 100 || summary.days_until_full === 0}
            <div class="flex items-center gap-1.5 px-2.5 py-1 rounded-md bg-danger/10 text-danger border border-danger/20 font-medium">
              <AlertCircle class="h-3.5 w-3.5" />
              <span>{t('Ổ đĩa đã đầy')}</span>
            </div>
          {:else if summary.days_until_full !== null && summary.days_until_full !== undefined}
            <div
              class={`flex items-center gap-1.5 px-2.5 py-1 rounded-md border font-medium ${
                summary.days_until_full <= 14
                  ? 'bg-danger/10 text-danger border-danger/20'
                  : summary.days_until_full <= 30
                  ? 'bg-amber-500/10 text-amber-600 border-amber-500/20'
                  : 'bg-accent/10 text-accent border-accent/20'
              }`}
            >
              <AlertCircle class="h-3.5 w-3.5" />
              <span>
                {t('Còn khoảng {days} ngày trước khi đầy', { days: summary.days_until_full })}
              </span>
            </div>
          {:else if isCritical || usagePercent >= 90}
            <div class="flex items-center gap-1.5 px-2.5 py-1 rounded-md bg-danger/10 text-danger border border-danger/20 font-medium">
              <AlertCircle class="h-3.5 w-3.5" />
              <span>{t('Nguy cấp · {pct:.0f}%', { pct: usagePercent })}</span>
            </div>
          {:else}
            <div class="flex items-center gap-1.5 px-2.5 py-1 rounded-md bg-accent/10 text-accent border border-accent/20 font-medium">
              <CheckCircle2 class="h-3.5 w-3.5" />
              <span>{t('Ổn định · {pct:.0f}%', { pct: usagePercent })}</span>
            </div>
          {/if}
        </div>

        <!-- Primary Data Drive Tag -->
        {#if summary.primary_data_drive}
          <div class="text-secondary text-xs">
            <span class="text-disabled">{t('Ổ lưu dữ liệu:')}</span>
            <span class="font-medium text-primary ml-1">{summary.primary_data_drive.data_root}</span>
            <span class="text-disabled ml-1">
              ({t('Còn trống {free}', { free: formatSize(summary.primary_data_drive.free_bytes) })})
            </span>
          </div>
        {/if}
      </div>
    {/if}
  </div>

  <!-- Section 2: Space Lens Treemap -->
  <div class="space-y-2">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-bold text-primary">{t('Bản đồ không gian (Space Lens)')}</h2>
      <button
        type="button"
        onclick={triggerSpaceScan}
        disabled={isScanningSpace}
        class="px-3.5 py-1.5 rounded-lg text-xs font-medium border border-border bg-card hover:bg-subtle text-primary transition-colors disabled:opacity-50 flex items-center gap-1.5 shadow-sm"
      >
        <RefreshCw class={`h-3.5 w-3.5 ${isScanningSpace ? 'animate-spin' : ''}`} />
        {t('Quét Space Lens')}
      </button>
    </div>

    <UpdateStatus savedAt={spaceSavedAt} refreshing={isScanningSpace} failed={spaceFailed} />

    <!-- Real-time Progress Bar Overlay during Scan -->
    {#if progressState && progressState.state === 'running'}
      <div class="rounded-lg border border-border bg-subtle p-3 space-y-1.5">
        <div class="flex items-center justify-between text-xs text-secondary">
          <span class="font-medium text-primary">{progressState.title}</span>
          <span class="tabular-numbers">{progressState.percent}%</span>
        </div>
        <div class="w-full bg-border/40 h-1.5 rounded-full overflow-hidden">
          <div
            class="h-full bg-accent transition-all duration-100"
            style="width: {progressState.percent}%"
          ></div>
        </div>
        <div class="text-[11px] text-disabled truncate">{progressState.detail}</div>
      </div>
    {/if}

    <SpaceTreemap
      rootNode={spaceNode}
      isLoading={isScanningSpace}
      onRefreshRequested={triggerSpaceScan}
    />
  </div>

  <!-- Section 3: Daily Delta Timeline & Process/Service Attribution (US5) -->
  <DailyBarChart drive={selectedDrive} />

  <!-- Section 4: ExpandableTableCard (Có thể giải phóng) -->
  <div class="space-y-2">
    <UpdateStatus
      savedAt={reclaimableSavedAt}
      refreshing={isScanningReclaimable}
      failed={reclaimableFailed}
    />
    <ExpandableTableCard
      title={t('Có thể giải phóng')}
      items={reclaimable}
      emptyText={isScanningReclaimable
        ? t('Đang quét phân tích ứng dụng và kiểm tra khóa tệp tin...')
        : reclaimableSavedAt !== null
          ? t('Không tìm thấy tệp rác hoặc bộ nhớ đệm cần dọn dẹp theo các hồ sơ đã chọn.')
          : t('Chưa có kết quả phân tích')}
    />
  </div>
</div>
