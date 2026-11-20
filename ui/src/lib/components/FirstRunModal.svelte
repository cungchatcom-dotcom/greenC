<!-- ui/src/lib/components/FirstRunModal.svelte -->
<script lang="ts">
  import { t, formatSize } from '$lib/i18n.svelte';
  import { completeFirstRun, type DriveInfo } from '$lib/api';
  import { HardDrive, AlertTriangle, CheckCircle, Clock } from 'lucide-svelte';

  interface Props {
    drives: DriveInfo[];
    recommendedDrive: string | null;
    initialRetentionDays?: number;
    onCompleted: () => void;
    onDefer?: () => void;
  }

  let {
    drives,
    recommendedDrive,
    initialRetentionDays = 14,
    onCompleted,
    onDefer
  }: Props = $props();

  // Find optimal default drive
  let initialDrive = $derived(
    recommendedDrive ||
    drives.find((d) => d.is_eligible && !d.is_system)?.drive_letter ||
    drives.find((d) => d.is_eligible)?.drive_letter ||
    'C'
  );

  let selectedDrive = $state('');
  $effect(() => {
    if (!selectedDrive && initialDrive) {
      selectedDrive = initialDrive;
    }
  });

  let retentionDays = $state(14);
  let retentionInitialized = false;
  $effect(() => {
    const initial = initialRetentionDays;
    if (!retentionInitialized) {
      retentionDays = initial;
      retentionInitialized = true;
    }
  });
  let isSubmitting = $state(false);
  let errorMessage = $state<string | null>(null);

  // Derived selected target data root
  let targetDataRoot = $derived(`${selectedDrive || 'C'}:\\GreenC\\`);

  async function handleConfirm(driveToUse: string) {
    if (isSubmitting) return;
    isSubmitting = true;
    errorMessage = null;

    try {
      await completeFirstRun(driveToUse, retentionDays);
      onCompleted();
    } catch (err: any) {
      errorMessage = err?.message || String(err);
      isSubmitting = false;
    }
  }

  function handleLater() {
    if (onDefer) {
      onDefer();
    } else {
      const fallbackDrive = recommendedDrive || initialDrive;
      handleConfirm(fallbackDrive);
    }
  }
</script>

<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm p-4">
  <div class="w-full max-w-xl rounded-xl border border-border bg-card p-6 shadow-2xl flex flex-col gap-5 animate-in fade-in zoom-in-95 duration-200 max-h-full overflow-y-auto">
    <!-- Header -->
    <div class="space-y-1">
      <h2 class="text-xl font-bold text-primary flex items-center gap-2">
        <HardDrive class="h-5 w-5 text-accent" />
        {t('Chọn nơi lưu dữ liệu')}
      </h2>
      <p class="text-sm text-secondary leading-relaxed">
        {t('{app} chuyển dữ liệu ứng dụng và Kho dự phòng sang ổ này để giải phóng ổ C:. Có thể đổi lại trong Cài đặt.', { app: 'GreenC' })}
      </p>
    </div>

    {#if errorMessage}
      <div class="rounded-lg bg-danger/10 border border-danger/30 p-3 text-xs text-danger flex items-center gap-2">
        <AlertTriangle class="h-4 w-4 shrink-0" />
        <span>{errorMessage}</span>
      </div>
    {/if}

    <!-- Drive Cards Radio Group -->
    <div class="space-y-2.5 max-h-[280px] overflow-y-auto pr-1">
      {#each drives as drive}
        {@const isSelected = selectedDrive === drive.drive_letter}
        {@const usedRatio = drive.total_bytes > 0 ? (drive.total_bytes - drive.free_bytes) / drive.total_bytes : 0}
        {@const isDanger = usedRatio >= 0.90}

        <label
          class={`flex items-start gap-3.5 p-3.5 rounded-lg border transition-all cursor-pointer ${
            !drive.is_eligible
              ? 'opacity-60 bg-subtle/50 border-border cursor-not-allowed'
              : isSelected
              ? 'border-accent bg-accent/5 ring-1 ring-accent'
              : 'border-border bg-card hover:bg-subtle'
          }`}
        >
          <input
            type="radio"
            name="driveSelection"
            value={drive.drive_letter}
            checked={isSelected}
            disabled={!drive.is_eligible}
            onchange={() => (selectedDrive = drive.drive_letter)}
            class="mt-1 h-4 w-4 text-accent border-border focus:ring-accent"
          />

          <div class="flex-1 space-y-2">
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-2">
                <span class="font-bold text-primary">{drive.drive_letter}:</span>
                <span class="text-sm text-secondary">{drive.label || t('Ổ đĩa cục bộ')}</span>
              </div>

              <!-- Badges -->
              <div class="flex items-center gap-1.5">
                {#if !drive.is_eligible}
                  <span class="px-2 py-0.5 rounded text-[11px] font-medium bg-danger/10 text-danger border border-danger/20">
                    {t('Không dùng được')}
                  </span>
                {:else if recommendedDrive === drive.drive_letter}
                  <span class="px-2 py-0.5 rounded text-[11px] font-medium bg-accent/15 text-accent border border-accent/30 flex items-center gap-1">
                    <CheckCircle class="h-3 w-3" />
                    {t('Đề xuất')}
                  </span>
                {:else if drive.is_system}
                  <span class="px-2 py-0.5 rounded text-[11px] font-medium bg-subtle text-secondary border border-border">
                    {t('Ổ hệ thống')}
                  </span>
                {/if}
              </div>
            </div>

            <!-- Capacity Progress Bar -->
            <div class="w-full bg-border/40 h-1.5 rounded-full overflow-hidden">
              <div
                class={`h-full transition-all duration-300 ${isDanger ? 'bg-danger' : 'bg-accent'}`}
                style="width: {Math.round(usedRatio * 100)}%"
              ></div>
            </div>

            <div class="flex items-center justify-between text-xs text-secondary tabular-numbers">
              {#if drive.is_eligible}
                <span>{t('Còn trống {free} / {total}', { free: formatSize(drive.free_bytes), total: formatSize(drive.total_bytes) })}</span>
                <span>{Math.round(usedRatio * 100)}%</span>
              {:else}
                <span class="text-danger">{drive.ineligible_reason || t('Định dạng không được hỗ trợ')}</span>
              {/if}
            </div>
          </div>
        </label>
      {/each}
    </div>

    <!-- Retention Days Setting -->
    <div class="flex items-center gap-3 p-3 rounded-lg bg-subtle border border-border text-sm">
      <Clock class="h-4 w-4 text-secondary shrink-0" />
      <span class="text-primary">{t('Giữ bản dự phòng trong')}</span>
      <input
        type="number"
        dir="ltr"
        min="1"
        max="90"
        bind:value={retentionDays}
        class="w-16 px-2 py-1 text-center font-bold rounded border border-border bg-card text-primary tabular-numbers focus:border-accent focus:ring-1 focus:ring-accent outline-none"
      />
      <span class="text-secondary">{t('ngày trước khi xóa vĩnh viễn')}</span>
    </div>

    <!-- Target Data Directory Preview -->
    <div class="text-xs text-secondary px-1">
      <span class="font-medium text-primary">{t('Thư mục: {path}', { path: targetDataRoot })}</span>
    </div>

    <!-- Footer Actions -->
    <div class="flex items-center justify-end gap-2.5 pt-2 border-t border-border">
      <button
        type="button"
        onclick={handleLater}
        disabled={isSubmitting}
        class="px-4 py-2 rounded-lg font-medium text-sm text-secondary hover:text-primary hover:bg-subtle transition-colors disabled:opacity-50"
      >
        {t('Để sau')}
      </button>

      <button
        type="button"
        onclick={() => handleConfirm(selectedDrive)}
        disabled={isSubmitting || !selectedDrive}
        class="px-5 py-2 rounded-lg font-medium text-sm bg-accent hover:bg-accent-hover text-white shadow-sm transition-colors flex items-center gap-2 disabled:opacity-50"
      >
        {#if isSubmitting}
          <div class="h-4 w-4 animate-spin rounded-full border-2 border-white border-t-transparent"></div>
        {/if}
        {t('Tiếp tục')}
      </button>
    </div>
  </div>
</div>
