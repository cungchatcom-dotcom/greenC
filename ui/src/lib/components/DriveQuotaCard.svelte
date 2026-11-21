<!-- ui/src/lib/components/DriveQuotaCard.svelte: Goal-first per-drive quota (simple 1-slider + collapsible advanced) -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    getBootstrapState,
    listDriveQuotas,
    saveDriveQuota,
    deleteDriveQuota,
    type DriveInfo,
    type DriveQuotaStatusDto
  } from '$lib/api';
  import {
    Gauge,
    Plus,
    Trash2,
    Edit3,
    X,
    HardDrive,
    CheckCircle2,
    ChevronDown,
    ChevronUp,
    ShieldCheck
  } from 'lucide-svelte';

  interface Props {
    drives?: DriveInfo[];
    onChanged?: () => void;
  }
  let { drives = [], onChanged }: Props = $props();

  const ONE_GIB = 1024 * 1024 * 1024;

  let localDrives = $state<DriveInfo[]>([]);
  let effectiveDrives = $derived(drives.length > 0 ? drives : localDrives);

  let quotas = $state<DriveQuotaStatusDto[]>([]);
  let editing = $state(false);
  let showAdvanced = $state(false);
  let error = $state<string | null>(null);
  let saving = $state(false);

  // form state
  let formId = $state<number | null>(null);
  let formRoot = $state('');
  let formLimitGb = $state(0);
  let formWarnPct = $state(75);
  let formCleanPct = $state(80);
  let formTargetPct = $state(70);
  let formAutoClean = $state(true);
  let formAutoBackup = $state(false);
  let formBackupDir = $state('');
  let formEnabled = $state(true);

  async function ensureDrivesLoaded() {
    if (drives.length > 0 || localDrives.length > 0) return;
    try {
      const boot = await getBootstrapState();
      if (boot?.drives?.length) {
        localDrives = boot.drives;
      }
    } catch {
      // ignore fallback errors
    }
  }

  async function refresh() {
    try {
      quotas = await listDriveQuotas();
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    ensureDrivesLoaded();
    refresh();
    const timer = setInterval(refresh, 30_000);
    return () => clearInterval(timer);
  });

  function applySimpleGoalPct(cleanPct: number) {
    const clamped = Math.min(95, Math.max(50, Math.round(cleanPct)));
    formCleanPct = clamped;
    if (!showAdvanced) {
      formWarnPct = Math.max(40, clamped - 5);
      formTargetPct = Math.max(35, clamped - 10);
    }
  }

  async function openNew() {
    await ensureDrivesLoaded();
    const list = effectiveDrives;
    const defaultDrive = list.find((d) => d.is_system) ?? list[0];
    formId = null;
    formRoot = defaultDrive?.root ?? 'C:\\';
    formLimitGb = 0;
    formCleanPct = 80;
    formWarnPct = 75;
    formTargetPct = 70;
    formAutoClean = true;
    formAutoBackup = false;
    formBackupDir = '';
    formEnabled = true;
    showAdvanced = false;
    error = null;
    editing = true;
  }

  function openEdit(q: DriveQuotaStatusDto) {
    formId = q.id;
    formRoot = q.drive_root;
    formLimitGb = q.limit_bytes ? Number((q.limit_bytes / ONE_GIB).toFixed(1)) : 0;
    formWarnPct = Math.round(q.warn_ratio * 100);
    formCleanPct = Math.round(q.clean_ratio * 100);
    formTargetPct = Math.round(q.target_ratio * 100);
    formAutoClean = q.auto_clean;
    formAutoBackup = q.auto_backup;
    formBackupDir = q.backup_dir ?? '';
    formEnabled = q.enabled;
    showAdvanced = false;
    error = null;
    editing = true;
  }

  async function submit(ev: Event) {
    ev.preventDefault();
    if (saving) return;
    error = null;
    if (!formRoot.trim()) {
      error = t('Vui lòng chọn ổ đĩa.');
      return;
    }
    if (!(formWarnPct <= formCleanPct && formTargetPct < formCleanPct)) {
      error = t('Ngưỡng phải thỏa: cảnh báo ≤ tự động hành động, và mức dọn về < tự động hành động.');
      return;
    }
    if (formAutoBackup && !formBackupDir.trim()) {
      error = t('Hãy chọn thư mục sao lưu (nên nằm trên ổ khác).');
      return;
    }
    saving = true;
    try {
      await saveDriveQuota({
        id: formId,
        drive_root: formRoot.trim(),
        limit_bytes: Math.round(Math.max(0, formLimitGb) * ONE_GIB),
        warn_ratio: formWarnPct / 100,
        clean_ratio: formCleanPct / 100,
        target_ratio: formTargetPct / 100,
        auto_clean: formAutoClean,
        auto_backup: formAutoBackup,
        backup_dir: formBackupDir.trim() || null,
        enabled: formEnabled
      });
      editing = false;
      await refresh();
      onChanged?.();
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function remove(q: DriveQuotaStatusDto) {
    await deleteDriveQuota(q.id);
    await refresh();
    onChanged?.();
  }

  function levelLabel(level: string): string {
    if (level === 'critical') return t('Nguy cấp');
    if (level === 'warning') return t('Cảnh báo');
    return t('Đạt mục tiêu');
  }

  function levelClass(level: string): string {
    if (level === 'critical') return 'bg-danger/10 text-danger';
    if (level === 'warning') return 'bg-warning/10 text-warning';
    return 'bg-success/10 text-success';
  }

  function barClass(level: string): string {
    if (level === 'critical') return 'bg-danger';
    if (level === 'warning') return 'bg-warning';
    return 'bg-accent';
  }
</script>

<section class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-4" data-testid="drive-quota-card">
  <div class="flex items-center justify-between gap-3 flex-wrap">
    <div>
      <h2 class="text-sm font-bold text-primary flex items-center gap-2">
        <Gauge class="h-4 w-4 text-accent" />
        {t('Mục tiêu giữ ổ đĩa luôn xanh')}
      </h2>
      <p class="text-xs text-secondary">
        {t('Đặt mức tối đa cho từng ổ. GreenC sẽ tự động theo dõi, dọn rác vào Kho an toàn và giữ ổ luôn xanh mỗi ngày.')}
      </p>
    </div>
    <button
      type="button"
      onclick={openNew}
      class="inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm"
    >
      <Plus class="h-3.5 w-3.5" />
      {t('Thêm hạn mức ổ')}
    </button>
  </div>

  {#if error && !editing}
    <div class="text-xs text-danger">{error}</div>
  {/if}

  {#if quotas.length === 0 && !editing}
    <div class="rounded-lg border border-dashed border-border bg-subtle/40 py-6 px-4 text-center space-y-2">
      <div class="text-xs font-medium text-primary">{t('Chưa có mục tiêu ổ đĩa nào.')}</div>
      <p class="text-[11px] text-secondary">
        {t('Đặt mục tiêu một lần (vd: giữ ổ hệ thống dưới 80%) để GreenC tự động bảo vệ mỗi ngày.')}
      </p>
      <button
        type="button"
        onclick={openNew}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-accent/15 text-accent hover:bg-accent/20 text-xs font-semibold"
      >
        <Plus class="h-3.5 w-3.5" />
        {t('Đặt mục tiêu ngay')}
      </button>
    </div>
  {:else if quotas.length > 0}
    <div class="space-y-2.5">
      {#each quotas as q (q.id)}
        {@const pct = Math.min(100, Math.round(q.usage_ratio * 100))}
        {@const targetMark = Math.min(100, Math.round(q.clean_ratio * 100))}
        <div class="rounded-lg border border-border bg-subtle/40 px-4 py-3 text-xs space-y-2">
          <div class="flex items-center justify-between gap-3">
            <div class="flex items-center gap-2 min-w-0 flex-wrap">
              <HardDrive class="h-3.5 w-3.5 text-accent shrink-0" />
              <span dir="ltr" class="font-mono font-bold text-primary ltr-isolate">{q.drive_root}</span>
              <span class="px-2 py-0.5 rounded-full text-[10px] font-semibold {levelClass(q.level)}">
                {levelLabel(q.level)}
              </span>
              <span class="text-[11px] text-secondary">
                {t('Mục tiêu')}: &lt; {targetMark}%
              </span>
              {#if !q.enabled}
                <span class="text-[10px] text-disabled">({t('Đang tắt')})</span>
              {/if}
              {#if !q.available}
                <span class="text-[10px] text-disabled">({t('Ổ không khả dụng')})</span>
              {/if}
            </div>
            <div class="flex items-center gap-1.5 shrink-0">
              <button
                type="button"
                onclick={() => openEdit(q)}
                class="p-1.5 rounded border border-border bg-card text-secondary hover:text-primary"
                title={t('Chỉnh sửa')}
              >
                <Edit3 class="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                onclick={() => remove(q)}
                class="p-1.5 rounded border border-border bg-card text-secondary hover:text-danger"
                title={t('Xóa hạn mức')}
              >
                <Trash2 class="h-3.5 w-3.5" />
              </button>
            </div>
          </div>

          <div class="relative h-2.5 rounded-full bg-border overflow-hidden">
            <div
              class="h-full rounded-full transition-all {barClass(q.level)}"
              style="width: {pct}%"
            ></div>
            <div
              class="absolute top-0 bottom-0 w-0.5 bg-primary/70"
              style="left: {targetMark}%"
              title="{t('Mục tiêu')}: {targetMark}%"
            ></div>
          </div>

          <div class="flex items-center justify-between text-[11px] text-secondary tabular-numbers flex-wrap gap-2">
            <span>
              {t('Đã dùng')}: <strong class="text-primary">{formatSize(q.used_bytes)}</strong> / {formatSize(q.effective_limit_bytes)} ({pct}%)
            </span>
            <span class="text-disabled">
              {#if q.auto_clean}{t('Tự động dọn về {pct}%', { pct: Math.round(q.target_ratio * 100) })}{:else}{t('Chỉ cảnh báo')}{/if}
              {#if q.auto_backup} · {t('Tự động sao lưu')}{/if}
            </span>
          </div>
        </div>
      {/each}
    </div>
  {/if}

  {#if editing}
    <form onsubmit={submit} class="rounded-xl border border-accent/30 bg-subtle/50 p-4 space-y-4 text-xs">
      <div class="flex items-center justify-between">
        <span class="font-bold text-primary text-sm flex items-center gap-1.5">
          <ShieldCheck class="h-4 w-4 text-accent" />
          {formId ? t('Chỉnh sửa hạn mức ổ') : t('Đặt mục tiêu cho ổ đĩa')}
        </span>
        <button type="button" onclick={() => (editing = false)} class="p-1 text-secondary hover:text-primary" title={t('Đóng')}>
          <X class="h-4 w-4" />
        </button>
      </div>

      <!-- Step 1: Visual Drive Picker (never blank) -->
      <div class="space-y-1.5">
        <div class="text-secondary font-semibold">{t('1. Chọn ổ đĩa cần bảo vệ')}</div>
        {#if effectiveDrives.length > 0}
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
            {#each effectiveDrives as d (d.root)}
              {@const selected = formRoot === d.root}
              {@const usedPct = d.total_bytes > 0 ? Math.round(((d.total_bytes - d.free_bytes) / d.total_bytes) * 100) : 0}
              <button
                type="button"
                disabled={formId !== null}
                onclick={() => (formRoot = d.root)}
                class={`text-start p-3 rounded-lg border transition-all space-y-1.5 disabled:opacity-70 ${
                  selected
                    ? 'border-accent bg-accent/10 ring-1 ring-accent'
                    : 'border-border bg-card hover:bg-subtle'
                }`}
              >
                <div class="flex items-center justify-between gap-2">
                  <div class="flex items-center gap-1.5 min-w-0">
                    <HardDrive class="h-3.5 w-3.5 text-accent shrink-0" />
                    <span dir="ltr" class="font-mono font-bold text-primary truncate">{d.root}</span>
                    {#if d.label}
                      <span class="text-secondary truncate">({d.label})</span>
                    {/if}
                  </div>
                  {#if selected}
                    <CheckCircle2 class="h-4 w-4 text-accent shrink-0" />
                  {/if}
                </div>
                <div class="h-1.5 rounded-full bg-border overflow-hidden">
                  <div class="h-full bg-accent rounded-full" style="width: {usedPct}%"></div>
                </div>
                <div class="text-[11px] text-secondary tabular-numbers">
                  {t('Còn trống {free} / {total}', { free: formatSize(d.free_bytes), total: formatSize(d.total_bytes) })} ({usedPct}%)
                </div>
              </button>
            {/each}
          </div>
        {:else}
          <input
            type="text"
            dir="ltr"
            bind:value={formRoot}
            disabled={formId !== null}
            placeholder="C:\ or /"
            class="w-full rounded-lg border border-border bg-card px-3 py-2 font-mono text-primary"
          />
        {/if}
      </div>

      <!-- Step 2: Single Target Slider -->
      <div class="rounded-lg border border-border bg-card p-3.5 space-y-2.5">
        <div class="flex items-center justify-between">
          <span class="font-semibold text-primary">{t('2. Giữ ổ luôn xanh dưới mức')}</span>
          <span class="text-base font-bold text-accent tabular-numbers">{formCleanPct}%</span>
        </div>
        <input
          type="range"
          min="50"
          max="95"
          step="5"
          value={formCleanPct}
          oninput={(e) => applySimpleGoalPct(Number((e.currentTarget as HTMLInputElement).value))}
          class="w-full accent-accent cursor-pointer"
        />
        <div class="flex justify-between text-[11px] text-disabled tabular-numbers">
          <span>50%</span>
          <span>{t('Cảnh báo')} {formWarnPct}% · {t('Tự dọn về')} {formTargetPct}%</span>
          <span>95%</span>
        </div>

        <label class="flex items-center gap-2 pt-2 border-t border-border/60 cursor-pointer text-primary">
          <input type="checkbox" bind:checked={formAutoClean} class="h-4 w-4 rounded border-border text-accent" />
          <span>{t('Tự động dọn file rác vào Kho an toàn khi vượt mức này')}</span>
        </label>
      </div>

      <!-- Collapsible Advanced Settings -->
      <div>
        <button
          type="button"
          onclick={() => (showAdvanced = !showAdvanced)}
          class="inline-flex items-center gap-1 text-secondary hover:text-primary font-medium"
        >
          {#if showAdvanced}
            <ChevronUp class="h-3.5 w-3.5" />
          {:else}
            <ChevronDown class="h-3.5 w-3.5" />
          {/if}
          {t('Tùy chỉnh nâng cao')}
        </button>

        {#if showAdvanced}
          <div class="mt-2.5 grid grid-cols-1 sm:grid-cols-2 gap-3 p-3 rounded-lg border border-border bg-card">
            <label class="space-y-1">
              <span class="text-secondary">{t('Hạn mức (GB, 0 = toàn bộ ổ)')}</span>
              <input type="number" min="0" step="1" bind:value={formLimitGb} class="w-full rounded border border-border bg-subtle px-2 py-1.5 text-primary" />
            </label>
            <label class="space-y-1">
              <span class="text-secondary">{t('Ngưỡng cảnh báo (%)')}</span>
              <input type="number" min="10" max="99" bind:value={formWarnPct} class="w-full rounded border border-border bg-subtle px-2 py-1.5 text-primary" />
            </label>
            <label class="space-y-1">
              <span class="text-secondary">{t('Ngưỡng tự động hành động (%)')}</span>
              <input type="number" min="15" max="99" bind:value={formCleanPct} class="w-full rounded border border-border bg-subtle px-2 py-1.5 text-primary" />
            </label>
            <label class="space-y-1">
              <span class="text-secondary">{t('Dọn về mức mục tiêu (%)')}</span>
              <input type="number" min="10" max="98" bind:value={formTargetPct} class="w-full rounded border border-border bg-subtle px-2 py-1.5 text-primary" />
            </label>

            <div class="sm:col-span-2 flex flex-wrap gap-4 pt-1">
              <label class="inline-flex items-center gap-1.5 cursor-pointer text-primary">
                <input type="checkbox" bind:checked={formAutoBackup} />
                <span>{t('Tự động sao lưu')}</span>
              </label>
              <label class="inline-flex items-center gap-1.5 cursor-pointer text-primary">
                <input type="checkbox" bind:checked={formEnabled} />
                <span>{t('Bật')}</span>
              </label>
            </div>

            {#if formAutoBackup}
              <label class="sm:col-span-2 space-y-1">
                <span class="text-secondary">{t('Thư mục sao lưu (ổ khác)')}</span>
                <input
                  type="text"
                  dir="ltr"
                  bind:value={formBackupDir}
                  placeholder="D:\GreenC-Backup"
                  class="w-full rounded border border-border bg-subtle px-2 py-1.5 font-mono text-primary"
                />
              </label>
            {/if}
          </div>
        {/if}
      </div>

      {#if error}
        <div class="text-danger">{error}</div>
      {/if}

      <div class="flex justify-end gap-2">
        <button
          type="button"
          onclick={() => (editing = false)}
          class="px-3 py-1.5 rounded-lg border border-border bg-card text-secondary hover:text-primary"
        >
          {t('Hủy')}
        </button>
        <button
          type="submit"
          disabled={saving}
          class="px-4 py-1.5 rounded-lg bg-accent hover:bg-accent-hover text-white font-semibold disabled:opacity-50"
        >
          {saving ? t('Đang lưu...') : t('Lưu mục tiêu')}
        </button>
      </div>
    </form>
  {/if}
</section>
