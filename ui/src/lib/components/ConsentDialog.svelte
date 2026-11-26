<!-- ui/src/lib/components/ConsentDialog.svelte -->
<script lang="ts">
  import { t } from '$lib/i18n.svelte';
  import { previewDiagnostics } from '$lib/api';
  import { X, ShieldCheck } from 'lucide-svelte';

  let {
    onChoose,
    onDismiss,
  }: {
    onChoose: (granted: boolean) => void;
    onDismiss: () => void;
  } = $props();

  let showPreview = $state(false);
  let previewJson = $state('');
  let loadingPreview = $state(false);
  let submitting = $state(false);

  async function togglePreview() {
    showPreview = !showPreview;
    if (showPreview && !previewJson && !loadingPreview) {
      loadingPreview = true;
      try {
        previewJson = await previewDiagnostics();
      } finally {
        loadingPreview = false;
      }
    }
  }

  function handleChoose(granted: boolean) {
    if (submitting) return;
    submitting = true;
    onChoose(granted);
  }

  function handleDismiss() {
    if (submitting) return;
    submitting = true;
    onDismiss();
  }
</script>

<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
  role="dialog"
  aria-modal="true"
  aria-labelledby="diagnostics-consent-title"
>
  <div
    class="w-full max-w-lg rounded-xl border border-border bg-card shadow-xl p-5 space-y-4 text-primary"
  >
    <div class="flex items-start justify-between gap-3">
      <div class="flex items-center gap-2">
        <ShieldCheck class="h-5 w-5 text-accent shrink-0" />
        <h2 id="diagnostics-consent-title" class="text-base font-bold">
          {t('Giúp GreenC chạy tốt hơn?')}
        </h2>
      </div>
      <button
        type="button"
        onclick={handleDismiss}
        aria-label={t('Đóng')}
        class="p-1 rounded-md text-secondary hover:text-primary hover:bg-subtle transition-colors"
      >
        <X class="h-4 w-4" />
      </button>
    </div>

    <p class="text-xs text-secondary leading-relaxed">
      {t(
        'Gửi số đếm ẩn danh về cách các tính năng hoạt động (tốc độ quét, mã lỗi, vị trí lỗi trong mã nguồn GreenC) để cải thiện ứng dụng. Không thu thập tên tệp, đường dẫn, mã thiết bị hay thông tin cá nhân.',
      )}
    </p>

    <div>
      <button
        type="button"
        onclick={togglePreview}
        class="text-xs font-medium text-accent hover:underline"
      >
        {showPreview ? t('Ẩn dữ liệu sẽ gửi') : t('Xem dữ liệu sẽ gửi')}
      </button>

      {#if showPreview}
        <div class="mt-2.5 space-y-2 rounded-lg border border-border bg-subtle p-3 text-xs">
          <p class="text-secondary leading-relaxed">
            {t(
              'Dữ liệu gồm: ngày (UTC), phiên bản ứng dụng, hệ điều hành, loại sự kiện, tên tính năng, kết quả, mã lỗi chuẩn hoá, khoảng thời gian, nhóm dung lượng ổ hệ thống và số lần.',
            )}
          </p>
          {#if previewJson}
            <pre
              dir="ltr"
              class="max-h-48 overflow-auto rounded border border-border bg-card p-2.5 text-[11px] font-mono text-primary ltr-isolate"
            >{previewJson}</pre>
          {/if}
        </div>
      {/if}
    </div>

    <div class="grid grid-cols-2 gap-3 pt-1">
      <button
        type="button"
        disabled={submitting}
        onclick={() => handleChoose(true)}
        class="px-4 py-2 rounded-lg border border-border bg-subtle hover:bg-card text-sm font-semibold text-primary transition-colors disabled:opacity-60"
      >
        {t('Đồng ý gửi')}
      </button>
      <button
        type="button"
        disabled={submitting}
        onclick={() => handleChoose(false)}
        class="px-4 py-2 rounded-lg border border-border bg-subtle hover:bg-card text-sm font-semibold text-primary transition-colors disabled:opacity-60"
      >
        {t('Không')}
      </button>
    </div>
  </div>
</div>
