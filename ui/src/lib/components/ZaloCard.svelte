<!-- ui/src/lib/components/ZaloCard.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    getZaloStatus,
    cleanZaloNow,
    moveZaloData,
    purgeZaloRetainedOriginal,
    restoreZaloRetainedOriginal,
    undoRelocation,
    getBootstrapState,
    listenTaskProgress,
    controlTask,
    type ZaloStatusDto,
    type ZaloCleanupResultDto,
    type ZaloSafetyCopyDto,
    type DriveInfo,
    type TaskProgressEvent,
    type RelocationCandidateDto,
  } from '$lib/api';
  import GuidedModal from '$lib/components/GuidedModal.svelte';
  import {
    MessageSquare,
    Trash2,
    ArrowRightLeft,
    RefreshCw,
    CheckCircle2,
    AlertTriangle,
    Undo2,
    HelpCircle,
    X,
  } from 'lucide-svelte';

  interface Props {
    preferredDrive?: string;
    onChanged?: () => void;
  }

  let { preferredDrive = undefined, onChanged = undefined }: Props = $props();

  let status = $state<ZaloStatusDto | null>(null);
  let drives = $state<DriveInfo[]>([]);
  let selectedDrive = $state('D');
  let isLoading = $state(false);
  let isCleaning = $state(false);
  let isMoving = $state(false);
  let isUndoing = $state(false);
  let busyOriginalId = $state<number | null>(null);

  $effect(() => {
    if (preferredDrive && !isMoving) {
      selectedDrive = preferredDrive;
    }
  });

  let showConfirmModal = $state(false);
  let reopenAfterMove = $state(true);
  let showGuidedFallback = $state(false);

  let currentTaskId = $state<string | null>(null);
  let moveProgress = $state<TaskProgressEvent | null>(null);
  let cleanResult = $state<ZaloCleanupResultDto | null>(null);
  let feedback = $state<{ kind: 'ok' | 'err'; text: string; offerGuide?: boolean } | null>(null);

  const moveSteps = [
    { key: 'preparing', label: 'Chuẩn bị' },
    { key: 'closing_zalo', label: 'Đóng Zalo' },
    { key: 'moving', label: 'Đang chuyển' },
    { key: 'verifying', label: 'Kiểm tra' },
    { key: 'reopening', label: 'Mở lại' },
    { key: 'done', label: 'Xong' },
  ];

  function normalizedStepIndex(phase: string | undefined): number {
    switch (phase) {
      case 'preparing':
        return 0;
      case 'closing_zalo':
        return 1;
      case 'moving':
      case 'linking':
        return 2;
      case 'verifying':
      case 'health_check':
      case 'rolling_back':
        return 3;
      case 'reopening':
        return 4;
      case 'done':
        return 5;
      default:
        return 0;
    }
  }

  let guidedCandidate = $derived<RelocationCandidateDto | null>(
    status
      ? {
          id: 'zalo',
          name: 'Zalo PC',
          strategy: 'guided',
          source_path: status.data_dir ?? '',
          target_path: `${selectedDrive}:\\GreenC\\Zalo PC`,
          size_bytes: status.chat_data_bytes,
          file_count: 0,
          is_locked: status.zalo_running,
          locked_processes: [],
          how_description: '',
          is_advanced: false,
          is_eligible: true,
          ineligible_reason: null,
        }
      : null
  );

  async function refresh() {
    isLoading = true;
    try {
      const [bootRes, zaloRes] = await Promise.allSettled([
        getBootstrapState(),
        getZaloStatus(preferredDrive || selectedDrive),
      ]);
      if (bootRes.status === 'fulfilled') {
        drives = (bootRes.value.drives ?? []).filter((d) => d.is_eligible && !d.is_system);
        const rec = preferredDrive || bootRes.value.recommended_drive || drives[0]?.drive_letter;
        if (rec && (!drives.length || drives.some((d) => d.drive_letter === rec))) {
          selectedDrive = rec;
        } else if (drives.length > 0) {
          selectedDrive = drives[0].drive_letter;
        }
      }
      if (zaloRes.status === 'fulfilled') {
        status = zaloRes.value;
        if (!preferredDrive && zaloRes.value.recommended_drive) {
          selectedDrive = zaloRes.value.recommended_drive;
        }
      }
    } finally {
      isLoading = false;
    }
  }

  async function handleCleanNow() {
    if (isCleaning || isMoving) return;
    isCleaning = true;
    feedback = null;
    cleanResult = null;
    try {
      const res = await cleanZaloNow(true);
      cleanResult = res;
      if (res.cleaned_files > 0) {
        const netFreed = Math.max(0, res.net_freed_bytes);
        let text = t('Đã dọn {size} từ bộ đệm và bản cũ Zalo.', {
          size: formatSize(netFreed),
        });
        if (res.vault_same_drive && res.vault_bytes > 0 && netFreed === 0) {
          text = `${text} ${t('Kho dự phòng đang ở thư mục tạm trên ổ C:. Chọn một ổ khác để giải phóng ổ C:.')}`;
        }
        feedback = { kind: 'ok', text };
      } else if (res.zalo_running && res.locked_skipped_bytes > 0) {
        feedback = {
          kind: 'ok',
          text: t('Còn {size} sẽ dọn được khi đóng Zalo', {
            size: formatSize(res.locked_skipped_bytes),
          }),
        };
      } else if (res.locked_notice) {
        feedback = { kind: 'ok', text: res.locked_notice };
      } else {
        feedback = { kind: 'ok', text: t('Zalo hiện không có tệp bộ đệm hoặc bản cũ cần dọn.') };
      }
      await refresh();
      onChanged?.();
    } catch (err) {
      feedback = { kind: 'err', text: String(err) };
    } finally {
      isCleaning = false;
    }
  }

  function handleClickMove() {
    feedback = null;
    if (!status) return;
    if (!status.auto_move_supported) {
      showGuidedFallback = true;
      return;
    }
    showConfirmModal = true;
  }

  async function handleConfirmMove() {
    showConfirmModal = false;
    isMoving = true;
    feedback = null;
    currentTaskId = null;
    moveProgress = {
      task_id: '',
      kind: 'zalo_move',
      state: 'running',
      percent: 2,
      phase: 'preparing',
      title: t('Chuẩn bị'),
      detail: '',
      done_files: 0,
      total_files: 0,
      done_bytes: 0,
      total_bytes: status?.chat_data_bytes ?? 0,
      throughput_bps: 0,
      eta_seconds: 0,
    };

    try {
      const res = await moveZaloData(selectedDrive, reopenAfterMove);
      feedback = {
        kind: 'ok',
        text: res.already_on_other_drive
          ? t('Đã ở ổ {drive}, không cần chuyển.', { drive: res.target_drive })
          : t('Đã chuyển dữ liệu Zalo sang ổ {drive}: thành công.', { drive: selectedDrive }),
      };
      await refresh();
      onChanged?.();
    } catch (err) {
      feedback = {
        kind: 'err',
        text: String(err),
        offerGuide: true,
      };
    } finally {
      isMoving = false;
      moveProgress = null;
      currentTaskId = null;
    }
  }

  async function handleUndoMove() {
    if (!status?.active_relocation_id || isUndoing) return;
    if (!confirm(t('Đưa dữ liệu chat Zalo trở về vị trí ban đầu?'))) return;
    isUndoing = true;
    feedback = null;
    try {
      await undoRelocation(status.active_relocation_id);
      feedback = { kind: 'ok', text: t('Đã hoàn tác dữ liệu Zalo về vị trí cũ.') };
      await refresh();
      onChanged?.();
    } catch (err) {
      feedback = { kind: 'err', text: String(err) };
    } finally {
      isUndoing = false;
    }
  }

  async function handleCancelMove() {
    if (currentTaskId) {
      await controlTask(currentTaskId, 'cancel');
    }
  }

  function formatCopyDate(iso: string | null): string {
    if (!iso) return t('Không rõ ngày');
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString('vi-VN', {
      day: '2-digit',
      month: '2-digit',
      year: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  function safetyCopyLabel(kind: string): string {
    switch (kind) {
      case 'original':
        return t('Bản gốc giữ tạm');
      case 'conflict':
        return t('Thư mục xung đột');
      case 'moving':
        return t('Đang chuyển dở');
      default:
        return kind;
    }
  }

  async function handlePurgeOriginal(copy: ZaloSafetyCopyDto) {
    if (copy.id == null || busyOriginalId !== null) return;
    const ok = confirm(
      t(
        'Xoá vĩnh viễn bản gốc đang giữ tạm ({size}) để giải phóng ổ C:? Chỉ nên xoá khi bạn đã mở Zalo và thấy đầy đủ tin nhắn.',
        { size: formatSize(copy.size_bytes) }
      )
    );
    if (!ok) return;
    busyOriginalId = copy.id;
    feedback = null;
    try {
      const msg = await purgeZaloRetainedOriginal(copy.id);
      feedback = { kind: 'ok', text: msg };
      await refresh();
      onChanged?.();
    } catch (err) {
      feedback = { kind: 'err', text: String(err) };
    } finally {
      busyOriginalId = null;
    }
  }

  async function handleRestoreOriginal(copy: ZaloSafetyCopyDto) {
    if (copy.id == null || busyOriginalId !== null) return;
    const ok = confirm(
      t(
        'Khôi phục bản gốc ({size}) về vị trí cũ trên ổ C:? Dữ liệu mới ở ổ đích (nếu có) vẫn được giữ lại thành bản riêng để không mất tin nhắn.',
        { size: formatSize(copy.size_bytes) }
      )
    );
    if (!ok) return;
    busyOriginalId = copy.id;
    feedback = null;
    try {
      const msg = await restoreZaloRetainedOriginal(copy.id);
      feedback = { kind: 'ok', text: msg };
      await refresh();
      onChanged?.();
    } catch (err) {
      feedback = { kind: 'err', text: String(err) };
    } finally {
      busyOriginalId = null;
    }
  }

  onMount(() => {
    refresh();
    const unlistenPromise = listenTaskProgress((event) => {
      if (isMoving && event.kind === 'zalo_move') {
        moveProgress = event;
        currentTaskId = event.task_id;
      }
    });
    return () => {
      unlistenPromise.then((u) => u());
    };
  });
</script>

{#if status?.installed || (status?.safety_copies && status.safety_copies.length > 0)}
  <section class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-4">
    <div class="flex items-start justify-between gap-3 flex-wrap">
      <div class="flex items-center gap-2.5">
        <div class="h-9 w-9 rounded-lg bg-blue-500/10 text-blue-600 dark:text-blue-400 flex items-center justify-center shrink-0">
          <MessageSquare class="h-5 w-5" />
        </div>
        <div>
          <h2 class="text-base font-bold text-primary">Zalo PC</h2>
          <p class="text-xs text-secondary">
            {status && !status.auto_move_supported
              ? t('Dùng tùy chọn lưu trữ của Zalo, có hướng dẫn từng bước')
              : t('Dọn bộ đệm, xóa bản cũ hoặc chuyển dữ liệu chat sang ổ khác an toàn.')}
          </p>
        </div>
      </div>

      <button
        type="button"
        onclick={refresh}
        disabled={isLoading || isCleaning || isMoving}
        class="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg border border-border bg-subtle text-xs font-medium text-secondary hover:text-primary disabled:opacity-50"
      >
        <RefreshCw class={`h-3.5 w-3.5 ${isLoading ? 'animate-spin' : ''}`} />
        {t('Kiểm tra lại')}
      </button>
    </div>

    <!-- 3 clear size rows -->
    <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
      <!-- Row 1: Chat data -->
      <div class="rounded-lg border border-border/80 bg-subtle/40 p-3.5 space-y-1">
        <div class="text-xs font-medium text-secondary">{t('Dữ liệu chat')}</div>
        <div class="text-lg font-bold text-primary tabular-numbers">
          {formatSize(status.chat_data_bytes)}
        </div>
        {#if status.already_on_other_drive}
          <div class="text-[11px] text-emerald-600 dark:text-emerald-400 font-medium">
            {t('Đã ở ổ {drive}, không cần chuyển.', { drive: status.data_drive ?? '' })}
          </div>
        {:else if status.active_relocation_target}
          <div class="text-[11px] text-emerald-600 dark:text-emerald-400 font-medium truncate" title={status.active_relocation_target}>
            {t('Đã chuyển sang {target}', { target: status.active_relocation_target })}
          </div>
        {:else if status.data_drive}
          <div class="text-[11px] text-secondary">
            {t('Tin nhắn và ảnh trên ổ {drive}', { drive: status.data_drive })}
          </div>
        {:else}
          <div class="text-[11px] text-secondary">
            {t('Không đọc được vị trí dữ liệu chat')}
          </div>
        {/if}
      </div>

      <!-- Row 2: Cache -->
      <div class="rounded-lg border border-border/80 bg-subtle/40 p-3.5 space-y-1">
        <div class="text-xs font-medium text-secondary">{t('Bộ đệm')}</div>
        <div class="text-lg font-bold text-primary tabular-numbers">
          {formatSize(status.cache_bytes)}
        </div>
        {#if status.zalo_running && status.total_locked_bytes > 0}
          <div class="text-[11px] text-amber-600 dark:text-amber-400 font-medium">
            {t('Còn {size} sẽ dọn được khi đóng Zalo', { size: formatSize(status.total_locked_bytes) })}
          </div>
        {:else}
          <div class="text-[11px] text-secondary">
            {t('Dọn được ngay: {size}', { size: formatSize(status.cache_reclaimable_bytes) })}
          </div>
        {/if}
      </div>

      <!-- Row 3: Old versions -->
      <div class="rounded-lg border border-border/80 bg-subtle/40 p-3.5 space-y-1">
        <div class="text-xs font-medium text-secondary">{t('Bản cũ')}</div>
        <div class="text-lg font-bold text-primary tabular-numbers">
          {formatSize(status.old_versions_bytes)}
        </div>
        <div class="text-[11px] text-secondary">
          {#if status.old_versions_count > 0}
            {t('{count} phiên bản cũ có thể dọn', { count: status.old_versions_count })}
          {:else}
            {t('Không có bản cũ cần dọn')}
          {/if}
        </div>
      </div>
    </div>

    <!-- Action bar -->
    <div class="flex flex-wrap items-center gap-2.5 pt-1">
      <button
        type="button"
        onclick={handleCleanNow}
        disabled={isCleaning || isMoving || status.total_reclaimable_bytes === 0}
        class="inline-flex items-center gap-2 px-4 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm disabled:opacity-50 transition-colors"
      >
        <Trash2 class="h-3.5 w-3.5" />
        {isCleaning ? t('Đang dọn...') : t('Dọn ngay')}
      </button>

      {#if status.active_relocation_id}
        <button
          type="button"
          onclick={handleUndoMove}
          disabled={isUndoing || isMoving}
          class="inline-flex items-center gap-2 px-4 py-2 rounded-lg border border-border bg-subtle hover:bg-card text-primary text-xs font-semibold disabled:opacity-50 transition-colors"
        >
          <Undo2 class="h-3.5 w-3.5" />
          {isUndoing ? t('Đang hoàn tác...') : t('Hoàn tác')}
        </button>
      {:else if status.data_dir && status.data_on_system_drive}
        <button
          type="button"
          onclick={handleClickMove}
          disabled={isMoving || isCleaning || (status.auto_move_supported && drives.length === 0)}
          class="inline-flex items-center gap-2 px-4 py-2 rounded-lg border border-accent text-accent hover:bg-accent/10 text-xs font-semibold disabled:opacity-50 transition-colors"
        >
          <ArrowRightLeft class="h-3.5 w-3.5" />
          {t('Chuyển sang ổ khác')}
        </button>
      {/if}

      {#if !status.auto_move_supported && status.auto_move_unsupported_reason && status.data_on_system_drive}
        <span class="text-[11px] text-secondary">
          {status.auto_move_unsupported_reason}
        </span>
      {/if}
    </div>

    {#if status.safety_copies && status.safety_copies.length > 0}
      <div class="rounded-lg border border-amber-500/35 bg-amber-500/10 p-3.5 space-y-2.5">
        <div class="flex items-center justify-between gap-2 flex-wrap">
          <div class="text-xs font-semibold text-amber-800 dark:text-amber-300">
            {t('Bản gốc đang giữ tạm')}
            {#if status.retained_original_bytes > 0}
              ({formatSize(status.retained_original_bytes)})
            {/if}
          </div>
          <span class="text-[11px] text-amber-700/90 dark:text-amber-300/90">
            {t('Chỉ tự xoá sau 24 giờ khi liên kết ổn định, hoặc khi bạn bấm Xoá bản gốc.')}
          </span>
        </div>

        <div class="space-y-2">
          {#each status.safety_copies as copy}
            <div class="rounded-md border border-border/80 bg-card/80 px-3 py-2 flex items-center justify-between gap-3 flex-wrap">
              <div class="min-w-0 space-y-0.5">
                <div class="flex items-center gap-2 text-xs font-semibold text-primary flex-wrap">
                  <span class="px-1.5 py-0.5 rounded bg-amber-500/20 text-amber-800 dark:text-amber-200 text-[10px]">
                    {safetyCopyLabel(copy.kind)}
                  </span>
                  <span class="tabular-numbers">{formatSize(copy.size_bytes)}</span>
                  <span class="text-[11px] font-normal text-secondary">
                    • {formatCopyDate(copy.created_at)}
                  </span>
                </div>
                <div class="text-[11px] text-secondary font-mono truncate" title={copy.path}>
                  {copy.path}
                </div>
                {#if copy.purge_after && copy.kind === 'original'}
                  <div class="text-[11px] text-secondary">
                    {copy.health_verified
                      ? t('Dự kiến tự dọn sau: {time}', { time: formatCopyDate(copy.purge_after) })
                      : t('Cần bạn xác nhận thủ công trước khi xoá')}
                  </div>
                {/if}
              </div>

              {#if copy.id !== null && copy.kind === 'original' && copy.exists_on_disk}
                <div class="flex items-center gap-1.5 shrink-0">
                  <button
                    type="button"
                    onclick={() => handleRestoreOriginal(copy)}
                    disabled={busyOriginalId !== null || isMoving}
                    class="inline-flex items-center gap-1 px-2.5 py-1.5 rounded-md border border-border bg-subtle hover:bg-card text-xs font-medium text-primary disabled:opacity-50"
                  >
                    <Undo2 class="h-3.5 w-3.5" />
                    {busyOriginalId === copy.id ? t('Đang xử lý...') : t('Khôi phục')}
                  </button>
                  <button
                    type="button"
                    onclick={() => handlePurgeOriginal(copy)}
                    disabled={busyOriginalId !== null || isMoving}
                    class="inline-flex items-center gap-1 px-2.5 py-1.5 rounded-md bg-rose-600 hover:bg-rose-700 text-white text-xs font-semibold disabled:opacity-50"
                  >
                    <Trash2 class="h-3.5 w-3.5" />
                    {t('Xoá bản gốc')}
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}

    {#if feedback}
      <div
        class={`rounded-lg border px-3.5 py-2.5 text-xs flex items-center justify-between gap-3 flex-wrap ${
          feedback.kind === 'ok'
            ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
            : 'border-rose-500/30 bg-rose-500/10 text-rose-700 dark:text-rose-300'
        }`}
      >
        <div class="flex items-center gap-2 min-w-0">
          {#if feedback.kind === 'ok'}
            <CheckCircle2 class="h-4 w-4 shrink-0" />
          {:else}
            <AlertTriangle class="h-4 w-4 shrink-0" />
          {/if}
          <span>{feedback.text}</span>
        </div>

        {#if feedback.offerGuide}
          <div class="flex items-center gap-2 shrink-0">
            <button
              type="button"
              onclick={handleClickMove}
              class="px-2.5 py-1 rounded-md bg-accent text-white font-semibold"
            >
              {t('Thử lại')}
            </button>
            <button
              type="button"
              onclick={() => (showGuidedFallback = true)}
              class="inline-flex items-center gap-1 px-2.5 py-1 rounded-md border border-border bg-card text-primary font-medium"
            >
              <HelpCircle class="h-3.5 w-3.5" />
              {t('Hướng dẫn')}
            </button>
          </div>
        {/if}
      </div>
    {/if}

    {#if cleanResult && cleanResult.zalo_running && cleanResult.locked_skipped_bytes > 0}
      {@const lockedMsg = t('Còn {size} sẽ dọn được khi đóng Zalo', {
        size: formatSize(cleanResult.locked_skipped_bytes),
      })}
      {#if feedback?.text !== lockedMsg}
        <p class="text-xs text-amber-600 dark:text-amber-400">
          {lockedMsg}
        </p>
      {/if}
    {/if}
  </section>
{/if}

<!-- One-time plain-language confirmation dialog -->
{#if showConfirmModal && status}
  <div class="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-xl border border-border bg-card p-6 shadow-2xl max-w-md w-full space-y-4">
      <div class="flex items-center justify-between">
        <h3 class="text-base font-bold text-primary">{t('Chuyển sang ổ khác')}</h3>
        <button
          type="button"
          onclick={() => (showConfirmModal = false)}
          class="text-secondary hover:text-primary"
        >
          <X class="h-4 w-4" />
        </button>
      </div>

      <p class="text-sm text-primary leading-relaxed">
        {t(
          'GreenC sẽ đóng Zalo, chuyển {size} dữ liệu chat sang {drive}:, rồi mở lại Zalo. Bản gốc sẽ được giữ tạm 24 giờ trên ổ C: để đảm bảo an toàn, tin nhắn và ảnh giữ nguyên, có thể hoàn tác.',
          {
            size: formatSize(status.chat_data_bytes),
            drive: selectedDrive,
          }
        )}
      </p>

      {#if status.space_estimate}
        <div
          class={`rounded-lg border px-3 py-2 text-xs space-y-1 ${
            status.space_estimate.can_proceed
              ? 'border-border bg-subtle/60 text-secondary'
              : 'border-rose-500/40 bg-rose-500/10 text-rose-700 dark:text-rose-300'
          }`}
        >
          <div>
            {t('Ổ nguồn {drive}: còn trống {free} (giữ tạm bản gốc {retained} trong 24 giờ)', {
              drive: status.space_estimate.source_drive,
              free: formatSize(status.space_estimate.source_free_bytes),
              retained: formatSize(status.space_estimate.source_retained_bytes),
            })}
          </div>
          <div>
            {t('Ổ đích {drive}: còn trống {free} (cần tối thiểu {need})', {
              drive: status.space_estimate.target_drive,
              free: formatSize(status.space_estimate.target_free_bytes),
              need: formatSize(status.space_estimate.target_required_bytes),
            })}
          </div>
          {#if status.space_estimate.blocking_reason}
            <div class="font-semibold">{status.space_estimate.blocking_reason}</div>
          {:else if status.space_estimate.source_notice}
            <div class="text-amber-700 dark:text-amber-300">{status.space_estimate.source_notice}</div>
          {/if}
        </div>
      {/if}

      {#if drives.length > 0}
        <div class="space-y-1">
          <label for="zalo-target-drive" class="text-xs font-medium text-secondary">
            {t('Chọn ổ đĩa đích')}
          </label>
          <select
            id="zalo-target-drive"
            bind:value={selectedDrive}
            class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-sm font-semibold text-primary"
          >
            {#each drives as d}
              <option value={d.drive_letter}>
                {d.drive_letter}: {d.label || ''} ({formatSize(d.free_bytes)} {t('trống')})
              </option>
            {/each}
          </select>
        </div>
      {/if}

      <label class="flex items-center gap-2 text-xs text-primary cursor-pointer">
        <input
          type="checkbox"
          bind:checked={reopenAfterMove}
          class="h-4 w-4 rounded border-border text-accent"
        />
        <span>{t('Mở lại Zalo sau khi chuyển xong')}</span>
      </label>

      <div class="flex justify-end gap-2 pt-2">
        <button
          type="button"
          onclick={() => (showConfirmModal = false)}
          class="px-4 py-2 rounded-lg border border-border bg-subtle text-xs font-medium text-primary"
        >
          {t('Hủy')}
        </button>
        <button
          type="button"
          onclick={handleConfirmMove}
          disabled={Boolean(status.space_estimate && !status.space_estimate.can_proceed)}
          class="px-4 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold disabled:opacity-50"
        >
          {t('Chuyển sang ổ khác')}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Clear 6-step progress dialog -->
{#if isMoving && moveProgress}
  {@const activeIdx = normalizedStepIndex(moveProgress.phase)}
  <div class="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm flex items-center justify-center p-4">
    <div class="rounded-xl border border-border bg-card p-6 shadow-2xl max-w-md w-full space-y-4">
      <div class="flex items-center justify-between">
        <h3 class="font-bold text-base text-primary flex items-center gap-2">
          <RefreshCw class="h-4 w-4 text-accent animate-spin" />
          {moveProgress.title ? t(moveProgress.title) : t('Đang chuyển')}
        </h3>
        <button
          type="button"
          onclick={handleCancelMove}
          class="text-xs font-medium text-rose-500 hover:text-rose-600"
        >
          {t('Hủy')}
        </button>
      </div>

      <div class="grid grid-cols-3 gap-1.5 text-[11px]">
        {#each moveSteps as step, idx}
          <div
            class={`px-2 py-1 rounded text-center font-medium ${
              idx < activeIdx
                ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300'
                : idx === activeIdx
                  ? 'bg-accent text-white'
                  : 'bg-subtle text-secondary'
            }`}
          >
            {t(step.label)}
          </div>
        {/each}
      </div>

      <div class="w-full bg-subtle rounded-full h-2.5 overflow-hidden">
        <div
          class="bg-accent h-2.5 rounded-full transition-all duration-150"
          style={`width: ${moveProgress.percent}%`}
        ></div>
      </div>

      <div class="flex items-center justify-between text-xs text-secondary font-mono">
        <span>{moveProgress.percent}%</span>
        <span>{formatSize(moveProgress.done_bytes)} / {formatSize(moveProgress.total_bytes)}</span>
      </div>

      {#if moveProgress.detail}
        <p class="text-xs text-secondary truncate bg-subtle px-2.5 py-1.5 rounded">
          {moveProgress.detail}
        </p>
      {/if}
    </div>
  </div>
{/if}

<!-- Fallback manual guide modal -->
<GuidedModal
  open={showGuidedFallback}
  candidate={guidedCandidate}
  targetDrive={selectedDrive}
  onClose={() => (showGuidedFallback = false)}
  onRecheck={() => {
    showGuidedFallback = false;
    refresh();
  }}
/>
