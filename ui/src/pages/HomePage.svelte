<!-- ui/src/pages/HomePage.svelte: Full-width low-tech landing screen with 1-click Make Green Now & Savings Scorecard -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    getBootstrapState,
    getOverviewSummary,
    listDriveQuotas,
    makeGreenNow,
    type BootstrapState,
    type DriveInfo,
    type DriveQuotaStatusDto,
    type MakeGreenReportDto,
    type OverviewSummary,
  } from '$lib/api';
  import { fileActions, onFilesChanged } from '$lib/busy.svelte';
  import { taskStore } from '$lib/tasks.svelte';
  import SavingsScorecard from '$lib/components/SavingsScorecard.svelte';
  import {
    Zap,
    HardDrive,
    CheckCircle2,
    AlertTriangle,
    RefreshCw,
    RotateCcw,
    SlidersHorizontal,
    ShieldCheck,
    Trash2,
    X,
    CheckSquare,
    Square,
    ChevronDown,
    ChevronUp,
  } from 'lucide-svelte';

  interface Props {
    goalRefreshKey?: number;
    onNavigateRestore?: () => void;
    onNavigateAdvanced?: () => void;
    onNavigateSettings?: () => void;
  }

  let {
    goalRefreshKey = 0,
    onNavigateRestore,
    onNavigateAdvanced,
    onNavigateSettings,
  }: Props = $props();

  let bootstrap = $state<BootstrapState | null>(null);
  let summary = $state<OverviewSummary | null>(null);
  let primaryGoal = $state<DriveQuotaStatusDto | null>(null);
  let selectedBackupDrive = $state<string | null>(null);
  let isLoading = $state(false);
  let scorecardKey = $state(0);

  // Result & follow-up modal states
  let lastReport = $state<MakeGreenReportDto | null>(null);
  let showActionsDetail = $state(false);
  let showSingleDriveModal = $state(false);
  let showConfirmCloseModal = $state(false);
  let selectedCloseAppIds = $state<string[]>([]);

  let backupDrives = $derived<DriveInfo[]>(
    (bootstrap?.drives ?? []).filter((d) => d.is_eligible && !d.is_system)
  );
  let selectedBackupInfo = $derived(
    backupDrives.find((d) => d.drive_letter === selectedBackupDrive) ?? null
  );

  let isBusy = $derived(fileActions.running > 0 || taskStore.active !== null);
  let usagePercent = $derived(summary ? Math.round(summary.usage_ratio * 100) : 0);
  let maxPct = $derived(primaryGoal ? Math.round(primaryGoal.clean_ratio * 100) : 80);
  let isOverGoal = $derived(usagePercent >= maxPct);
  let isWarning = $derived(!isOverGoal && usagePercent >= Math.max(70, maxPct - 10));

  const MAKE_GREEN_STEPS = [
    { key: 'analyze', label: 'Phân tích' },
    { key: 'clean', label: 'Dọn rác' },
    { key: 'compact', label: 'Nén gọn' },
    { key: 'relocate', label: 'Chuyển ổ' },
  ] as const;

  async function loadHome() {
    isLoading = true;
    try {
      const [boot, sum, quotas] = await Promise.all([
        getBootstrapState(),
        getOverviewSummary('C'),
        listDriveQuotas().catch(() => [] as DriveQuotaStatusDto[]),
      ]);
      bootstrap = boot;
      summary = sum;
      primaryGoal = Array.isArray(quotas)
        ? (quotas.find((q) => q.enabled) ?? quotas[0] ?? null)
        : null;

      const eligible = (boot.drives ?? []).filter((d) => d.is_eligible && !d.is_system);
      if (eligible.length > 0) {
        if (!selectedBackupDrive || !eligible.some((d) => d.drive_letter === selectedBackupDrive)) {
          selectedBackupDrive = boot.recommended_drive ?? eligible[0].drive_letter;
        }
      } else {
        selectedBackupDrive = null;
      }
    } catch (err) {
      console.error('Home load failed:', err);
    } finally {
      isLoading = false;
    }
  }

  onFilesChanged(() => {
    loadHome();
    scorecardKey += 1;
  });

  $effect(() => {
    void goalRefreshKey;
    loadHome();
  });

  onMount(() => {
    loadHome();
  });

  function handleClickMakeGreen() {
    if (isBusy) return;
    // Single-drive PC: ask user cleanly between 2 clear options
    if (backupDrives.length === 0) {
      showSingleDriveModal = true;
      return;
    }
    void executeMakeGreen({
      target_drive: selectedBackupDrive,
      include_known_folders: true,
    });
  }

  function handlePickSingleDriveMode(mode: 'direct_cache' | 'local_vault') {
    showSingleDriveModal = false;
    void executeMakeGreen({
      target_drive: null,
      single_drive_mode: mode,
      include_known_folders: false,
    });
  }

  function openConfirmCloseLocked() {
    if (!lastReport || lastReport.skipped_locked.length === 0) return;
    selectedCloseAppIds = lastReport.skipped_locked.map((s) => s.app_id);
    showConfirmCloseModal = true;
  }

  function toggleCloseApp(appId: string) {
    if (selectedCloseAppIds.includes(appId)) {
      selectedCloseAppIds = selectedCloseAppIds.filter((id) => id !== appId);
    } else {
      selectedCloseAppIds = [...selectedCloseAppIds, appId];
    }
  }

  function handleConfirmCloseAndFinish() {
    if (selectedCloseAppIds.length === 0) return;
    const ids = [...selectedCloseAppIds];
    showConfirmCloseModal = false;
    void executeMakeGreen({
      target_drive: selectedBackupDrive,
      single_drive_mode:
        backupDrives.length === 0
          ? ((lastReport?.single_drive_mode as 'direct_cache' | 'local_vault' | null) ??
            'direct_cache')
          : null,
      close_app_ids: ids,
      confirm_close_running_apps: true,
      include_known_folders: backupDrives.length > 0,
    });
  }

  async function executeMakeGreen(opts: Parameters<typeof makeGreenNow>[0]) {
    try {
      const report = await taskStore.run(
        {
          kind: 'make_green',
          title: t('Đang làm xanh ổ C:...'),
          phase: 'analyze',
          steps: MAKE_GREEN_STEPS,
        },
        () => makeGreenNow(opts),
        (res, wasMinimized) => {
          if (wasMinimized) {
            return {
              kind: 'ok',
              title: t('Đã làm xanh ổ C: xong!'),
              detail: t('Đã giải phóng {size}', { size: formatSize(res.total_freed_bytes) }),
            };
          }
          return null;
        }
      );
      lastReport = report;
      showActionsDetail = false;
    } catch {
      // Toast already surfaced by taskStore
    }
  }

  let beforePct = $derived(
    lastReport && lastReport.total_bytes > 0
      ? Math.round((lastReport.before_used_bytes / lastReport.total_bytes) * 100)
      : 0
  );
  let afterPct = $derived(
    lastReport && lastReport.total_bytes > 0
      ? Math.round((lastReport.after_used_bytes / lastReport.total_bytes) * 100)
      : 0
  );
  let totalLockedBytes = $derived(
    lastReport
      ? lastReport.skipped_locked.reduce((acc, item) => acc + item.size_bytes, 0)
      : 0
  );
</script>

<div class="max-w-6xl mx-auto w-full space-y-6">
  <!-- Hero Card: Drive C: Health & One-Click Make Green Now -->
  <section
    class={`rounded-2xl border p-6 shadow-sm transition-colors ${
      isOverGoal
        ? 'border-danger/35 bg-danger/5'
        : isWarning
          ? 'border-amber-500/35 bg-amber-500/5'
          : 'border-accent/30 bg-accent/5'
    }`}
  >
    <div class="flex flex-col lg:flex-row lg:items-center justify-between gap-6">
      <!-- Left: Status & Progress -->
      <div class="flex-1 min-w-0 space-y-3">
        <div class="flex items-center gap-2.5 flex-wrap">
          <span
            class={`inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full text-xs font-semibold ${
              isOverGoal
                ? 'bg-danger/15 text-danger'
                : isWarning
                  ? 'bg-amber-500/15 text-amber-700 dark:text-amber-300'
                  : 'bg-accent/15 text-accent'
            }`}
          >
            {#if isOverGoal || isWarning}
              <AlertTriangle class="h-3.5 w-3.5" />
            {:else}
              <CheckCircle2 class="h-3.5 w-3.5" />
            {/if}
            {isOverGoal
              ? t('Ổ C: đang đầy')
              : isWarning
                ? t('Ổ C: sắp đầy')
                : t('Ổ C: đang xanh')}
          </span>

          <span class="text-xs text-secondary font-medium">
            {t('Mục tiêu')}: <strong class="text-primary font-mono">C:\ &lt; {maxPct}%</strong>
          </span>

          <button
            type="button"
            onclick={loadHome}
            disabled={isLoading}
            class="p-1 rounded-md text-secondary hover:text-primary hover:bg-card/70 transition-colors"
            title={t('Làm mới')}
          >
            <RefreshCw class={`h-3.5 w-3.5 ${isLoading ? 'animate-spin' : ''}`} />
          </button>
        </div>

        <div class="flex items-baseline gap-3 flex-wrap">
          <h1 class="text-2xl sm:text-3xl font-bold text-primary tracking-tight tabular-numbers">
            {#if summary}
              {t('Ổ C: đã dùng {pct}%', { pct: usagePercent })}
            {:else}
              {t('Đang kiểm tra ổ C:...')}
            {/if}
          </h1>
          {#if summary}
            <span class="text-sm text-secondary tabular-numbers">
              {t('Còn trống {free} / {total}', {
                free: formatSize(summary.free_bytes),
                total: formatSize(summary.total_bytes),
              })}
            </span>
          {/if}
        </div>

        <!-- Usage Bar with Goal Marker -->
        <div class="space-y-1.5">
          <div class="relative h-3 rounded-full bg-border/60 overflow-hidden">
            <div
              class={`h-full rounded-full transition-all duration-500 ${
                isOverGoal ? 'bg-danger' : isWarning ? 'bg-amber-500' : 'bg-accent'
              }`}
              style={`width: ${usagePercent}%`}
            ></div>
            <div
              class="absolute top-0 bottom-0 w-0.5 bg-primary/70"
              style={`left: ${maxPct}%`}
              title={`${t('Mục tiêu')}: ${maxPct}%`}
            ></div>
          </div>

          <!-- Backup Drive Row -->
          <div class="flex items-center justify-between gap-3 flex-wrap text-xs text-secondary pt-1">
            <div class="flex items-center gap-2 flex-wrap">
              <HardDrive class="h-3.5 w-3.5 text-accent" />
              {#if backupDrives.length > 0}
                <span>{t('Ổ dự phòng:')}</span>
                <select
                  bind:value={selectedBackupDrive}
                  disabled={isBusy}
                  class="bg-card border border-border rounded-md px-2 py-0.5 text-xs font-semibold text-primary outline-none cursor-pointer"
                >
                  {#each backupDrives as drv}
                    <option value={drv.drive_letter}>
                      {drv.drive_letter}: ({drv.label || t('Ổ đĩa cục bộ')} · {t('trống {free}', { free: formatSize(drv.free_bytes) })})
                    </option>
                  {/each}
                </select>
              {:else}
                <span>{t('Máy chỉ có 1 ổ C: (sẽ hỏi cách dọn an toàn khi chạy)')}</span>
              {/if}
            </div>

            {#if selectedBackupInfo}
              <span class="text-emerald-600 dark:text-emerald-400 font-medium">
                {t('Tự động sao lưu & có thể khôi phục 1 chạm')}
              </span>
            {/if}
          </div>
        </div>
      </div>

      <!-- Right: Primary CTA -->
      <div class="flex flex-col items-stretch sm:items-end gap-2 shrink-0">
        <button
          type="button"
          onclick={handleClickMakeGreen}
          disabled={isBusy}
          class="inline-flex items-center justify-center gap-2.5 px-6 py-3.5 rounded-xl bg-accent hover:bg-accent-hover text-white text-sm font-bold shadow-md hover:shadow-lg transition-all disabled:opacity-50 cursor-pointer"
        >
          <Zap class="h-5 w-5" />
          <span>{t('Làm xanh ổ C: ngay')}</span>
        </button>
        <p class="text-[11px] text-secondary text-center sm:text-right max-w-xs">
          {t('Tự động phân tích → dọn rác → nén gọn → chuyển dữ liệu sang ổ dự phòng.')}
        </p>
      </div>
    </div>
  </section>

  <!-- Inline Report Card after Make Green Now finishes -->
  {#if lastReport}
    <section class="rounded-2xl border border-emerald-500/35 bg-card p-5 shadow-sm space-y-4">
      <div class="flex items-start justify-between gap-3">
        <div class="flex items-center gap-2.5">
          <div class="h-9 w-9 rounded-xl bg-emerald-500/15 text-emerald-600 dark:text-emerald-400 flex items-center justify-center shrink-0">
            <CheckCircle2 class="h-5 w-5" />
          </div>
          <div>
            <h3 class="text-base font-bold text-primary">
              {#if lastReport.cancelled}
                {t('Đã dừng — đã giải phóng {size}', { size: formatSize(lastReport.total_freed_bytes) })}
              {:else if lastReport.total_freed_bytes > 0}
                {t('Xong! Đã giải phóng {size} cho ổ C:', { size: formatSize(lastReport.total_freed_bytes) })}
              {:else}
                {t('Ổ C: của bạn đã khá gọn gàng!')}
              {/if}
            </h3>
            <p class="text-xs text-secondary tabular-numbers">
              {t('Ổ C: từ {before}% → {after}% · Trống thêm từ {bFree} lên {aFree}', {
                before: beforePct,
                after: afterPct,
                bFree: formatSize(lastReport.before_free_bytes),
                aFree: formatSize(lastReport.after_free_bytes),
              })}
            </p>
          </div>
        </div>
        <button
          type="button"
          onclick={() => (lastReport = null)}
          class="p-1 text-secondary hover:text-primary"
          aria-label={t('Đóng')}
        >
          <X class="h-4 w-4" />
        </button>
      </div>

      <!-- Breakdown pills -->
      <div class="grid grid-cols-1 sm:grid-cols-3 gap-2.5 text-xs">
        <div class="rounded-lg bg-subtle px-3.5 py-2.5 flex items-center justify-between">
          <span class="text-secondary">{t('Đã dọn rác & cache')}</span>
          <strong class="text-primary font-mono">{formatSize(lastReport.cleaned_bytes)}</strong>
        </div>
        <div class="rounded-lg bg-subtle px-3.5 py-2.5 flex items-center justify-between">
          <span class="text-secondary">{t('Đã nén gọn tại chỗ')}</span>
          <strong class="text-primary font-mono">{formatSize(lastReport.compacted_bytes)}</strong>
        </div>
        <div class="rounded-lg bg-subtle px-3.5 py-2.5 flex items-center justify-between">
          <span class="text-secondary">
            {lastReport.backup_drive
              ? t('Đã chuyển sang ổ {drive}:', { drive: lastReport.backup_drive })
              : t('Di chuyển dữ liệu')}
          </span>
          <strong class="text-primary font-mono">{formatSize(lastReport.relocated_bytes)}</strong>
        </div>
      </div>

      <!-- Locked / Running Apps Follow-up Callout -->
      {#if lastReport.skipped_locked.length > 0}
        <div class="rounded-xl border border-amber-500/35 bg-amber-500/10 p-4 flex flex-col sm:flex-row sm:items-center justify-between gap-3">
          <div class="space-y-0.5 text-xs">
            <div class="font-bold text-amber-800 dark:text-amber-200 flex items-center gap-1.5">
              <AlertTriangle class="h-4 w-4 shrink-0" />
              {t('Còn {size} ở các ứng dụng đang mở chưa thể xử lý', {
                size: formatSize(totalLockedBytes),
              })}
            </div>
            <p class="text-amber-700 dark:text-amber-300">
              {lastReport.skipped_locked.map((s) => `${s.app_name} (${formatSize(s.size_bytes)})`).join(' · ')}
            </p>
          </div>
          <button
            type="button"
            onclick={openConfirmCloseLocked}
            disabled={isBusy}
            class="px-4 py-2 rounded-lg bg-amber-600 hover:bg-amber-700 text-white text-xs font-semibold shrink-0 disabled:opacity-50"
          >
            {t('Đóng ứng dụng & giải phóng nốt')}
          </button>
        </div>
      {/if}

      <!-- Optional itemized details + Quick links -->
      <div class="flex items-center justify-between gap-3 flex-wrap pt-1 text-xs">
        {#if lastReport.actions.length > 0}
          <button
            type="button"
            onclick={() => (showActionsDetail = !showActionsDetail)}
            class="inline-flex items-center gap-1 text-secondary hover:text-primary font-medium"
          >
            {#if showActionsDetail}
              <ChevronUp class="h-3.5 w-3.5" />
            {:else}
              <ChevronDown class="h-3.5 w-3.5" />
            {/if}
            {t('Xem chi tiết ({count} mục)', { count: lastReport.actions.length })}
          </button>
        {:else}
          <span></span>
        {/if}

        <div class="flex items-center gap-3">
          {#if onNavigateRestore}
            <button
              type="button"
              onclick={onNavigateRestore}
              class="inline-flex items-center gap-1 font-semibold text-accent hover:underline"
            >
              <RotateCcw class="h-3.5 w-3.5" />
              {t('Cần lấy lại dữ liệu? Mở Khôi phục')}
            </button>
          {/if}
          {#if onNavigateAdvanced}
            <button
              type="button"
              onclick={onNavigateAdvanced}
              class="inline-flex items-center gap-1 font-medium text-secondary hover:text-primary"
            >
              <SlidersHorizontal class="h-3.5 w-3.5" />
              {t('Tùy chỉnh nâng cao')}
            </button>
          {/if}
        </div>
      </div>

      {#if showActionsDetail && lastReport.actions.length > 0}
        <div class="rounded-lg border border-border/70 divide-y divide-border/50 text-xs max-h-52 overflow-y-auto">
          {#each lastReport.actions as act}
            <div class="px-3 py-2 flex items-center justify-between gap-2">
              <div class="min-w-0">
                <span class="font-semibold text-primary">{act.name}</span>
                {#if act.detail}
                  <span class="text-secondary ml-1.5">· {act.detail}</span>
                {/if}
              </div>
              <span class="font-mono font-semibold text-emerald-600 dark:text-emerald-400 shrink-0">
                +{formatSize(act.bytes_freed)}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </section>
  {/if}

  <!-- Impact & Savings Scorecard (Day / Week / Month / Year + Top Junk Apps) -->
  <SavingsScorecard refreshKey={scorecardKey} />

  <!-- Subtle Quick Footer Hint -->
  {#if onNavigateSettings}
    <div class="flex items-center justify-end text-xs text-secondary">
      <button type="button" onclick={onNavigateSettings} class="hover:text-primary underline-offset-2 hover:underline">
        {t('Chỉnh mục tiêu dung lượng & lịch chạy tự động')}
      </button>
    </div>
  {/if}
</div>

<!-- Single-Drive 2-Option Modal (shown ONLY when PC has no secondary backup drive) -->
{#if showSingleDriveModal}
  <div class="fixed inset-0 z-50 bg-black/45 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-2xl border border-border bg-card p-6 shadow-2xl max-w-lg w-full space-y-4">
      <div class="flex items-center justify-between">
        <h3 class="font-bold text-base text-primary">{t('Máy bạn chỉ có 1 ổ C:')}</h3>
        <button
          type="button"
          onclick={() => (showSingleDriveModal = false)}
          class="text-secondary hover:text-primary"
        >
          <X class="h-4 w-4" />
        </button>
      </div>

      <p class="text-xs text-secondary leading-relaxed">
        {t('Không tìm thấy ổ đĩa phụ (D:, E:...) để cất bản dự phòng. Bạn muốn GreenC dọn ổ C: theo cách nào?')}
      </p>

      <div class="space-y-2.5">
        <button
          type="button"
          onclick={() => handlePickSingleDriveMode('direct_cache')}
          class="w-full text-left rounded-xl border-2 border-accent/50 hover:border-accent bg-accent/5 p-4 transition-colors space-y-1"
        >
          <div class="flex items-center gap-2 font-bold text-sm text-primary">
            <Trash2 class="h-4 w-4 text-accent" />
            <span>{t('Giải phóng tối đa ngay (Khuyên dùng)')}</span>
          </div>
          <p class="text-xs text-secondary">
            {t('Xóa thẳng các bộ nhớ đệm có thể tự tạo lại (trình duyệt, app chat, Temp, dev cache) + nén gọn thư mục. Không bao giờ đụng vào dữ liệu cá nhân hay Windows.old.')}
          </p>
        </button>

        <button
          type="button"
          onclick={() => handlePickSingleDriveMode('local_vault')}
          class="w-full text-left rounded-xl border border-border hover:border-accent/60 bg-subtle/50 p-4 transition-colors space-y-1"
        >
          <div class="flex items-center gap-2 font-bold text-sm text-primary">
            <ShieldCheck class="h-4 w-4 text-emerald-600" />
            <span>{t('An toàn — Nén giữ bản dự phòng trên ổ C:')}</span>
          </div>
          <p class="text-xs text-secondary">
            {t('Nén các tệp đã dọn vào Kho dự phòng trên ổ C: để có thể khôi phục 1 chạm trong trang Khôi phục (dung lượng trống tăng ít hơn).')}
          </p>
        </button>
      </div>

      <div class="flex justify-end">
        <button
          type="button"
          onclick={() => (showSingleDriveModal = false)}
          class="px-4 py-1.5 rounded-lg border border-border text-xs font-medium text-secondary hover:text-primary"
        >
          {t('Hủy')}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Confirmed Close Running Apps Modal -->
{#if showConfirmCloseModal && lastReport}
  <div class="fixed inset-0 z-50 bg-black/45 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-2xl border border-border bg-card p-6 shadow-2xl max-w-md w-full space-y-4">
      <div class="flex items-center justify-between">
        <h3 class="font-bold text-base text-primary">{t('Đóng các ứng dụng đang mở?')}</h3>
        <button
          type="button"
          onclick={() => (showConfirmCloseModal = false)}
          class="text-secondary hover:text-primary"
        >
          <X class="h-4 w-4" />
        </button>
      </div>

      <p class="text-xs text-secondary leading-relaxed">
        {t('Bạn có muốn GreenC đóng tạm các ứng dụng bên dưới để dọn và chuyển nốt dữ liệu, sau đó tự mở lại không?')}
      </p>

      <div class="rounded-xl border border-border divide-y divide-border/60 max-h-56 overflow-y-auto">
        {#each lastReport.skipped_locked as item}
          {@const checked = selectedCloseAppIds.includes(item.app_id)}
          <button
            type="button"
            onclick={() => toggleCloseApp(item.app_id)}
            class="w-full px-3.5 py-2.5 flex items-center justify-between gap-3 hover:bg-subtle/60 text-left text-xs"
          >
            <span class="flex items-center gap-2 min-w-0">
              {#if checked}
                <CheckSquare class="h-4 w-4 text-accent shrink-0" />
              {:else}
                <Square class="h-4 w-4 text-secondary shrink-0" />
              {/if}
              <span class="font-semibold text-primary truncate">{item.app_name}</span>
            </span>
            <span class="font-mono font-medium text-secondary shrink-0">
              {formatSize(item.size_bytes)}
            </span>
          </button>
        {/each}
      </div>

      <div class="flex justify-end gap-2 pt-1">
        <button
          type="button"
          onclick={() => (showConfirmCloseModal = false)}
          class="px-4 py-2 rounded-lg border border-border text-xs font-medium text-secondary hover:text-primary"
        >
          {t('Để sau')}
        </button>
        <button
          type="button"
          disabled={selectedCloseAppIds.length === 0}
          onclick={handleConfirmCloseAndFinish}
          class="px-4 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold disabled:opacity-50"
        >
          {t('Đóng & giải phóng nốt')}
        </button>
      </div>
    </div>
  </div>
{/if}
