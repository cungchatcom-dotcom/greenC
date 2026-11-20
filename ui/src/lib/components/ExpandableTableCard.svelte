<!-- ui/src/lib/components/ExpandableTableCard.svelte -->
<script lang="ts">
  import { t, formatSize } from '$lib/i18n.svelte';
  import { revealInExplorer, type ReclaimableItemDto } from '$lib/api';
  import { Eye, EyeOff, Copy, ExternalLink, Check } from 'lucide-svelte';

  interface Props {
    title: string;
    items: ReclaimableItemDto[];
    emptyText?: string;
  }

  let { title, items, emptyText }: Props = $props();

  let isDetailMode = $state(false);
  let copiedToast = $state(false);

  async function copyPath(path: string) {
    try {
      await navigator.clipboard.writeText(path);
      copiedToast = true;
      setTimeout(() => (copiedToast = false), 1200);
    } catch {}
  }
</script>

<div class="rounded-xl border border-border bg-card shadow-sm overflow-hidden flex flex-col">
  <!-- Card Header -->
  <div class="flex items-center justify-between px-5 py-4 border-b border-border">
    <div class="flex items-center gap-2">
      <h3 class="font-bold text-base text-primary">{title}</h3>
      {#if items.length > 0}
        <span class="px-2 py-0.5 rounded-full text-xs bg-subtle text-secondary font-medium">
          {items.length}
        </span>
      {/if}
    </div>

    <!-- Eye Toggle Button -->
    <button
      type="button"
      onclick={() => (isDetailMode = !isDetailMode)}
      class="flex items-center gap-1.5 text-xs text-secondary hover:text-primary px-2.5 py-1.5 rounded-md hover:bg-subtle transition-colors"
      title={isDetailMode ? t('Ẩn chi tiết') : t('Xem chi tiết')}
    >
      {#if isDetailMode}
        <EyeOff class="h-4 w-4" />
        <span>{t('Ẩn chi tiết')}</span>
      {:else}
        <Eye class="h-4 w-4" />
        <span>{t('Xem chi tiết')}</span>
      {/if}
    </button>
  </div>

  <!-- Content Table -->
  {#if items.length === 0}
    <div class="p-8 text-center text-xs text-secondary">
      {emptyText || t('Không có mục nào')}
    </div>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-start text-xs border-collapse">
        <thead>
          <tr class="border-b border-border bg-subtle/50 text-secondary font-semibold">
            <th class="py-2.5 px-4 text-start">{t('Ứng dụng')}</th>
            {#if isDetailMode}
              <th class="py-2.5 px-4 text-start">{t('Vị trí')}</th>
            {/if}
            <th class="py-2.5 px-4 text-end">{t('Dung lượng')}</th>
            {#if isDetailMode}
              <th class="py-2.5 px-4 text-start">{t('Giải phóng')}</th>
            {/if}
            <th class="py-2.5 px-4 text-start">{t('Đề xuất')}</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-border/60">
          {#each items as item}
            <tr class="hover:bg-subtle/40 transition-colors">
              <td class="py-3 px-4 font-medium text-primary">{item.app_name}</td>

              {#if isDetailMode}
                <td class="py-3 px-4 max-w-xs">
                  <div class="flex items-center gap-1.5 text-secondary">
                    <span class="truncate font-mono text-[11px]" title={item.path}>{item.path}</span>
                    <button
                      type="button"
                      onclick={() => copyPath(item.path)}
                      class="hover:text-primary transition-colors p-1"
                      title={t('Sao chép đường dẫn')}
                    >
                      <Copy class="h-3 w-3" />
                    </button>
                    <button
                      type="button"
                      onclick={() => revealInExplorer(item.path)}
                      class="hover:text-primary transition-colors p-1"
                      title={t('Mở trong Explorer')}
                    >
                      <ExternalLink class="h-3 w-3" />
                    </button>
                  </div>
                </td>
              {/if}

              <td class="py-3 px-4 text-end tabular-numbers text-primary font-medium">
                {formatSize(item.size_bytes)}
              </td>

              {#if isDetailMode}
                <td class="py-3 px-4 text-secondary capitalize">{item.category}</td>
              {/if}

              <td class="py-3 px-4">
                <span class="px-2 py-0.5 rounded text-[11px] font-medium bg-accent/10 text-accent border border-accent/20">
                  {item.recommendation}
                </span>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<!-- Toast Notification -->
{#if copiedToast}
  <div class="fixed bottom-6 right-6 z-50 bg-primary text-card px-3.5 py-2 rounded-lg text-xs font-medium shadow-lg flex items-center gap-2">
    <Check class="h-3.5 w-3.5 text-accent" />
    {t('Đã sao chép đường dẫn')}
  </div>
{/if}
