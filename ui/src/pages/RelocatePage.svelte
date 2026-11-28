<!-- ui/src/pages/RelocatePage.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import UpdateStatus from '$lib/components/UpdateStatus.svelte';
  import { createLoader } from '$lib/cache';
  import { fileActions, onFilesChanged, recoveries, recoverOnce } from '$lib/busy.svelte';
  import { taskStore } from '$lib/tasks.svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    scanRelocationCandidates,
    relocateDirectory,
    relocateBatch,
    getKnownFolders,
    relocateKnownFolder,
    listRelocationHistory,
    undoRelocation,
    repairJunctionTarget,
    getBootstrapState,
    type RelocationCandidateDto,
    type KnownFolderDto,
    type RelocationResultDto,
    type DriveInfo,
  } from '$lib/api';
  import GuidedModal from '$lib/components/GuidedModal.svelte';
  import ZaloCard from '$lib/components/ZaloCard.svelte';
  import {
    ArrowRightLeft,
    FolderSync,
    AlertTriangle,
    CheckCircle2,
    RefreshCw,
    Folder,
    HardDrive,
    Undo2,
    Copy,
    CloudOff,
    Download,
    FileText,
    Image,
    Video,
    Music,
    ChevronDown,
    ChevronUp,
    Wrench,
    CheckSquare,
    Square,
    Zap,
  } from 'lucide-svelte';

  let candidates = $state<RelocationCandidateDto[]>([]);
  let knownFolders = $state<KnownFolderDto[]>([]);
  let historyRecords = $state<RelocationResultDto[]>([]);
  let needsReview = $derived(recoveries.list.filter((r) => r.status === 'needs_review'));
  let availableDrives = $state<DriveInfo[]>([]);
  let targetDrive = $state('D');
  let isStoreBuild = $state(false);

  let isLoading = $state(false);
  let savedAt = $state<number | null>(null);
  let isFresh = $state(false);
  let loadFailed = $state(false);
  let canAct = $derived(isFresh && fileActions.running === 0 && taskStore.active === null);
  let showAdvanced = $state(false);

  // Batch selection (all eligible unlocked items selected by default after scan)
  let selectedCandidateIds = $state<string[]>([]);
  let selectedKnownFolderIds = $state<string[]>([]);
  let showBatchConfirm = $state(false);

  // Inline undo/repair controls (zero browser confirm/prompt)
  let confirmUndoId = $state<number | null>(null);
  let repairingId = $state<number | null>(null);
  let repairTargetInput = $state('');

  // Guided Modal State
  let showGuidedModal = $state(false);
  let activeGuidedCandidate = $state<RelocationCandidateDto | null>(null);

  let targetDriveInfo = $derived(
    availableDrives.find((d) => d.drive_letter === targetDrive)
  );
  let targetFreeBytes = $derived(targetDriveInfo?.free_bytes ?? 0);
  let mainCandidates = $derived(candidates.filter((c) => !c.is_advanced));
  let advancedCandidates = $derived(candidates.filter((c) => c.is_advanced));

  let movableKnownFolders = $derived(
    isStoreBuild
      ? []
      : knownFolders.filter((kf) => !kf.is_onedrive && !kf.is_redirected && kf.size_bytes > 0)
  );
  let movableMainCandidates = $derived(
    mainCandidates.filter((c) => c.strategy !== 'guided' && c.is_eligible && !c.is_locked && c.size_bytes > 0)
  );

  let selectedTotalCount = $derived(selectedKnownFolderIds.length + selectedCandidateIds.length);
  let selectedTotalBytes = $derived.by(() => {
    let sum = 0;
    for (const kf of knownFolders) {
      if (selectedKnownFolderIds.includes(kf.folder_id)) sum += kf.size_bytes;
    }
    for (const c of candidates) {
      if (selectedCandidateIds.includes(c.id)) sum += c.size_bytes;
    }
    return sum;
  });

  let allMovableCount = $derived(movableKnownFolders.length + movableMainCandidates.length);
  let isAllSelected = $derived(
    allMovableCount > 0 && selectedTotalCount === allMovableCount
  );

  interface RelocateSnapshot {
    candidates: RelocationCandidateDto[];
    knownFolders: KnownFolderDto[];
    historyRecords: RelocationResultDto[];
  }

  const loadScan = createLoader<RelocateSnapshot>({
    show(data, at, fresh) {
      candidates = data?.candidates ?? [];
      knownFolders = data?.knownFolders ?? [];
      historyRecords = data?.historyRecords ?? [];
      savedAt = at;
      isFresh = fresh;

      // Auto-select all eligible unlocked main items & non-OneDrive Known Folders
      selectedKnownFolderIds = isStoreBuild
        ? []
        : knownFolders
            .filter((kf) => !kf.is_onedrive && !kf.is_redirected && kf.size_bytes > 0)
            .map((kf) => kf.folder_id);
      selectedCandidateIds = candidates
        .filter((c) => !c.is_advanced && c.strategy !== 'guided' && c.is_eligible && !c.is_locked && c.size_bytes > 0)
        .map((c) => c.id);
    },
    status(loading, failed) {
      isLoading = loading;
      loadFailed = failed;
    },
  });

  async function loadData(force = false) {
    loadScan.drop();
    isLoading = true;
    isFresh = false;
    try {
      const bootstrap = await getBootstrapState();
      isStoreBuild = Boolean(bootstrap?.is_store_build);
      if (bootstrap?.drives && bootstrap.drives.length > 0) {
        availableDrives = bootstrap.drives.filter((d) => d.is_eligible && !d.is_system);
        if (availableDrives.length > 0 && !availableDrives.some((d) => d.drive_letter === targetDrive)) {
          targetDrive = bootstrap.recommended_drive || availableDrives[0].drive_letter;
        }
      }
    } catch (err) {
      console.error('Failed to load drives:', err);
    }

    const drive = targetDrive;
    await loadScan(
      `relocate.${drive}`,
      async () => {
        try {
          await recoverOnce();
        } catch (err) {
          console.error('Crash recovery failed:', err);
        }

        const [candList, kfList, histList] = await Promise.all([
          scanRelocationCandidates(drive),
          getKnownFolders(drive),
          listRelocationHistory(),
        ]);

        return {
          candidates: candList.filter((c) => c.strategy !== 'known_folder' && c.id !== 'zalo'),
          knownFolders: kfList,
          historyRecords: histList,
        };
      },
      { force }
    );
  }

  function toggleKnownFolder(id: string) {
    if (selectedKnownFolderIds.includes(id)) {
      selectedKnownFolderIds = selectedKnownFolderIds.filter((x) => x !== id);
    } else {
      selectedKnownFolderIds = [...selectedKnownFolderIds, id];
    }
  }

  function toggleCandidate(id: string) {
    if (selectedCandidateIds.includes(id)) {
      selectedCandidateIds = selectedCandidateIds.filter((x) => x !== id);
    } else {
      selectedCandidateIds = [...selectedCandidateIds, id];
    }
  }

  function toggleSelectAll() {
    if (isAllSelected) {
      selectedKnownFolderIds = [];
      selectedCandidateIds = [];
    } else {
      selectedKnownFolderIds = movableKnownFolders.map((kf) => kf.folder_id);
      selectedCandidateIds = movableMainCandidates.map((c) => c.id);
    }
  }

  async function handleConfirmBatchMove() {
    if (selectedTotalCount === 0 || !canAct) return;
    showBatchConfirm = false;
    const cIds = [...selectedCandidateIds];
    const kfIds = [...selectedKnownFolderIds];
    const totalB = selectedTotalBytes;
    const drive = targetDrive;

    try {
      await taskStore.run(
        {
          kind: 'relocate_batch',
          title: t('Đang chuyển {count} mục sang ổ {drive}:', { count: cIds.length + kfIds.length, drive }),
          phase: 'moving',
          totalBytes: totalB,
        },
        () => relocateBatch(cIds, kfIds, drive),
        (res) => ({
          kind: res.failed_count === 0 ? 'ok' : 'info',
          title: t('Đã hoàn tất di chuyển sang ổ {drive}:', { drive: res.target_drive }),
          detail:
            res.failed_count === 0
              ? t('Đã chuyển thành công {count} mục ({size}).', {
                  count: res.succeeded_count,
                  size: formatSize(res.total_moved_bytes),
                })
              : t('Thành công {ok} mục ({size}) · Bỏ qua {fail} mục.', {
                  ok: res.succeeded_count,
                  size: formatSize(res.total_moved_bytes),
                  fail: res.failed_count,
                }),
        })
      );
    } catch {
      // Toast handled by taskStore
    }
  }

  async function handleRelocateDirectory(candidate: RelocationCandidateDto) {
    if (candidate.strategy === 'guided') {
      activeGuidedCandidate = candidate;
      showGuidedModal = true;
      return;
    }
    if (!canAct) return;

    try {
      await taskStore.run(
        {
          kind: 'relocate',
          title: t('Đang chuyển {name}', { name: candidate.name }),
          phase: 'moving',
          totalBytes: candidate.size_bytes,
          totalFiles: candidate.file_count,
        },
        () => relocateDirectory(candidate.id, targetDrive),
        (res) => ({
          kind: 'ok',
          title: t('Đã chuyển {name} sang ổ {drive}:', { name: res.app_name, drive: targetDrive }),
          detail: formatSize(res.size_bytes),
        })
      );
    } catch {}
  }

  async function handleRelocateKnownFolder(kf: KnownFolderDto) {
    if (kf.is_onedrive || isStoreBuild || !canAct) return;

    try {
      await taskStore.run(
        {
          kind: 'relocate',
          title: t('Đang chuyển {name}', { name: kf.name }),
          phase: 'moving',
          totalBytes: kf.size_bytes,
          totalFiles: kf.file_count,
        },
        () => relocateKnownFolder(kf.folder_id, targetDrive),
        (res) => ({
          kind: 'ok',
          title: t('Đã chuyển {name} sang ổ {drive}:', { name: res.app_name, drive: targetDrive }),
          detail: formatSize(res.size_bytes),
        })
      );
    } catch {}
  }

  async function executeUndo(rec: RelocationResultDto) {
    if (!rec.id) return;
    confirmUndoId = null;
    try {
      await undoRelocation(rec.id);
      taskStore.showToast({
        kind: 'ok',
        title: t('Đã hoàn tác {name} về ổ C:', { name: rec.app_name }),
      });
    } catch (err) {
      taskStore.showToast({
        kind: 'err',
        title: t('Không hoàn tác được {name}', { name: rec.app_name }),
        detail: String(err),
      });
    }
  }

  async function executeRepair(rec: RelocationResultDto) {
    if (!rec.id || !repairTargetInput.trim()) return;
    const id = rec.id;
    const target = repairTargetInput.trim();
    repairingId = null;
    try {
      await repairJunctionTarget(id, target);
      taskStore.showToast({
        kind: 'ok',
        title: t('Đã sửa liên kết thành công'),
      });
    } catch (err) {
      taskStore.showToast({
        kind: 'err',
        title: t('Lỗi sửa liên kết'),
        detail: String(err),
      });
    }
  }

  onFilesChanged(() => loadData(true));

  async function copyPath(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      taskStore.showToast({ kind: 'info', title: t('Đã sao chép đường dẫn') }, 2000);
    } catch {}
  }

  function getFolderIcon(id: string) {
    switch (id.toLowerCase()) {
      case 'downloads': return Download;
      case 'documents': return FileText;
      case 'pictures': return Image;
      case 'videos': return Video;
      case 'music': return Music;
      default: return Folder;
    }
  }

  function getStrategyBadge(strategy: string) {
    switch (strategy) {
      case 'known_folder': return { label: t('Vị trí của Windows'), class: 'bg-blue-50 text-blue-700 dark:bg-blue-950 dark:text-blue-300' };
      case 'tool_config': return { label: t('Cấu hình công cụ'), class: 'bg-purple-50 text-purple-700 dark:bg-purple-950 dark:text-purple-300' };
      case 'guided': return { label: t('Tự động qua PowerShell / App'), class: 'bg-amber-50 text-amber-700 dark:bg-amber-950 dark:text-amber-300' };
      default: return { label: t('Liên kết thư mục'), class: 'bg-emerald-50 text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300' };
    }
  }

  onMount(() => {
    loadData();
  });
</script>

<div class="flex flex-col gap-5 w-full">
  <!-- Header Bar with Drive Selector + Move All CTA -->
  <div class="rounded-2xl border border-border bg-card p-4 shadow-sm flex flex-col md:flex-row md:items-center justify-between gap-4">
    <div class="flex flex-wrap items-center gap-3">
      <div class="flex items-center gap-2 bg-subtle border border-border px-3 py-1.5 rounded-lg">
        <HardDrive class="h-4 w-4 text-accent" />
        <span class="text-xs font-medium text-secondary">{t('Ổ đích:')}</span>
        <select
          bind:value={targetDrive}
          onchange={() => loadData()}
          class="bg-transparent text-xs font-bold text-primary outline-none cursor-pointer"
        >
          {#each availableDrives as drive}
            <option value={drive.drive_letter}>{drive.drive_letter}: ({drive.label || t('Ổ đĩa cục bộ')})</option>
          {/each}
        </select>
        <span class="text-xs text-emerald-600 dark:text-emerald-400 font-medium ml-1">
          {t('Trống {free}', { free: formatSize(targetFreeBytes) })}
        </span>
      </div>

      <button
        type="button"
        onclick={() => loadData(true)}
        disabled={isLoading || !canAct}
        class="inline-flex items-center gap-1.5 bg-subtle border border-border hover:bg-card px-3 py-1.5 rounded-lg text-xs font-medium text-primary transition-colors disabled:opacity-50"
      >
        <RefreshCw class={`h-3.5 w-3.5 ${isLoading ? 'animate-spin' : ''}`} />
        {t('Phân tích')}
      </button>
    </div>

    <!-- Batch Selection & Move All -->
    <div class="flex items-center gap-2.5 flex-wrap">
      {#if allMovableCount > 0}
        <button
          type="button"
          onclick={toggleSelectAll}
          class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border bg-subtle hover:bg-card text-xs font-medium text-primary"
        >
          {#if isAllSelected}
            <CheckSquare class="h-3.5 w-3.5 text-accent" />
          {:else}
            <Square class="h-3.5 w-3.5 text-secondary" />
          {/if}
          <span>{t('Chọn tất cả ({count})', { count: allMovableCount })}</span>
        </button>
      {/if}

      <button
        type="button"
        disabled={!canAct || selectedTotalCount === 0 || availableDrives.length === 0}
        onclick={() => (showBatchConfirm = true)}
        class="inline-flex items-center gap-2 px-4 py-2 rounded-xl bg-accent hover:bg-accent-hover text-white text-xs font-bold shadow-sm disabled:opacity-50 transition-colors"
      >
        <Zap class="h-4 w-4" />
        {selectedTotalCount > 0
          ? t('Chuyển tất cả đã chọn ({count} · {size})', {
              count: selectedTotalCount,
              size: formatSize(selectedTotalBytes),
            })
          : t('Chuyển tất cả')}
      </button>
    </div>
  </div>

  <!-- Single Clean Inline Batch Confirmation Strip (no browser confirm()) -->
  {#if showBatchConfirm}
    <div class="rounded-xl border border-accent/40 bg-accent/10 p-4 flex flex-col sm:flex-row sm:items-center justify-between gap-3">
      <div class="text-xs space-y-0.5">
        <div class="font-bold text-primary">
          {t('Chuyển {count} mục ({size}) sang ổ {drive}:?', {
            count: selectedTotalCount,
            size: formatSize(selectedTotalBytes),
            drive: targetDrive,
          })}
        </div>
        <p class="text-secondary">
          {t('Dữ liệu được sao chép và kiểm tra nguyên vẹn trước khi giải phóng ổ C:. Bạn có thể ẩn chạy ngầm hoặc hoàn tác bất cứ lúc nào.')}
        </p>
      </div>
      <div class="flex items-center gap-2 shrink-0">
        <button
          type="button"
          onclick={() => (showBatchConfirm = false)}
          class="px-3 py-1.5 rounded-lg border border-border bg-card text-xs font-medium text-secondary"
        >
          {t('Hủy')}
        </button>
        <button
          type="button"
          onclick={handleConfirmBatchMove}
          class="px-4 py-1.5 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-bold"
        >
          {t('Bắt đầu chuyển ngay')}
        </button>
      </div>
    </div>
  {/if}

  <!-- Interrupted Recovery Alert Banner -->
  {#if recoveries.list.length > 0}
    <div class="rounded-xl border border-amber-300 dark:border-amber-700 bg-amber-50 dark:bg-amber-950/50 p-4 shadow-sm flex items-center justify-between">
      <div class="flex items-center gap-3">
        <AlertTriangle class="h-5 w-5 text-amber-600 dark:text-amber-400 shrink-0" />
        <div>
          {#if needsReview.length > 0}
            {#each needsReview as rec (rec.id)}
              <h4 class="text-sm font-semibold text-amber-900 dark:text-amber-200">{rec.app_name}</h4>
              <p class="text-xs text-amber-800 dark:text-amber-300 mt-0.5 break-words">{rec.detail}</p>
            {/each}
          {:else}
            <h4 class="text-sm font-semibold text-amber-900 dark:text-amber-200">
              {t('Bị gián đoạn, đã khôi phục về vị trí cũ')}
            </h4>
            <p class="text-xs text-amber-800 dark:text-amber-300 mt-0.5">
              {t('Phát hiện {count} tác vụ di chuyển chưa hoàn tất do tắt máy đột ngột. GreenC đã tự động hoàn trả an toàn.', { count: recoveries.list.length })}
            </p>
          {/if}
        </div>
      </div>
      <button
        type="button"
        onclick={() => (recoveries.list = [])}
        class="text-xs font-medium px-3 py-1.5 rounded-md bg-amber-200/60 dark:bg-amber-900/60 text-amber-900 dark:text-amber-200 hover:bg-amber-300 transition-colors"
      >
        {t('Đóng')}
      </button>
    </div>
  {/if}

  <UpdateStatus {savedAt} refreshing={isLoading} failed={loadFailed} />

  <!-- Zalo PC Card (renders ONLY when Zalo is installed AND actually has movable data on C: or active relocation/safety copy) -->
  <ZaloCard preferredDrive={targetDrive} mode="relocate" />

  <!-- Section 1: Windows Known Folders Cards -->
  <div class="rounded-xl border border-border bg-card p-5 shadow-sm">
    <div class="flex items-center justify-between mb-4">
      <div class="flex items-center gap-2">
        <FolderSync class="h-5 w-5 text-accent" />
        <h2 class="text-base font-semibold text-primary">{t('Thư mục cá nhân (Downloads, Videos, Pictures...)')}</h2>
      </div>
      <span class="text-xs text-secondary">{t('Dùng vị trí chuẩn của Windows')}</span>
    </div>

    <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
      {#each knownFolders as kf}
        {@const IconComp = getFolderIcon(kf.folder_id)}
        {@const selectable = !kf.is_onedrive && !isStoreBuild && !kf.is_redirected && kf.size_bytes > 0}
        {@const checked = selectedKnownFolderIds.includes(kf.folder_id)}
        <div
          class={`border rounded-lg p-3.5 flex flex-col justify-between transition-colors ${
            checked ? 'border-accent/50 bg-accent/5' : 'border-border/80 bg-subtle/40'
          }`}
        >
          <div>
            <div class="flex items-center justify-between mb-1.5 gap-2">
              <div class="flex items-center gap-2 min-w-0">
                {#if selectable}
                  <button
                    type="button"
                    onclick={() => toggleKnownFolder(kf.folder_id)}
                    class="text-secondary hover:text-primary shrink-0"
                  >
                    {#if checked}
                      <CheckSquare class="h-4 w-4 text-accent" />
                    {:else}
                      <Square class="h-4 w-4" />
                    {/if}
                  </button>
                {/if}
                <IconComp class="h-4 w-4 text-accent shrink-0" />
                <span class="text-sm font-semibold text-primary truncate">{kf.name}</span>
              </div>
              <span class="text-xs font-mono font-medium text-secondary shrink-0">{formatSize(kf.size_bytes)}</span>
            </div>
            <div class="text-xs text-secondary truncate" title={kf.current_path}>
              {kf.current_path}
            </div>
            {#if kf.is_onedrive}
              <div class="flex items-center gap-1 text-[11px] text-amber-600 dark:text-amber-400 mt-2 font-medium">
                <CloudOff class="h-3 w-3" />
                <span>OneDrive ({t('Bỏ qua')})</span>
              </div>
            {:else if isStoreBuild}
              <div class="text-[11px] text-secondary mt-2">
                {t('Bản Microsoft Store không hỗ trợ chuyển thư mục hệ thống tự động.')}
              </div>
            {/if}
          </div>

          <div class="mt-3 pt-2 border-t border-border/50 flex items-center justify-between">
            <span class="text-xs text-disabled">
              {kf.is_redirected ? t('Đã chuyển sang {target}', { target: targetDrive + ':\\' }) : t('Ổ C:')}
            </span>
            <button
              type="button"
              onclick={() => handleRelocateKnownFolder(kf)}
              disabled={!selectable || !canAct}
              class="text-xs font-medium px-2.5 py-1 rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-40 transition-colors"
            >
              {kf.is_redirected ? t('Đã chuyển') : t('Chuyển')}
            </button>
          </div>
        </div>
      {/each}
    </div>
  </div>

  <!-- Section 2: Application Candidates Table -->
  <div class="rounded-xl border border-border bg-card p-5 shadow-sm">
    <div class="flex items-center justify-between mb-4 gap-3 flex-wrap">
      <div>
        <h2 class="text-base font-semibold text-primary">{t('Dữ liệu ứng dụng có thể chuyển')}</h2>
        <p class="text-xs text-secondary mt-0.5">
          {t('Tích chọn nhiều mục và bấm "Chuyển tất cả" ở trên, hoặc bấm chuyển ngay từng mục.')}
        </p>
      </div>
      <div class="text-xs font-medium text-secondary">
        {t('{count} ứng dụng', { count: mainCandidates.length })}
      </div>
    </div>

    {#if mainCandidates.length === 0}
      <div class="p-8 text-center text-sm text-secondary">
        {savedAt === null
          ? isLoading
            ? t('Đang tải...')
            : t('Chưa có kết quả phân tích')
          : t('Không có dữ liệu ứng dụng lớn nào trên ổ C:.')}
      </div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-left border-collapse text-sm">
          <thead>
            <tr class="border-b border-border text-xs text-secondary uppercase font-semibold">
              <th class="py-2.5 px-2 w-8"></th>
              <th class="py-2.5 px-3">{t('Ứng dụng')}</th>
              <th class="py-2.5 px-3">{t('Cách chuyển')}</th>
              <th class="py-2.5 px-3">{t('Vị trí')}</th>
              <th class="py-2.5 px-3 text-right">{t('Dung lượng')}</th>
              <th class="py-2.5 px-3 text-right">{t('Thao tác')}</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-border/60">
            {#each mainCandidates as cand}
              {@const badge = getStrategyBadge(cand.strategy)}
              {@const selectable = cand.strategy !== 'guided' && cand.is_eligible && !cand.is_locked && cand.size_bytes > 0}
              {@const checked = selectedCandidateIds.includes(cand.id)}
              <tr class={`hover:bg-subtle/50 transition-colors ${checked ? 'bg-accent/5' : ''}`}>
                <td class="py-3 px-2">
                  {#if selectable}
                    <button
                      type="button"
                      onclick={() => toggleCandidate(cand.id)}
                      class="text-secondary hover:text-primary"
                    >
                      {#if checked}
                        <CheckSquare class="h-4 w-4 text-accent" />
                      {:else}
                        <Square class="h-4 w-4" />
                      {/if}
                    </button>
                  {/if}
                </td>
                <td class="py-3 px-3 font-semibold text-primary">
                  {cand.name}
                  {#if cand.is_locked}
                    <span class="ml-2 text-xs font-normal text-rose-500">
                      ({t('Cần đóng {app}', { app: cand.name })})
                    </span>
                  {/if}
                  {#if !cand.is_eligible && cand.ineligible_reason}
                    <div class="text-[11px] font-normal text-secondary mt-0.5">
                      {cand.ineligible_reason}
                    </div>
                  {/if}
                </td>
                <td class="py-3 px-3">
                  <span class={`inline-block px-2 py-0.5 rounded text-xs font-medium ${badge.class}`}>
                    {badge.label}
                  </span>
                </td>
                <td class="py-3 px-3 text-xs text-secondary font-mono">
                  <div class="flex items-center gap-1.5 max-w-xs truncate" title={cand.source_path}>
                    <span class="truncate">{cand.source_path}</span>
                    <button
                      type="button"
                      onclick={() => copyPath(cand.source_path)}
                      class="text-secondary hover:text-primary shrink-0"
                      title={t('Sao chép đường dẫn')}
                    >
                      <Copy class="h-3.5 w-3.5" />
                    </button>
                  </div>
                </td>
                <td class="py-3 px-3 text-right font-mono font-medium text-primary">
                  {formatSize(cand.size_bytes)}
                </td>
                <td class="py-3 px-3 text-right">
                  <button
                    type="button"
                    onclick={() => handleRelocateDirectory(cand)}
                    disabled={!canAct || cand.is_locked || (cand.strategy !== 'guided' && !cand.is_eligible)}
                    title={!cand.is_eligible && cand.ineligible_reason ? cand.ineligible_reason : undefined}
                    class="px-3 py-1 text-xs font-semibold rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-40 transition-colors"
                  >
                    {cand.strategy === 'guided'
                      ? cand.id === 'wsl2'
                        ? t('Chạy PowerShell')
                        : t('Hướng dẫn')
                      : t('Chuyển')}
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    <!-- Advanced Toggle -->
    {#if advancedCandidates.length > 0}
      <div class="mt-4 pt-3 border-t border-border">
        <button
          type="button"
          onclick={() => (showAdvanced = !showAdvanced)}
          class="flex items-center gap-1.5 text-xs font-medium text-secondary hover:text-primary transition-colors"
        >
          {#if showAdvanced}
            <ChevronUp class="h-4 w-4" />
          {:else}
            <ChevronDown class="h-4 w-4" />
          {/if}
          <span>{t('Hiện thư mục nâng cao')} ({advancedCandidates.length})</span>
        </button>

        {#if showAdvanced}
          <div class="mt-3 overflow-x-auto">
            <table class="w-full text-left border-collapse text-sm">
              <tbody class="divide-y divide-border/60">
                {#each advancedCandidates as cand}
                  <tr class="hover:bg-subtle/50 transition-colors">
                    <td class="py-2.5 px-3 font-medium text-primary">
                      {cand.name}
                      {#if !cand.is_eligible && cand.ineligible_reason}
                        <div class="text-[11px] font-normal text-secondary mt-0.5">
                          {cand.ineligible_reason}
                        </div>
                      {/if}
                    </td>
                    <td class="py-2.5 px-3 text-xs text-secondary font-mono truncate max-w-xs" title={cand.source_path}>
                      {cand.source_path}
                    </td>
                    <td class="py-2.5 px-3 text-right font-mono text-xs">{formatSize(cand.size_bytes)}</td>
                    <td class="py-2.5 px-3 text-right">
                      <button
                        type="button"
                        onclick={() => handleRelocateDirectory(cand)}
                        disabled={!canAct || cand.is_locked || !cand.is_eligible}
                        class="px-2.5 py-1 text-xs font-medium rounded-md bg-subtle hover:bg-border text-primary disabled:opacity-40 transition-colors"
                      >
                        {t('Chuyển')}
                      </button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </div>
    {/if}
  </div>

  <!-- Section 3: Relocation History ("Đã chuyển") -->
  <div class="rounded-xl border border-border bg-card p-5 shadow-sm">
    <div class="flex items-center justify-between mb-4">
      <h2 class="text-base font-semibold text-primary">{t('Đã chuyển')}</h2>
      <span class="text-xs text-secondary">{historyRecords.length} {t('mục')}</span>
    </div>

    {#if historyRecords.length === 0}
      <div class="p-6 text-center text-sm text-secondary">
        {t('Chưa chuyển mục nào.')}
      </div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-left border-collapse text-sm">
          <thead>
            <tr class="border-b border-border text-xs text-secondary uppercase font-semibold">
              <th class="py-2.5 px-3">{t('Ứng dụng')}</th>
              <th class="py-2.5 px-3">{t('Vị trí mới')}</th>
              <th class="py-2.5 px-3">{t('Trạng thái')}</th>
              <th class="py-2.5 px-3 text-right">{t('Thao tác')}</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-border/60">
            {#each historyRecords as rec}
              <tr class="hover:bg-subtle/50 transition-colors">
                <td class="py-3 px-3 font-semibold text-primary">{rec.app_name}</td>
                <td class="py-3 px-3 text-xs text-secondary font-mono truncate max-w-xs" title={rec.target_path}>
                  {rec.target_path}
                </td>
                <td class="py-3 px-3">
                  {#if rec.health_status === 'healthy'}
                    <span class="inline-flex items-center gap-1 text-xs text-emerald-600 dark:text-emerald-400 font-medium">
                      <CheckCircle2 class="h-3.5 w-3.5" />
                      {t('Hoạt động bình thường')}
                    </span>
                  {:else if rec.health_status === 'target_offline'}
                    <span class="inline-flex items-center gap-1 text-xs text-amber-600 dark:text-amber-400 font-medium">
                      <AlertTriangle class="h-3.5 w-3.5" />
                      {t('Mất kết nối đích')}
                    </span>
                  {:else}
                    <span class="inline-flex items-center gap-1 text-xs text-rose-600 dark:text-rose-400 font-medium">
                      <AlertTriangle class="h-3.5 w-3.5" />
                      {t('Liên kết hỏng')}
                    </span>
                  {/if}
                </td>
                <td class="py-3 px-3 text-right">
                  <div class="flex items-center justify-end gap-2 flex-wrap">
                    {#if rec.health_status !== 'healthy' && rec.strategy === 'junction' && rec.id}
                      {#if repairingId === rec.id}
                        <input
                          type="text"
                          bind:value={repairTargetInput}
                          class="px-2 py-1 text-xs font-mono border border-border rounded bg-subtle text-primary w-44"
                        />
                        <button
                          type="button"
                          onclick={() => executeRepair(rec)}
                          class="px-2.5 py-1 text-xs font-semibold rounded bg-accent text-white"
                        >
                          {t('Lưu')}
                        </button>
                      {:else}
                        <button
                          type="button"
                          disabled={!canAct}
                          onclick={() => {
                            repairingId = rec.id ?? null;
                            repairTargetInput = rec.target_path;
                          }}
                          class="px-2 py-1 text-xs font-medium rounded-md border border-border hover:bg-subtle text-accent"
                        >
                          <Wrench class="h-3.5 w-3.5 inline mr-1" />
                          {t('Sửa')}
                        </button>
                      {/if}
                    {/if}

                    {#if rec.id && rec.strategy !== 'guided' && rec.status !== 'undone'}
                      {#if confirmUndoId === rec.id}
                        <button
                          type="button"
                          onclick={() => (confirmUndoId = null)}
                          class="px-2 py-1 text-xs rounded border border-border text-secondary"
                        >
                          {t('Hủy')}
                        </button>
                        <button
                          type="button"
                          disabled={!canAct}
                          onclick={() => executeUndo(rec)}
                          class="px-2.5 py-1 text-xs font-semibold rounded bg-emerald-600 text-white"
                        >
                          {t('Xác nhận')}
                        </button>
                      {:else}
                        <button
                          type="button"
                          onclick={() => (confirmUndoId = rec.id ?? null)}
                          disabled={!canAct}
                          class="px-2.5 py-1 text-xs font-medium rounded-md border border-border hover:bg-subtle text-secondary hover:text-primary disabled:opacity-40"
                        >
                          <Undo2 class="h-3.5 w-3.5 inline mr-1" />
                          {t('Hoàn tác')}
                        </button>
                      {/if}
                    {/if}
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>

<!-- Guided Migration / 1-Click WSL2 PowerShell Modal -->
<GuidedModal
  open={showGuidedModal}
  candidate={activeGuidedCandidate}
  {targetDrive}
  {isStoreBuild}
  onClose={() => (showGuidedModal = false)}
  onRecheck={() => {
    showGuidedModal = false;
    loadData(true);
  }}
/>
