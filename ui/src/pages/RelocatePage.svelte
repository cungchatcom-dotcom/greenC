<!-- ui/src/pages/RelocatePage.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    scanRelocationCandidates,
    relocateDirectory,
    getKnownFolders,
    relocateKnownFolder,
    recoverInterruptedRelocations,
    listRelocationHistory,
    undoRelocation,
    repairJunctionTarget,
    revealInExplorer,
    getBootstrapState,
    listenTaskProgress,
    controlTask,
    type RelocationCandidateDto,
    type KnownFolderDto,
    type RelocationResultDto,
    type TaskProgressEvent,
    type DriveInfo
  } from '$lib/api';
  import GuidedModal from '$lib/components/GuidedModal.svelte';
  import {
    ArrowRightLeft,
    FolderSync,
    AlertTriangle,
    CheckCircle2,
    RefreshCw,
    X,
    Folder,
    HardDrive,
    ExternalLink,
    HelpCircle,
    Undo2,
    Copy,
    Check,
    CloudOff,
    Download,
    FileText,
    Image,
    Video,
    Music,
    ChevronDown,
    ChevronUp,
    Wrench
  } from 'lucide-svelte';

  // Svelte 5 Runes State
  let candidates = $state<RelocationCandidateDto[]>([]);
  let knownFolders = $state<KnownFolderDto[]>([]);
  let historyRecords = $state<RelocationResultDto[]>([]);
  let interruptedRecoveries = $state<RelocationResultDto[]>([]);
  let availableDrives = $state<DriveInfo[]>([]);
  let targetDrive = $state('D');
  let isStoreBuild = $state(false);

  let isLoading = $state(false);
  let isRelocating = $state(false);
  let showAdvanced = $state(false);
  let currentTaskId = $state<string | null>(null);
  let progressState = $state<TaskProgressEvent | null>(null);

  // Guided Modal State
  let showGuidedModal = $state(false);
  let activeGuidedCandidate = $state<RelocationCandidateDto | null>(null);
  let copyToast = $state(false);

  // Derived Values
  let targetDriveInfo = $derived(
    availableDrives.find((d) => d.drive_letter === targetDrive)
  );
  let targetFreeBytes = $derived(targetDriveInfo?.free_bytes ?? 0);
  let mainCandidates = $derived(candidates.filter((c) => !c.is_advanced));
  let advancedCandidates = $derived(candidates.filter((c) => c.is_advanced));
  let totalMovableBytes = $derived(
    candidates.reduce((sum, c) => sum + c.size_bytes, 0)
  );

  async function loadData() {
    isLoading = true;
    try {
      const bootstrap = await getBootstrapState();
      isStoreBuild = Boolean(bootstrap?.is_store_build);
      if (bootstrap?.drives && bootstrap.drives.length > 0) {
        availableDrives = bootstrap.drives.filter((d) => d.is_eligible && !d.is_system);
        if (availableDrives.length > 0 && !availableDrives.some((d) => d.drive_letter === targetDrive)) {
          targetDrive = bootstrap.recommended_drive || availableDrives[0].drive_letter;
        }
      }

      // Check crash recoveries first
      interruptedRecoveries = await recoverInterruptedRelocations();

      // Parallel fetch candidates, known folders, and history
      const [candList, kfList, histList] = await Promise.all([
        scanRelocationCandidates(targetDrive),
        getKnownFolders(targetDrive),
        listRelocationHistory()
      ]);

      // Known Folders have their own section that moves them through the Windows shell API;
      // offering them here would send them down the junction path, which the backend refuses.
      candidates = candList.filter((c) => c.strategy !== 'known_folder');
      knownFolders = kfList;
      historyRecords = histList;
    } catch (err) {
      console.error('Failed to load relocation data:', err);
    } finally {
      isLoading = false;
    }
  }

  async function handleRelocateDirectory(candidate: RelocationCandidateDto) {
    if (candidate.strategy === 'guided') {
      activeGuidedCandidate = candidate;
      showGuidedModal = true;
      return;
    }

    if (!confirm(t('Chuyển {name} ({size}) sang {target}?\n\nDữ liệu được sao chép và kiểm tra trước, sau đó mới xóa ở ổ C:. Có thể hoàn tác.', {
      name: candidate.name,
      size: formatSize(candidate.size_bytes),
      target: `${targetDrive}:\\GreenC\\`
    }))) {
      return;
    }

    isRelocating = true;
    try {
      await relocateDirectory(candidate.id, targetDrive);
      await loadData();
    } catch (err) {
      alert(t('Không chuyển được {name}', { name: candidate.name }) + ': ' + err);
    } finally {
      isRelocating = false;
      progressState = null;
      currentTaskId = null;
    }
  }

  async function handleRelocateKnownFolder(kf: KnownFolderDto) {
    if (kf.is_onedrive) {
      alert(t('Thư mục đang được đồng bộ đám mây OneDrive, không thể chuyển tự động.'));
      return;
    }

    if (!confirm(t('Chuyển {name} ({size}) sang {target}?\n\nDữ liệu được sao chép và kiểm tra trước, sau đó mới xóa ở ổ C:. Có thể hoàn tác.', {
      name: kf.name,
      size: formatSize(kf.size_bytes),
      target: `${targetDrive}:\\${kf.name}`
    }))) {
      return;
    }

    isRelocating = true;
    try {
      await relocateKnownFolder(kf.folder_id, targetDrive);
      await loadData();
    } catch (err) {
      alert(t('Không chuyển được {name}', { name: kf.name }) + ': ' + err);
    } finally {
      isRelocating = false;
      progressState = null;
      currentTaskId = null;
    }
  }

  async function handleUndo(rec: RelocationResultDto) {
    if (!rec.id) return;
    if (!confirm(t('Hoàn tác {name}', { name: rec.app_name }) + '?')) return;

    try {
      await undoRelocation(rec.id);
      await loadData();
    } catch (err) {
      alert(t('Không hoàn tác được {name}', { name: rec.app_name }) + ': ' + err);
    }
  }

  async function handleRepair(rec: RelocationResultDto) {
    if (!rec.id) return;
    const newTarget = prompt(t('Nhập đường dẫn thư mục đích mới:'), rec.target_path);
    if (!newTarget) return;

    try {
      await repairJunctionTarget(rec.id, newTarget);
      await loadData();
    } catch (err) {
      alert(t('Lỗi sửa liên kết') + ': ' + err);
    }
  }

  async function handleCancelTask() {
    if (currentTaskId) {
      await controlTask(currentTaskId, 'cancel');
    }
  }

  async function copyPath(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copyToast = true;
      setTimeout(() => (copyToast = false), 1500);
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
      case 'guided': return { label: t('Tùy chọn của ứng dụng'), class: 'bg-amber-50 text-amber-700 dark:bg-amber-950 dark:text-amber-300' };
      default: return { label: t('Liên kết thư mục'), class: 'bg-emerald-50 text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300' };
    }
  }

  onMount(() => {
    loadData();

    const unlistenPromise = listenTaskProgress((event) => {
      if (event.kind === 'relocate' || event.kind === 'relocate_scan') {
        progressState = event;
        currentTaskId = event.task_id;
      }
    });

    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  });
</script>

<div class="flex flex-col gap-6 p-6 max-w-7xl mx-auto w-full">
  <!-- Header Bar -->
  <div class="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4">
    <div>
      <h1 class="text-2xl font-bold tracking-tight text-primary flex items-center gap-2">
        <ArrowRightLeft class="h-6 w-6 text-accent" />
        {t('Di chuyển dữ liệu')}
      </h1>
      <p class="text-sm text-secondary mt-1">
        {t('Chuyển thư mục và dữ liệu ứng dụng sang ổ đĩa khác để giải phóng ổ C: mà không làm hỏng ứng dụng.')}
      </p>
    </div>

    <!-- Drive Selector and Free Space Badge -->
    <div class="flex items-center gap-3">
      <div class="flex items-center gap-2 bg-card border border-border px-3 py-1.5 rounded-lg shadow-sm">
        <HardDrive class="h-4 w-4 text-secondary" />
        <span class="text-xs font-medium text-secondary">{t('Đích:')}</span>
        <select
          bind:value={targetDrive}
          onchange={() => loadData()}
          class="bg-transparent text-sm font-semibold text-primary outline-none cursor-pointer"
        >
          {#each availableDrives as drive}
            <option value={drive.drive_letter}>{drive.drive_letter}: ({drive.label || t('Ổ đĩa cục bộ')})</option>
          {/each}
        </select>
        <span class="text-xs text-emerald-600 dark:text-emerald-400 font-medium ml-2">
          {t('Còn trống {free}', { free: formatSize(targetFreeBytes) })}
        </span>
      </div>

      <button
        onclick={loadData}
        disabled={isLoading || isRelocating}
        class="flex items-center gap-2 bg-card border border-border hover:bg-subtle px-3 py-1.5 rounded-lg text-sm font-medium text-primary shadow-sm transition-colors"
      >
        <RefreshCw class={`h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
        {t('Phân tích')}
      </button>
    </div>
  </div>

  <!-- Interrupted Recovery Alert Banner -->
  {#if interruptedRecoveries.length > 0}
    <div class="rounded-xl border border-amber-300 dark:border-amber-700 bg-amber-50 dark:bg-amber-950/50 p-4 shadow-sm flex items-center justify-between">
      <div class="flex items-center gap-3">
        <AlertTriangle class="h-5 w-5 text-amber-600 dark:text-amber-400 shrink-0" />
        <div>
          <h4 class="text-sm font-semibold text-amber-900 dark:text-amber-200">
            {t('Bị gián đoạn, đã khôi phục về vị trí cũ')}
          </h4>
          <p class="text-xs text-amber-800 dark:text-amber-300 mt-0.5">
            {t('Phát hiện {count} tác vụ di chuyển chưa hoàn tất do tắt máy đột ngột. GreenC đã tự động hoàn trả an toàn.', { count: interruptedRecoveries.length })}
          </p>
        </div>
      </div>
      <button
        onclick={() => (interruptedRecoveries = [])}
        class="text-xs font-medium px-3 py-1.5 rounded-md bg-amber-200/60 dark:bg-amber-900/60 text-amber-900 dark:text-amber-200 hover:bg-amber-300 transition-colors"
      >
        {t('Đóng')}
      </button>
    </div>
  {/if}

  <!-- Section 1: Windows Known Folders Cards -->
  <div class="rounded-xl border border-border bg-card p-5 shadow-sm">
    <div class="flex items-center justify-between mb-4">
      <div class="flex items-center gap-2">
        <FolderSync class="h-5 w-5 text-accent" />
        <h2 class="text-base font-semibold text-primary">{t('Vị trí của Windows')}</h2>
      </div>
      <span class="text-xs text-secondary">{t('Dùng tính năng Vị trí của Windows')}</span>
    </div>

    <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
      {#each knownFolders as kf}
        {@const IconComp = getFolderIcon(kf.folder_id)}
        <div class="border border-border/80 bg-subtle/50 rounded-lg p-3.5 flex flex-col justify-between hover:border-border transition-colors">
          <div>
            <div class="flex items-center justify-between mb-1.5">
              <div class="flex items-center gap-2">
                <IconComp class="h-4 w-4 text-accent" />
                <span class="text-sm font-semibold text-primary">{kf.name}</span>
              </div>
              <span class="text-xs font-mono font-medium text-secondary">{formatSize(kf.size_bytes)}</span>
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
                {t('Bản Microsoft Store không hỗ trợ chuyển thư mục hệ thống tự động. Hãy đổi trong File Explorer → Properties → Location.')}
              </div>
            {/if}
          </div>

          <div class="mt-3 pt-2 border-t border-border/50 flex items-center justify-between">
            <span class="text-xs text-disabled">
              {kf.is_redirected ? t('Đã chuyển sang {target}', { target: targetDrive + ':\\' }) : t('Ổ C: (Mặc định)')}
            </span>
            <button
              onclick={() => handleRelocateKnownFolder(kf)}
              disabled={kf.is_onedrive || isStoreBuild || isRelocating || kf.is_redirected}
              class="text-xs font-medium px-2.5 py-1 rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors"
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
    <div class="flex items-center justify-between mb-4">
      <div>
        <h2 class="text-base font-semibold text-primary">{t('Có thể chuyển')}</h2>
        <p class="text-xs text-secondary mt-0.5">
          {t('Dữ liệu được sao chép và kiểm tra trước, sau đó mới xóa ở ổ C:. Có thể hoàn tác.')}
        </p>
      </div>
      <div class="text-xs font-medium text-secondary">
        {t('{count} ứng dụng · có thể giải phóng {size}', { count: mainCandidates.length, size: formatSize(totalMovableBytes) })}
      </div>
    </div>

    {#if mainCandidates.length === 0}
      <div class="p-8 text-center text-sm text-secondary">
        {t('Không có dữ liệu ứng dụng lớn nào trên ổ C:.')}
      </div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-left border-collapse text-sm">
          <thead>
            <tr class="border-b border-border text-xs text-secondary uppercase font-semibold">
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
              <tr class="hover:bg-subtle/50 transition-colors">
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
                    onclick={() => handleRelocateDirectory(cand)}
                    disabled={isRelocating || cand.is_locked || (cand.strategy !== 'guided' && !cand.is_eligible)}
                    title={!cand.is_eligible && cand.ineligible_reason ? cand.ineligible_reason : undefined}
                    class="px-3 py-1 text-xs font-semibold rounded-md bg-accent text-white hover:bg-accent-hover disabled:opacity-50 transition-colors"
                  >
                    {cand.strategy === 'guided' ? t('Hướng dẫn') : t('Chuyển')}
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
                        onclick={() => handleRelocateDirectory(cand)}
                        disabled={isRelocating || cand.is_locked || !cand.is_eligible}
                        title={!cand.is_eligible && cand.ineligible_reason ? cand.ineligible_reason : undefined}
                        class="px-2.5 py-1 text-xs font-medium rounded-md bg-subtle hover:bg-border text-primary disabled:opacity-50 transition-colors"
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
      <div class="p-8 text-center text-sm text-secondary">
        {t('Chưa chuyển mục nào.')}
      </div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-left border-collapse text-sm">
          <thead>
            <tr class="border-b border-border text-xs text-secondary uppercase font-semibold">
              <th class="py-2.5 px-3">{t('Ứng dụng')}</th>
              <th class="py-2.5 px-3">{t('Vị trí mới')}</th>
              <th class="py-2.5 px-3">{t('Cách chuyển')}</th>
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
                <td class="py-3 px-3 text-xs text-secondary">{rec.strategy}</td>
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
                <td class="py-3 px-3 text-right flex items-center justify-end gap-2">
                  {#if rec.health_status !== 'healthy' && rec.strategy === 'junction'}
                    <button
                      onclick={() => handleRepair(rec)}
                      class="px-2 py-1 text-xs font-medium rounded-md border border-border hover:bg-subtle text-accent transition-colors"
                      title={t('Sửa liên kết')}
                    >
                      <Wrench class="h-3.5 w-3.5 inline mr-1" />
                      {t('Sửa')}
                    </button>
                  {/if}
                  <button
                    onclick={() => handleUndo(rec)}
                    disabled={rec.strategy === 'guided' || rec.status === 'undone'}
                    class="px-2.5 py-1 text-xs font-medium rounded-md border border-border hover:bg-subtle text-secondary hover:text-primary transition-colors disabled:opacity-40"
                    title={rec.strategy === 'guided' ? t('Đổi lại vị trí lưu trong Cài đặt của ứng dụng') : t('Hoàn tác')}
                  >
                    <Undo2 class="h-3.5 w-3.5 inline mr-1" />
                    {t('Hoàn tác')}
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>

<!-- Real-time Progress Modal Overlay -->
{#if isRelocating && progressState}
  <div class="fixed inset-0 z-50 bg-black/40 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-xl border border-border bg-card p-6 shadow-2xl max-w-md w-full max-h-full overflow-y-auto">
      <div class="flex items-center justify-between mb-4">
        <h3 class="font-bold text-base text-primary flex items-center gap-2">
          <ArrowRightLeft class="h-5 w-5 text-accent animate-pulse" />
          {progressState.title || t('Đang sao chép dữ liệu')}
        </h3>
        <button onclick={handleCancelTask} class="text-xs text-rose-500 hover:text-rose-600 font-medium">
          {t('Hủy')}
        </button>
      </div>

      <div class="w-full bg-subtle rounded-full h-2.5 mb-3 overflow-hidden">
        <div class="bg-accent h-2.5 rounded-full transition-all duration-150" style={`width: ${progressState.percent}%`}></div>
      </div>

      <div class="flex items-center justify-between text-xs text-secondary mb-2 font-mono">
        <span>{progressState.percent}%</span>
        <span>{formatSize(progressState.done_bytes)} / {formatSize(progressState.total_bytes)}</span>
      </div>

      <p class="text-xs text-secondary truncate font-mono bg-subtle p-2 rounded">
        {progressState.detail}
      </p>
    </div>
  </div>
{/if}

<!-- Guided Migration Modal -->
<GuidedModal
  open={showGuidedModal}
  candidate={activeGuidedCandidate}
  {targetDrive}
  onClose={() => (showGuidedModal = false)}
  onRecheck={loadData}
/>
