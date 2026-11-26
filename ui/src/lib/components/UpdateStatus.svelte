<!-- ui/src/lib/components/UpdateStatus.svelte: says how current the shown result is -->
<script lang="ts">
  import { t, i18n } from '$lib/i18n.svelte';
  import { RefreshCw, AlertTriangle } from 'lucide-svelte';

  interface Props {
    savedAt: number | null;
    refreshing: boolean;
    failed?: boolean;
  }

  let { savedAt, refreshing, failed = false }: Props = $props();

  function when(at: number): string {
    const options: Intl.DateTimeFormatOptions = {
      day: '2-digit',
      month: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    };
    try {
      return new Date(at).toLocaleString(i18n.locale.replace('_', '-'), options);
    } catch {
      return new Date(at).toLocaleString(undefined, options);
    }
  }
</script>

{#if failed}
  <p class="flex items-center gap-1.5 text-xs text-amber-600" role="status">
    <AlertTriangle class="h-3.5 w-3.5 shrink-0" />
    <span>{t('Không cập nhật được. Hãy thử quét lại.')}</span>
    {#if savedAt !== null}
      <span class="text-disabled">{t('Cập nhật lúc {time}', { time: when(savedAt) })}</span>
    {/if}
  </p>
{:else if savedAt !== null}
  <p class="flex items-center gap-1.5 text-xs text-secondary" role="status">
    {#if refreshing}
      <RefreshCw class="h-3.5 w-3.5 shrink-0 animate-spin text-accent" />
      <span>{t('Đang cập nhật, đang hiện kết quả lúc {time}', { time: when(savedAt) })}</span>
    {:else}
      <span>{t('Cập nhật lúc {time}', { time: when(savedAt) })}</span>
    {/if}
  </p>
{/if}
