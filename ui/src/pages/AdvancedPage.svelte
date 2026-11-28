<!-- ui/src/pages/AdvancedPage.svelte: Power-user sub-tabs (Relocate, Cleanup, Compact, Disk Map) -->
<script lang="ts">
  import { t } from '$lib/i18n.svelte';
  import RelocatePage from './RelocatePage.svelte';
  import CleanupPage from './CleanupPage.svelte';
  import CompactPage from './CompactPage.svelte';
  import OverviewPage from './OverviewPage.svelte';
  import { ArrowRightLeft, Sparkles, Minimize, HardDrive } from 'lucide-svelte';

  type SubTab = 'relocate' | 'cleanup' | 'compact' | 'diskmap';

  interface Props {
    initialSubTab?: SubTab;
    goalRefreshKey?: number;
    onNavigateSettings?: () => void;
  }

  let { initialSubTab = 'relocate', goalRefreshKey = 0, onNavigateSettings }: Props = $props();
  let subTab = $state<SubTab>('relocate');

  $effect(() => {
    subTab = initialSubTab;
  });

  const subTabs: ReadonlyArray<{ id: SubTab; labelKey: string; icon: typeof ArrowRightLeft }> = [
    { id: 'relocate', labelKey: 'Di chuyển', icon: ArrowRightLeft },
    { id: 'cleanup', labelKey: 'Dọn dẹp chi tiết', icon: Sparkles },
    { id: 'compact', labelKey: 'Thu gọn', icon: Minimize },
    { id: 'diskmap', labelKey: 'Bản đồ ổ đĩa', icon: HardDrive },
  ];
</script>

<div class="max-w-6xl mx-auto w-full space-y-5">
  <!-- Compact Sub-navigation Bar -->
  <div class="flex items-center justify-between gap-3 flex-wrap">
    <div>
      <h2 class="text-lg font-bold text-primary">{t('Tùy chỉnh nâng cao')}</h2>
      <p class="text-xs text-secondary">
        {t('Dành cho người muốn tự chọn từng thư mục để di chuyển, dọn dẹp hoặc nén gọn.')}
      </p>
    </div>

    <div class="inline-flex items-center gap-1 p-1 rounded-xl border border-border bg-subtle">
      {#each subTabs as tab}
        {@const Icon = tab.icon}
        <button
          type="button"
          onclick={() => (subTab = tab.id)}
          class={`inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors ${
            subTab === tab.id
              ? 'bg-card text-accent shadow-sm'
              : 'text-secondary hover:text-primary'
          }`}
        >
          <Icon class="h-3.5 w-3.5" />
          <span>{t(tab.labelKey)}</span>
        </button>
      {/each}
    </div>
  </div>

  {#if subTab === 'relocate'}
    <RelocatePage />
  {:else if subTab === 'cleanup'}
    <CleanupPage />
  {:else if subTab === 'compact'}
    <CompactPage />
  {:else}
    <OverviewPage
      {goalRefreshKey}
      onNavigateCleanup={() => (subTab = 'cleanup')}
      {onNavigateSettings}
    />
  {/if}
</div>
