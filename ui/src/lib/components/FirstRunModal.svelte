<!-- ui/src/lib/components/FirstRunModal.svelte: 2-Step Goal-First Onboarding -->
<script lang="ts">
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    completeFirstRun,
    saveDriveQuota,
    updateSettings,
    type DriveInfo
  } from '$lib/api';
  import BrandLogo from '$lib/components/BrandLogo.svelte';
  import {
    HardDrive,
    AlertTriangle,
    CheckCircle,
    Clock,
    Target,
    ShieldCheck,
    ArrowRight,
    ArrowLeft
  } from 'lucide-svelte';

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
    initialRetentionDays = 7,
    onCompleted,
    onDefer
  }: Props = $props();

  let step = $state<1 | 2>(1);

  // Step 1: Goal state (protect system drive by default)
  let systemDrive = $derived(drives.find((d) => d.is_system) ?? drives[0] ?? null);
  let goalDriveRoot = $state('');
  $effect(() => {
    if (!goalDriveRoot && systemDrive) {
      goalDriveRoot = systemDrive.root;
    }
  });
  let goalMaxPct = $state(80);
  let goalAutoClean = $state(true);
  let goalWarnPct = $derived(Math.max(40, goalMaxPct - 5));
  let goalTargetPct = $derived(Math.max(35, goalMaxPct - 10));

  // Step 2: Vault storage & Autostart
  let initialVaultDrive = $derived(
    recommendedDrive ||
      drives.find((d) => d.is_eligible && !d.is_system)?.drive_letter ||
      drives.find((d) => d.is_eligible)?.drive_letter ||
      'C'
  );
  let selectedDrive = $state('');
  $effect(() => {
    if (!selectedDrive && initialVaultDrive) {
      selectedDrive = initialVaultDrive;
    }
  });

  let retentionDays = $state(7);
  let retentionInitialized = false;
  $effect(() => {
    const initial = initialRetentionDays;
    if (!retentionInitialized) {
      retentionDays = initial;
      retentionInitialized = true;
    }
  });
  let enableAutostart = $state(true);

  let isSubmitting = $state(false);
  let errorMessage = $state<string | null>(null);

  async function handleFinish() {
    if (isSubmitting) return;
    isSubmitting = true;
    errorMessage = null;

    try {
      await completeFirstRun(selectedDrive || initialVaultDrive, retentionDays);
      if (goalDriveRoot) {
        await saveDriveQuota({
          drive_root: goalDriveRoot,
          limit_bytes: 0,
          warn_ratio: goalWarnPct / 100,
          clean_ratio: goalMaxPct / 100,
          target_ratio: goalTargetPct / 100,
          auto_clean: goalAutoClean,
          auto_backup: false,
          backup_dir: null,
          enabled: true
        });
      }
      try {
        await updateSettings({ autostart_enabled: enableAutostart });
      } catch {
        // Non-fatal if OS blocks autostart registration in sandboxed env
      }
      onCompleted();
    } catch (err: unknown) {
      errorMessage = err instanceof Error ? err.message : String(err);
      isSubmitting = false;
    }
  }

  function handleLater() {
    if (onDefer) {
      onDefer();
    } else {
      handleFinish();
    }
  }
</script>

<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/55 backdrop-blur-sm p-4">
  <div class="w-full max-w-xl rounded-2xl border border-border bg-card p-6 shadow-2xl flex flex-col gap-5 max-h-full overflow-y-auto">
    <!-- Brand + Stepper Header -->
    <div class="flex items-center justify-between gap-3">
      <div class="flex items-center gap-2.5">
        <BrandLogo size={32} />
        <div>
          <div class="text-sm font-bold text-primary">GreenC</div>
          <div class="text-[11px] text-secondary">
            {t('Đặt hạn mức một lần, ổ đĩa luôn xanh mỗi ngày')}
          </div>
        </div>
      </div>
      <div class="flex items-center gap-1.5 text-[11px] font-semibold">
        <span class={`px-2.5 py-1 rounded-full ${step === 1 ? 'bg-accent text-white' : 'bg-accent/15 text-accent'}`}>
          1. {t('Mục tiêu')}
        </span>
        <span class={`px-2.5 py-1 rounded-full ${step === 2 ? 'bg-accent text-white' : 'bg-subtle text-secondary'}`}>
          2. {t('Kho an toàn')}
        </span>
      </div>
    </div>

    {#if errorMessage}
      <div class="rounded-lg bg-danger/10 border border-danger/30 p-3 text-xs text-danger flex items-center gap-2">
        <AlertTriangle class="h-4 w-4 shrink-0" />
        <span>{errorMessage}</span>
      </div>
    {/if}

    {#if step === 1}
      <!-- STEP 1: Set Drive Goal -->
      <div class="space-y-4">
        <div class="space-y-1">
          <h2 class="text-lg font-bold text-primary flex items-center gap-2">
            <Target class="h-5 w-5 text-accent" />
            {t('Đặt mục tiêu giữ ổ đĩa luôn xanh')}
          </h2>
          <p class="text-xs text-secondary leading-relaxed">
            {t('Chỉ cần chọn ổ đĩa và mức dung lượng tối đa bạn muốn giữ. Mọi hoạt động dọn dẹp và báo cáo sẽ xoay quanh mục tiêu này.')}
          </p>
        </div>

        <div class="grid grid-cols-1 sm:grid-cols-2 gap-2.5 max-h-[190px] overflow-y-auto pr-1">
          {#each drives as d (d.root)}
            {@const isSelected = goalDriveRoot === d.root}
            {@const usedPct = d.total_bytes > 0 ? Math.round(((d.total_bytes - d.free_bytes) / d.total_bytes) * 100) : 0}
            <button
              type="button"
              onclick={() => (goalDriveRoot = d.root)}
              class={`text-start p-3 rounded-xl border transition-all space-y-1.5 ${
                isSelected
                  ? 'border-accent bg-accent/10 ring-1 ring-accent'
                  : 'border-border bg-subtle/40 hover:bg-subtle'
              }`}
            >
              <div class="flex items-center justify-between gap-2">
                <div class="flex items-center gap-1.5 min-w-0">
                  <HardDrive class="h-4 w-4 text-accent shrink-0" />
                  <span dir="ltr" class="font-mono font-bold text-primary truncate">{d.root}</span>
                  {#if d.is_system}
                    <span class="px-1.5 py-0.5 rounded text-[10px] bg-accent/15 text-accent font-semibold">
                      {t('Ổ hệ thống')}
                    </span>
                  {/if}
                </div>
                {#if isSelected}
                  <CheckCircle class="h-4 w-4 text-accent shrink-0" />
                {/if}
              </div>
              <div class="h-1.5 rounded-full bg-border overflow-hidden">
                <div class={`h-full rounded-full ${usedPct >= 90 ? 'bg-danger' : 'bg-accent'}`} style="width: {usedPct}%"></div>
              </div>
              <div class="text-[11px] text-secondary tabular-numbers">
                {t('Còn trống {free} / {total}', { free: formatSize(d.free_bytes), total: formatSize(d.total_bytes) })} ({usedPct}%)
              </div>
            </button>
          {/each}
        </div>

        <!-- Goal Slider -->
        <div class="rounded-xl border border-accent/25 bg-accent/5 p-4 space-y-3">
          <div class="flex items-center justify-between">
            <span class="text-xs font-semibold text-primary">
              {t('Giữ ổ luôn xanh dưới mức')}
            </span>
            <span class="text-xl font-bold text-accent tabular-numbers">{goalMaxPct}%</span>
          </div>
          <input
            type="range"
            min="55"
            max="90"
            step="5"
            bind:value={goalMaxPct}
            class="w-full accent-accent cursor-pointer"
          />
          <div class="flex justify-between text-[11px] text-secondary tabular-numbers">
            <span>{t('Cảnh báo ở {pct}%', { pct: goalWarnPct })}</span>
            <span>{t('Tự động dọn về {pct}%', { pct: goalTargetPct })}</span>
          </div>

          <label class="flex items-center gap-2.5 pt-2 border-t border-border/60 text-xs text-primary cursor-pointer">
            <input type="checkbox" bind:checked={goalAutoClean} class="h-4 w-4 rounded border-border text-accent" />
            <span>{t('Tự động dọn file rác vào Kho an toàn khi vượt mức này')}</span>
          </label>
        </div>

        <div class="flex items-center justify-between pt-1">
          <button
            type="button"
            onclick={handleLater}
            class="text-xs text-secondary hover:text-primary font-medium"
          >
            {t('Để sau')}
          </button>
          <button
            type="button"
            onclick={() => (step = 2)}
            class="inline-flex items-center gap-1.5 px-5 py-2.5 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm"
          >
            {t('Tiếp tục')}
            <ArrowRight class="h-4 w-4" />
          </button>
        </div>
      </div>
    {:else}
      <!-- STEP 2: Canary Vault Storage & Background Protection -->
      <div class="space-y-4">
        <div class="space-y-1">
          <h2 class="text-lg font-bold text-primary flex items-center gap-2">
            <ShieldCheck class="h-5 w-5 text-accent" />
            {t('Chọn nơi lưu dữ liệu')}
          </h2>
          <p class="text-xs text-secondary leading-relaxed">
            {t('Mọi file dọn dẹp được nén và cất vào Kho an toàn trước khi xóa — bạn có thể khôi phục nguyên vẹn bất cứ lúc nào.')}
          </p>
        </div>

        <div class="space-y-2 max-h-[200px] overflow-y-auto pr-1">
          {#each drives as drive (drive.root)}
            {@const isSelected = selectedDrive === drive.drive_letter}
            {@const usedRatio = drive.total_bytes > 0 ? (drive.total_bytes - drive.free_bytes) / drive.total_bytes : 0}
            <label
              class={`flex items-center justify-between gap-3 p-3 rounded-xl border transition-all cursor-pointer ${
                !drive.is_eligible
                  ? 'opacity-50 bg-subtle/40 border-border cursor-not-allowed'
                  : isSelected
                  ? 'border-accent bg-accent/10 ring-1 ring-accent'
                  : 'border-border bg-subtle/30 hover:bg-subtle'
              }`}
            >
              <div class="flex items-center gap-3 min-w-0">
                <input
                  type="radio"
                  name="vaultDrive"
                  value={drive.drive_letter}
                  checked={isSelected}
                  disabled={!drive.is_eligible}
                  onchange={() => (selectedDrive = drive.drive_letter)}
                  class="h-4 w-4 text-accent border-border"
                />
                <div class="min-w-0">
                  <div class="text-xs font-bold text-primary flex items-center gap-1.5">
                    <span dir="ltr" class="font-mono">{drive.root}</span>
                    <span class="font-normal text-secondary truncate">{drive.label || t('Ổ đĩa cục bộ')}</span>
                  </div>
                  <div class="text-[11px] text-secondary tabular-numbers">
                    {#if drive.is_eligible}
                      {t('Còn trống {free} / {total}', { free: formatSize(drive.free_bytes), total: formatSize(drive.total_bytes) })} ({Math.round(usedRatio * 100)}%)
                    {:else}
                      <span class="text-danger">{drive.ineligible_reason || t('Không dùng được')}</span>
                    {/if}
                  </div>
                </div>
              </div>
              {#if drive.is_eligible && recommendedDrive === drive.drive_letter}
                <span class="px-2 py-0.5 rounded-full text-[10px] font-semibold bg-accent/15 text-accent shrink-0">
                  {t('Đề xuất')}
                </span>
              {/if}
            </label>
          {/each}
        </div>

        <div class="rounded-xl border border-border bg-subtle/40 p-3.5 space-y-3 text-xs">
          <div class="flex items-center justify-between gap-3">
            <div class="flex items-center gap-2 text-primary font-medium">
              <Clock class="h-4 w-4 text-accent" />
              <span>{t('Giữ bản sao lưu an toàn trong')}</span>
            </div>
            <div class="flex items-center gap-1.5">
              <input
                type="number"
                min="1"
                max="90"
                bind:value={retentionDays}
                class="w-16 rounded-lg border border-border bg-card px-2 py-1 text-center font-bold text-primary tabular-numbers"
              />
              <span class="text-secondary">{t('ngày')}</span>
            </div>
          </div>

          <label class="flex items-center justify-between gap-3 pt-2.5 border-t border-border/60 cursor-pointer">
            <div>
              <div class="font-semibold text-primary">{t('Khởi động cùng hệ điều hành')}</div>
              <div class="text-[11px] text-secondary">{t('Chạy ngầm ở khay hệ thống (< 15 MB RAM) để tự bảo vệ ổ đĩa mỗi ngày.')}</div>
            </div>
            <input
              type="checkbox"
              bind:checked={enableAutostart}
              class="h-4 w-4 rounded border-border text-accent shrink-0"
            />
          </label>
        </div>

        <div class="flex items-center justify-between pt-1">
          <button
            type="button"
            onclick={() => (step = 1)}
            class="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg border border-border text-xs text-secondary hover:text-primary"
          >
            <ArrowLeft class="h-3.5 w-3.5" />
            {t('Quay lại')}
          </button>
          <button
            type="button"
            disabled={isSubmitting}
            onclick={handleFinish}
            class="inline-flex items-center gap-1.5 px-5 py-2.5 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm disabled:opacity-50"
          >
            {isSubmitting ? t('Đang thiết lập...') : t('Bắt đầu bảo vệ ổ đĩa')}
          </button>
        </div>
      </div>
    {/if}
  </div>
</div>
