<!-- ui/src/pages/CleanupPage.svelte -->
<script module lang="ts">
  // Groups the user unticked. Only their own clicks change this, whatever list is shown.
  const unticked = new Set<string>();
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { fileActions, onFilesChanged } from '$lib/busy.svelte';
  import { taskStore } from '$lib/tasks.svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    scanCleanupCandidates,
    executeCleanup,
    type CleanupCandidateDto,
    type CleanupReportDto,
  } from '$lib/api';
  import UpdateStatus from '$lib/components/UpdateStatus.svelte';
  import { createLoader } from '$lib/cache';
  import { DEFAULT_CLEANUP_PROFILES, cleanupKey } from '$lib/scans';
  import {
    Trash2,
    ShieldCheck,
    CheckSquare,
    Square,
    AlertTriangle,
    CheckCircle,
    RefreshCw,
    X,
    Folder,
    HardDrive,
    Info,
    Lock
  } from 'lucide-svelte';

  // State via Svelte 5 Runes
  let candidates = $state<CleanupCandidateDto[]>([]);
  let isLoading = $state(false);
  // The list may be the last result: cleaning waits until a scan made now has arrived.
  let savedAt = $state<number | null>(null);
  let isFresh = $state(false);
  let loadFailed = $state(false);
  // Actions wait for a scan made now and for any other file action to end.
  let canAct = $derived(isFresh && fileActions.running === 0);
  let isExecuting = $state(false);
  let enableVault = $state(true);
  let isPermanent = $state(false);

  // Selected Profile IDs for filtering: includes safe 'old_versions', excludes opt-in 'orphan_radar'
  const allProfiles = [
    { id: 'windows_temp', name: 'Temp Windows' },
    { id: 'browser_cache', name: 'Trình duyệt' },
    { id: 'chat_media', name: 'Chat & Mạng xã hội' },
    { id: 'crash_dumps', name: 'Crash Dumps' },
    { id: 'package_managers', name: 'Gói Dev' },
    { id: 'app_logs', name: 'Nhật ký' },
    { id: 'old_versions', name: 'Bản cũ' },
    { id: 'orphan_radar', name: 'Mồ côi' },
  ];

  let selectedProfileIds = $state<string[]>([...DEFAULT_CLEANUP_PROFILES]);

  let cleanupReport = $state<CleanupReportDto | null>(null);
  let showReportModal = $state(false);

  // Derived metrics
  let selectedCandidates = $derived(candidates.filter((c) => c.selected));
  let totalReclaimableBytes = $derived(
    selectedCandidates.reduce((acc, c) => acc + c.reclaimable_bytes, 0)
  );
  let totalPhysicalBytes = $derived(
    selectedCandidates.reduce((acc, c) => acc + c.total_physical_bytes, 0)
  );
  let totalFilesCount = $derived(
    selectedCandidates.reduce((acc, c) => acc + c.file_count, 0)
  );
  let isAllSelected = $derived(
    candidates.length > 0 && candidates.every((c) => c.selected)
  );

  const loadScan = createLoader<CleanupCandidateDto[]>({
    show(data, at, fresh) {
      candidates = (data ?? []).map((c) => (unticked.has(c.id) ? { ...c, selected: false } : c));
      savedAt = at;
      isFresh = fresh;
    },
    status(loading, failed) {
      isLoading = loading;
      loadFailed = failed;
    },
  });

  function triggerScan(force = false) {
    const profiles = [...selectedProfileIds];
    return loadScan(cleanupKey(profiles), () => scanCleanupCandidates(profiles), { force });
  }

  function toggleProfileFilter(profileId: string) {
    if (selectedProfileIds.includes(profileId)) {
      selectedProfileIds = selectedProfileIds.filter((id) => id !== profileId);
    } else {
      selectedProfileIds = [...selectedProfileIds, profileId];
    }
    triggerScan();
  }

  function setSelected(cand: CleanupCandidateDto, selected: boolean) {
    cand.selected = selected;
    if (selected) unticked.delete(cand.id);
    else unticked.add(cand.id);
  }

  function toggleSelectAll() {
    const nextState = !isAllSelected;
    for (const c of candidates) {
      setSelected(c, nextState);
    }
  }

  function toggleCandidate(cand: CleanupCandidateDto) {
    setSelected(cand, !cand.selected);
  }

  async function handleStartCleanup() {
    const selectedIds = selectedCandidates.map((c) => c.id);
    if (selectedIds.length === 0) return;

    cleanupReport = null;
    isExecuting = true;

    try {
      const report = await taskStore.run(
        {
          kind: 'cleanup_execute',
          title: t('Đang dọn dẹp an toàn...'),
          phase: 'prepare',
          totalFiles: selectedIds.length,
        },
        () => executeCleanup(selectedIds, enableVault, isPermanent),
        (res, wasMinimized) =>
          wasMinimized
            ? {
                kind: 'ok',
                title: t('Dọn dẹp thành công!'),
                detail: t('Đã giải phóng {size}', { size: formatSize(res.total_reclaimed_bytes) }),
              }
            : null
      );
      cleanupReport = report;
      showReportModal = true;
    } catch {
      // Toast handled by taskStore
    } finally {
      isExecuting = false;
    }
  }

  onFilesChanged(() => triggerScan(true));

  onMount(() => {
    triggerScan();
  });
</script>

<div class="space-y-6">
  <!-- Profile Filter Chips -->
  <div class="flex flex-wrap items-center gap-2">
    <span class="text-xs font-semibold uppercase tracking-wider text-secondary mr-2">
      {t('Hồ sơ dọn dẹp:')}
    </span>
    {#each allProfiles as prof}
      <button
        class={`px-3 py-1.5 rounded-full text-xs font-medium transition-all ${
          selectedProfileIds.includes(prof.id)
            ? 'bg-accent text-white shadow-sm'
            : 'bg-subtle text-secondary hover:bg-card border border-border'
        }`}
        onclick={() => toggleProfileFilter(prof.id)}
      >
        {t(prof.name)}
      </button>
    {/each}
    <button
      class="ml-auto inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md border border-border bg-card text-xs font-medium text-secondary hover:bg-subtle transition-colors"
      disabled={isLoading || isExecuting}
      onclick={() => triggerScan(true)}
    >
      <RefreshCw class={`h-3.5 w-3.5 ${isLoading ? 'animate-spin' : ''}`} />
      {t('Quét lại')}
    </button>
  </div>

  <UpdateStatus {savedAt} refreshing={isLoading} failed={loadFailed} />

  <!-- Reclaimable Space Summary Card -->
  <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
    <div class="rounded-xl border border-border bg-card p-5 shadow-sm">
      <div class="flex items-center justify-between">
        <span class="text-xs font-medium text-secondary uppercase">{t('Dung lượng có thể giải phóng')}</span>
        <HardDrive class="h-4 w-4 text-accent" />
      </div>
      <div class="mt-2 flex items-baseline gap-2">
        <span class="text-3xl font-bold tracking-tight text-primary">
          {formatSize(totalReclaimableBytes)}
        </span>
        <span class="text-xs text-secondary">
          / {t('{size} tổng', { size: formatSize(totalPhysicalBytes) })}
        </span>
      </div>
      <div class="mt-3 flex items-center gap-2 text-xs text-secondary">
        <span class="inline-flex items-center gap-1 text-emerald-600 font-medium">
          <CheckCircle class="h-3.5 w-3.5" />
          {t('{count} nhóm đã chọn', { count: selectedCandidates.length })}
        </span>
        <span>• {t('{count} tệp tin', { count: totalFilesCount })}</span>
      </div>
    </div>

    <!-- Safety & Canary Vault Options Card -->
    <div class="rounded-xl border border-border bg-card p-5 shadow-sm md:col-span-2 flex flex-col justify-between">
      <div class="flex items-start justify-between">
        <div>
          <div class="flex items-center gap-2">
            <ShieldCheck class="h-5 w-5 text-emerald-600" />
            <h3 class="text-sm font-semibold text-primary">{t('Cơ chế Bảo vệ Canary Vault')}</h3>
          </div>
          <p class="mt-1 text-xs text-secondary leading-relaxed">
            {t('Tự động nén và chuyển tệp vào Kho an toàn (Canary Vault) trong thời gian cách ly trước khi giải phóng. Nếu ứng dụng báo lỗi tệp tin thiếu, GreenC sẽ tự động hoàn tác khôi phục ngay lập tức.')}
          </p>
        </div>
      </div>

      <div class="mt-4 flex flex-wrap items-center justify-between gap-4 pt-3 border-t border-border/60">
        <label class="flex items-center gap-2.5 cursor-pointer">
          <input
            type="checkbox"
            class="h-4 w-4 rounded border-border text-accent focus:ring-accent"
            bind:checked={enableVault}
          />
          <span class="text-xs font-medium text-primary">{t('Lưu trữ vào Kho an toàn (Khuyên dùng)')}</span>
        </label>

        {#if !enableVault}
          <label class="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              class="h-4 w-4 rounded border-red-500 text-red-600 focus:ring-red-500"
              bind:checked={isPermanent}
            />
            <span class="text-xs font-medium text-red-600">{t('Xóa vĩnh viễn (Bỏ qua Thùng rác)')}</span>
          </label>
        {/if}

        <button
          class="ml-auto inline-flex items-center gap-2 px-5 py-2 rounded-lg bg-accent text-white text-sm font-medium hover:bg-accent/90 shadow transition-all disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={selectedCandidates.length === 0 || isExecuting || isLoading || !canAct}
          onclick={handleStartCleanup}
        >
          <Trash2 class="h-4 w-4" />
          {t('Dọn dẹp {size}', { size: formatSize(totalReclaimableBytes) })}
        </button>
      </div>
    </div>
  </div>

  <!-- Candidates Table -->
  <div class="rounded-xl border border-border bg-card shadow-sm overflow-hidden">
    <div class="px-5 py-3.5 border-b border-border bg-subtle/50 flex items-center justify-between">
      <div class="flex items-center gap-3">
        <button
          class="text-secondary hover:text-primary transition-colors"
          onclick={toggleSelectAll}
          title={t('Chọn tất cả')}
        >
          {#if isAllSelected}
            <CheckSquare class="h-4 w-4 text-accent" />
          {:else}
            <Square class="h-4 w-4" />
          {/if}
        </button>
        <span class="text-xs font-semibold text-primary uppercase tracking-wider">
          {t('Danh sách đề xuất dọn dẹp')} ({candidates.length})
        </span>
      </div>
      <span class="text-xs text-secondary">
        {t('Đã chọn {selected} / {total}', { selected: selectedCandidates.length, total: candidates.length })}
      </span>
    </div>

    {#if isLoading && candidates.length === 0}
      <div class="p-12 text-center text-secondary">
        <RefreshCw class="h-8 w-8 animate-spin mx-auto text-accent mb-3" />
        <p class="text-sm">{t('Đang quét phân tích ứng dụng và kiểm tra khóa tệp tin...')}</p>
      </div>
    {:else if candidates.length === 0 && !loadFailed}
      <div class="p-12 text-center text-secondary">
        <CheckCircle class="h-8 w-8 text-emerald-500 mx-auto mb-2" />
        <p class="text-sm font-medium text-primary">{t('Hệ thống sạch sẽ')}</p>
        <p class="text-xs mt-1">{t('Không tìm thấy tệp rác hoặc bộ nhớ đệm cần dọn dẹp theo các hồ sơ đã chọn.')}</p>
      </div>
    {:else}
      <div class="divide-y divide-border/60">
        {#each candidates as cand}
          <div
            class={`p-4 flex items-center justify-between gap-4 transition-colors hover:bg-subtle/40 ${
              cand.selected ? 'bg-accent/5' : ''
            }`}
          >
            <!-- Checkbox & Info -->
            <div class="flex items-start gap-3 min-w-0 flex-1">
              <button
                class="mt-1 text-secondary hover:text-primary transition-colors flex-shrink-0"
                onclick={() => toggleCandidate(cand)}
              >
                {#if cand.selected}
                  <CheckSquare class="h-4 w-4 text-accent" />
                {:else}
                  <Square class="h-4 w-4" />
                {/if}
              </button>

              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2 flex-wrap">
                  <span class="font-semibold text-sm text-primary">{cand.app_name}</span>
                  {#if cand.is_orphan}
                    <span class="px-2 py-0.5 rounded text-[10px] font-medium bg-amber-500/10 text-amber-600 border border-amber-500/20">
                      {t('Mồ côi')}
                    </span>
                  {/if}
                  {#if cand.is_locked}
                    <span class="px-2 py-0.5 rounded text-[10px] font-medium bg-red-500/10 text-red-600 border border-red-500/20 flex items-center gap-1">
                      <Lock class="h-2.5 w-2.5" />
                      {t('{count} tệp đang khóa', { count: cand.locked_file_count })}
                    </span>
                  {/if}
                  {#if cand.canary_probe_passed}
                    <span class="px-2 py-0.5 rounded text-[10px] font-medium bg-emerald-500/10 text-emerald-600 border border-emerald-500/20 flex items-center gap-1">
                      <ShieldCheck class="h-2.5 w-2.5" />
                      {t('Canary an toàn')}
                    </span>
                  {/if}
                </div>

                <p class="text-xs text-secondary truncate mt-0.5" title={cand.path}>
                  {cand.path}
                </p>

                <p class="text-[11px] text-disabled mt-1">
                  {t('{count} tệp tin', { count: cand.file_count })} • {cand.canary_probe_detail}
                </p>
              </div>
            </div>

            <!-- Size & Actions -->
            <div class="text-right flex-shrink-0">
              <div class="text-sm font-bold text-primary">
                {formatSize(cand.reclaimable_bytes)}
              </div>
              {#if cand.total_physical_bytes > cand.reclaimable_bytes}
                <div class="text-[11px] text-secondary">
                  {t('Tổng: {size}', { size: formatSize(cand.total_physical_bytes) })}
                </div>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

<!-- Post-Cleanup Summary Report Dialog -->
{#if showReportModal && cleanupReport}
  <div class="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="bg-card border border-border rounded-2xl p-6 max-w-lg w-full shadow-2xl space-y-4 max-h-full overflow-y-auto">
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-2 text-emerald-600">
          <CheckCircle class="h-6 w-6" />
          <h3 class="font-bold text-lg text-primary">{t('Dọn dẹp thành công!')}</h3>
        </div>
        <button
          class="text-secondary hover:text-primary"
          onclick={() => showReportModal = false}
        >
          <X class="h-5 w-5" />
        </button>
      </div>

      <div class="rounded-xl bg-subtle p-4 space-y-2 text-sm">
        <div class="flex justify-between">
          <span class="text-secondary">{t('Dung lượng giải phóng:')}</span>
          <span class="font-bold text-emerald-600">{formatSize(cleanupReport.total_reclaimed_bytes)}</span>
        </div>
        <div class="flex justify-between">
          <span class="text-secondary">{t('Số tệp tin đã xóa:')}</span>
          <span class="font-medium text-primary">{cleanupReport.total_files_deleted}</span>
        </div>
        {#if cleanupReport.vault_archive_id}
          <div class="flex justify-between">
            <span class="text-secondary">{t('Lưu trữ Kho an toàn:')}</span>
            <span class="font-medium text-accent">{cleanupReport.vault_archive_id}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-secondary">{t('Dung lượng nén trong kho:')}</span>
            <span class="font-medium text-primary">{formatSize(cleanupReport.compressed_vault_bytes)}</span>
          </div>
        {/if}
        {#if cleanupReport.skipped_locked_files_count > 0}
          <div class="flex justify-between text-amber-600">
            <span>{t('Tệp bỏ qua do đang khóa:')}</span>
            <span>{cleanupReport.skipped_locked_files_count}</span>
          </div>
        {/if}
      </div>

      <p class="text-xs text-secondary">
        {t('Dữ liệu đã được bảo vệ trong Kho dự phòng. Bạn có thể duyệt hoặc hoàn tác bất cứ lúc nào trong trang Kho dự phòng.')}
      </p>

      <div class="flex justify-end pt-2">
        <button
          class="px-5 py-2 rounded-lg bg-accent text-white text-sm font-medium hover:bg-accent/90"
          onclick={() => showReportModal = false}
        >
          {t('Đóng')}
        </button>
      </div>
    </div>
  </div>
{/if}
