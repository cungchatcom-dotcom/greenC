<!-- ui/src/lib/components/QuotaRuleModal.svelte (Task T062) -->
<script lang="ts">
  import { t } from '$lib/i18n.svelte';
  import {
    saveQuotaRule,
    type QuotaRuleInputDto,
    type QuotaRuleStatusDto
  } from '$lib/api';
  import { ShieldCheck, X, AlertTriangle } from 'lucide-svelte';

  export interface QuotaRulePrefill {
    id?: number | null;
    app_name?: string;
    process_name?: string;
    service_name?: string;
    target_paths?: string[];
    max_bytes?: number;
    watermark_ratio?: number;
    retention_days?: number;
    exclude_patterns?: string[];
    enabled?: boolean;
  }

  interface Props {
    initialRule?: QuotaRulePrefill | null;
    onClose: () => void;
    onSaved: (saved: QuotaRuleStatusDto) => void;
  }

  let { initialRule = null, onClose, onSaved }: Props = $props();

  const ONE_GIB = 1024 * 1024 * 1024;

  let appName = $state('');
  let processName = $state('');
  let serviceName = $state('');
  let pathsText = $state('');
  let maxGb = $state(5);
  let watermarkPct = $state(85);
  let retentionDays = $state(7);
  let excludesText = $state('');
  let enabled = $state(true);

  let isSaving = $state(false);
  let errorMsg = $state<string | null>(null);

  $effect(() => {
    appName = initialRule?.app_name ?? '';
    processName = initialRule?.process_name ?? '';
    serviceName = initialRule?.service_name ?? '';
    pathsText = (initialRule?.target_paths ?? []).join('\n');
    maxGb = initialRule?.max_bytes
      ? Math.max(0.5, Number((initialRule.max_bytes / ONE_GIB).toFixed(1)))
      : 5;
    watermarkPct = initialRule?.watermark_ratio
      ? Math.round(initialRule.watermark_ratio * 100)
      : 85;
    retentionDays = initialRule?.retention_days ?? 7;
    excludesText = (initialRule?.exclude_patterns ?? []).join(', ');
    enabled = initialRule?.enabled ?? true;
  });

  async function handleSubmit(ev: Event) {
    ev.preventDefault();
    if (isSaving) return;
    errorMsg = null;

    const targetPaths = pathsText
      .split(/\r?\n/)
      .map((p) => p.trim())
      .filter((p) => p.length > 0);

    if (!appName.trim()) {
      errorMsg = t('Vui lòng nhập tên ứng dụng hoặc nhóm dữ liệu.');
      return;
    }
    if (targetPaths.length === 0) {
      errorMsg = t('Vui lòng nhập ít nhất 1 đường dẫn thư mục cần kiểm soát hạn mức.');
      return;
    }

    const excludePatterns = excludesText
      .split(',')
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    const payload: QuotaRuleInputDto = {
      id: initialRule?.id ?? null,
      app_name: appName.trim(),
      process_name: processName.trim(),
      service_name: serviceName.trim(),
      target_paths: targetPaths,
      max_bytes: Math.round(Math.max(0.1, maxGb) * ONE_GIB),
      watermark_ratio: Math.min(95, Math.max(50, watermarkPct)) / 100,
      retention_days: Math.min(90, Math.max(1, retentionDays)),
      exclude_patterns: excludePatterns,
      enabled
    };

    isSaving = true;
    try {
      const saved = await saveQuotaRule(payload);
      onSaved(saved);
    } catch (err: unknown) {
      errorMsg = err instanceof Error ? err.message : String(err);
    } finally {
      isSaving = false;
    }
  }
</script>

<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm p-4">
  <form
    onsubmit={handleSubmit}
    class="w-full max-w-lg rounded-xl border border-border bg-card p-6 shadow-2xl space-y-4 max-h-full overflow-y-auto"
  >
    <div class="flex items-center justify-between border-b border-border pb-3">
      <div class="flex items-center gap-2">
        <ShieldCheck class="h-5 w-5 text-accent" />
        <h3 class="text-base font-bold text-primary">
          {initialRule?.id ? t('Chỉnh sửa quy tắc hạn mức') : t('Tạo quy tắc hạn mức tự động')}
        </h3>
      </div>
      <button
        type="button"
        onclick={onClose}
        class="p-1 rounded-md text-secondary hover:text-primary hover:bg-subtle"
        aria-label={t('Đóng')}
      >
        <X class="h-4 w-4" />
      </button>
    </div>

    {#if errorMsg}
      <div class="rounded-lg border border-danger/30 bg-danger/10 p-3 text-xs text-danger flex items-center gap-2">
        <AlertTriangle class="h-4 w-4 shrink-0" />
        <span>{errorMsg}</span>
      </div>
    {/if}

    <div class="space-y-3 text-xs">
      <div>
        <label for="qr-app-name" class="block font-medium text-primary mb-1">
          {t('Tên ứng dụng / Nhóm dữ liệu')}
        </label>
        <input
          id="qr-app-name"
          type="text"
          bind:value={appName}
          placeholder="Zalo / DingTalk / Temp Cache"
          class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-primary focus:border-accent outline-none"
        />
      </div>

      <div class="grid grid-cols-2 gap-3">
        <div>
          <label for="qr-proc" class="block font-medium text-primary mb-1">
            {t('Tiến trình (tùy chọn)')}
          </label>
          <input
            id="qr-proc"
            type="text"
            dir="ltr"
            bind:value={processName}
            placeholder="Zalo.exe"
            class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-primary ltr-isolate focus:border-accent outline-none"
          />
        </div>
        <div>
          <label for="qr-svc" class="block font-medium text-primary mb-1">
            {t('Windows Service (tùy chọn)')}
          </label>
          <input
            id="qr-svc"
            type="text"
            dir="ltr"
            bind:value={serviceName}
            placeholder="wuauserv"
            class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-primary ltr-isolate focus:border-accent outline-none"
          />
        </div>
      </div>

      <div>
        <label for="qr-paths" class="block font-medium text-primary mb-1">
          {t('Thư mục áp dụng (mỗi dòng 1 đường dẫn)')}
        </label>
        <textarea
          id="qr-paths"
          rows="3"
          dir="ltr"
          bind:value={pathsText}
          placeholder="C:\Users\...\AppData\Roaming\ZaloData"
          class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-primary font-mono ltr-isolate focus:border-accent outline-none"
        ></textarea>
      </div>

      <div class="grid grid-cols-3 gap-3">
        <div>
          <label for="qr-max-gb" class="block font-medium text-primary mb-1">
            {t('Trần dung lượng (GB)')}
          </label>
          <input
            id="qr-max-gb"
            type="number"
            dir="ltr"
            step="0.5"
            min="0.5"
            max="500"
            bind:value={maxGb}
            class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-primary tabular-numbers focus:border-accent outline-none"
          />
        </div>
        <div>
          <label for="qr-watermark" class="block font-medium text-primary mb-1">
            {t('Mức thu về (%)')}
          </label>
          <input
            id="qr-watermark"
            type="number"
            dir="ltr"
            min="50"
            max="95"
            bind:value={watermarkPct}
            class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-primary tabular-numbers focus:border-accent outline-none"
          />
        </div>
        <div>
          <label for="qr-retention" class="block font-medium text-primary mb-1">
            {t('Giữ dự phòng (ngày)')}
          </label>
          <input
            id="qr-retention"
            type="number"
            dir="ltr"
            min="1"
            max="90"
            bind:value={retentionDays}
            class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-primary tabular-numbers focus:border-accent outline-none"
          />
        </div>
      </div>

      <div>
        <label for="qr-excludes" class="block font-medium text-primary mb-1">
          {t('Mẫu loại trừ bổ sung (phân cách bằng dấu phẩy)')}
        </label>
        <input
          id="qr-excludes"
          type="text"
          dir="ltr"
          bind:value={excludesText}
          placeholder="*.key, *.lic"
          class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-primary ltr-isolate focus:border-accent outline-none"
        />
      </div>

      <label class="flex items-center gap-2 pt-1 cursor-pointer">
        <input
          type="checkbox"
          bind:checked={enabled}
          class="h-4 w-4 rounded border-border text-accent focus:ring-accent"
        />
        <span class="text-primary font-medium">
          {t('Bật giám sát ngầm tự động mỗi 45 giây')}
        </span>
      </label>
    </div>

    <div class="flex items-center justify-end gap-2 pt-3 border-t border-border">
      <button
        type="button"
        onclick={onClose}
        class="px-4 py-2 rounded-lg text-xs font-medium text-secondary hover:text-primary hover:bg-subtle"
      >
        {t('Hủy')}
      </button>
      <button
        type="submit"
        disabled={isSaving}
        class="px-5 py-2 rounded-lg text-xs font-semibold bg-accent hover:bg-accent-hover text-white shadow-sm disabled:opacity-50"
      >
        {isSaving ? t('Đang lưu...') : t('Lưu quy tắc')}
      </button>
    </div>
  </form>
</div>
