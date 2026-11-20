<!-- ui/src/lib/components/DriveQuotaCard.svelte: per-drive quota (warn / backup / auto-clean) -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, formatSize } from '$lib/i18n.svelte';
  import {
    listDriveQuotas,
    saveDriveQuota,
    deleteDriveQuota,
    type DriveInfo,
    type DriveQuotaStatusDto
  } from '$lib/api';
  import { Gauge, Plus, Trash2, Edit3, X } from 'lucide-svelte';

  interface Props {
    drives: DriveInfo[];
  }
  let { drives }: Props = $props();

  const ONE_GIB = 1024 * 1024 * 1024;

  let quotas = $state<DriveQuotaStatusDto[]>([]);
  let editing = $state(false);
  let error = $state<string | null>(null);
  let saving = $state(false);

  // form state
  let formId = $state<number | null>(null);
  let formRoot = $state('');
  let formLimitGb = $state(0);
  let formWarnPct = $state(80);
  let formCleanPct = $state(90);
  let formTargetPct = $state(85);
  let formAutoClean = $state(false);
  let formAutoBackup = $state(false);
  let formBackupDir = $state('');
  let formEnabled = $state(true);

  async function refresh() {
    try {
      quotas = await listDriveQuotas();
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    refresh();
    const timer = setInterval(refresh, 30_000);
    return () => clearInterval(timer);
  });

  function openNew() {
    formId = null;
    formRoot = drives.find((d) => d.is_system)?.root ?? drives[0]?.root ?? 'C:\\';
    formLimitGb = 0;
    formWarnPct = 80;
    formCleanPct = 90;
    formTargetPct = 85;
    formAutoClean = false;
    formAutoBackup = false;
    formBackupDir = '';
    formEnabled = true;
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
    error = null;
    editing = true;
  }

  async function submit(ev: Event) {
    ev.preventDefault();
    if (saving) return;
    error = null;
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
        drive_root: formRoot,
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
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function remove(q: DriveQuotaStatusDto) {
    await deleteDriveQuota(q.id);
    await refresh();
  }

  function levelLabel(level: string): string {
    if (level === 'critical') return t('Nguy cấp');
    if (level === 'warning') return t('Cảnh báo');
    return t('Bình thường');
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
  <div class="flex items-center justify-between">
    <div>
      <h2 class="text-sm font-bold text-primary flex items-center gap-2">
        <Gauge class="h-4 w-4" />
        {t('Hạn mức theo ổ đĩa')}
      </h2>
      <p class="text-xs text-secondary">
        {t('Đặt hạn mức dung lượng cho từng ổ. GreenC dựa vào đó để cảnh báo, sao lưu và dọn dẹp tự động.')}
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

  {#if quotas.length === 0}
    <div class="rounded-lg border border-border/60 bg-subtle/50 py-6 text-center text-xs text-secondary">
      {t('Chưa có hạn mức ổ đĩa nào.')}
    </div>
  {:else}
    <div class="space-y-2">
      {#each quotas as q (q.id)}
        <div class="rounded-lg border border-border bg-subtle/40 px-3.5 py-3 text-xs space-y-2">
          <div class="flex items-center justify-between gap-3">
            <div class="flex items-center gap-2 min-w-0">
              <span dir="ltr" class="font-mono font-bold text-primary ltr-isolate">{q.drive_root}</span>
              <span class="px-1.5 py-0.5 rounded text-[10px] font-semibold {levelClass(q.level)}">
                {levelLabel(q.level)}
              </span>
              {#if !q.enabled}
                <span class="text-[10px] text-disabled">{t('Đang tắt')}</span>
              {/if}
              {#if !q.available}
                <span class="text-[10px] text-disabled">{t('Ổ không khả dụng')}</span>
              {/if}
            </div>
            <div class="flex items-center gap-2 shrink-0">
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

          <div class="relative h-2 rounded-full bg-border overflow-hidden">
            <div
              class="h-full rounded-full {barClass(q.level)}"
              style="width: {Math.min(100, Math.round(q.usage_ratio * 100))}%"
            ></div>
          </div>

          <div class="text-[11px] text-disabled tabular-numbers">
            {t('Đã dùng')}: {formatSize(q.used_bytes)} / {formatSize(q.effective_limit_bytes)}
            ({Math.round(q.usage_ratio * 100)}%)
            · {t('Cảnh báo')} {Math.round(q.warn_ratio * 100)}%
            · {t('Tự động hành động')} {Math.round(q.clean_ratio * 100)}%
            {#if q.auto_clean}· {t('Tự động dọn dẹp')}{/if}
            {#if q.auto_backup}· {t('Tự động sao lưu')}{/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}

  {#if editing}
    <form onsubmit={submit} class="rounded-lg border border-border bg-subtle/40 p-4 space-y-3 text-xs">
      <div class="flex items-center justify-between">
        <span class="font-bold text-primary">
          {formId ? t('Chỉnh sửa hạn mức ổ') : t('Thêm hạn mức ổ')}
        </span>
        <button type="button" onclick={() => (editing = false)} class="p-1 text-secondary hover:text-primary" title={t('Đóng')}>
          <X class="h-4 w-4" />
        </button>
      </div>

      <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <label class="space-y-1">
          <span class="text-secondary">{t('Ổ đĩa')}</span>
          <select bind:value={formRoot} disabled={formId !== null} class="w-full rounded border border-border bg-card px-2 py-1.5">
            {#each drives as d (d.root)}
              <option value={d.root}>{d.root} {d.label ? `(${d.label})` : ''}</option>
            {/each}
          </select>
        </label>
        <label class="space-y-1">
          <span class="text-secondary">{t('Hạn mức (GB, 0 = toàn bộ ổ)')}</span>
          <input type="number" min="0" step="1" bind:value={formLimitGb} class="w-full rounded border border-border bg-card px-2 py-1.5" />
        </label>
        <label class="space-y-1">
          <span class="text-secondary">{t('Cảnh báo ở mức (%)')}</span>
          <input type="number" min="10" max="99" bind:value={formWarnPct} class="w-full rounded border border-border bg-card px-2 py-1.5" />
        </label>
        <label class="space-y-1">
          <span class="text-secondary">{t('Tự động hành động ở mức (%)')}</span>
          <input type="number" min="20" max="99" bind:value={formCleanPct} class="w-full rounded border border-border bg-card px-2 py-1.5" />
        </label>
        <label class="space-y-1">
          <span class="text-secondary">{t('Dọn về mức (%)')}</span>
          <input type="number" min="10" max="98" bind:value={formTargetPct} class="w-full rounded border border-border bg-card px-2 py-1.5" />
        </label>
      </div>

      <div class="space-y-2">
        <label class="flex items-center gap-2">
          <input type="checkbox" bind:checked={formAutoClean} class="h-4 w-4 rounded border-border" />
          <span>{t('Tự động dọn dẹp khi vượt ngưỡng (dữ liệu vào Kho dự phòng, có thể khôi phục)')}</span>
        </label>
        <label class="flex items-center gap-2">
          <input type="checkbox" bind:checked={formAutoBackup} class="h-4 w-4 rounded border-border" />
          <span>{t('Tự động sao lưu gói Kho dự phòng mới sang thư mục khác')}</span>
        </label>
        {#if formAutoBackup}
          <label class="block space-y-1">
            <span class="text-secondary">{t('Thư mục sao lưu (phải nằm trên ổ khác)')}</span>
            <input
              type="text"
              dir="ltr"
              bind:value={formBackupDir}
              placeholder="D:\GreenC-Backup"
              class="w-full rounded border border-border bg-card px-2 py-1.5 font-mono"
            />
          </label>
        {/if}
        <label class="flex items-center gap-2">
          <input type="checkbox" bind:checked={formEnabled} class="h-4 w-4 rounded border-border" />
          <span>{t('Bật hạn mức này')}</span>
        </label>
      </div>

      {#if error}
        <div class="text-danger">{error}</div>
      {/if}

      <div class="flex justify-end gap-2">
        <button type="button" onclick={() => (editing = false)} class="px-3 py-1.5 rounded border border-border bg-card text-secondary">
          {t('Hủy')}
        </button>
        <button type="submit" disabled={saving} class="px-3.5 py-1.5 rounded bg-accent hover:bg-accent-hover text-white font-semibold disabled:opacity-60">
          {t('Lưu hạn mức')}
        </button>
      </div>
    </form>
  {/if}
</section>
