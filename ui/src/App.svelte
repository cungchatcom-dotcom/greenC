<script lang="ts">
  import { onMount } from 'svelte';
  import { i18n, t } from '$lib/i18n.svelte';
  import {
    getBootstrapState,
    listenWatchdogAlert,
    type BootstrapState,
    type WatchdogAlertEvent
  } from '$lib/api';
  import OverviewPage from './pages/OverviewPage.svelte';
  import CleanupPage from './pages/CleanupPage.svelte';
  import RelocatePage from './pages/RelocatePage.svelte';
  import CompactPage from './pages/CompactPage.svelte';
  import VaultPage from './pages/VaultPage.svelte';
  import SettingsPage from './pages/SettingsPage.svelte';
  import FirstRunModal from './lib/components/FirstRunModal.svelte';
  import { AlertTriangle, X } from 'lucide-svelte';

  type TabId = 'overview' | 'cleanup' | 'relocate' | 'compact' | 'vault' | 'settings';

  let activeTab = $state<TabId>('overview');
  let bootstrap = $state<BootstrapState | null>(null);
  let showFirstRun = $state(false);
  let activeAlert = $state<WatchdogAlertEvent | null>(null);

  const tabDefs: ReadonlyArray<{ id: TabId; labelKey: string }> = [
    { id: 'overview', labelKey: 'Tổng quan' },
    { id: 'cleanup', labelKey: 'Dọn dẹp' },
    { id: 'relocate', labelKey: 'Di chuyển' },
    { id: 'compact', labelKey: 'Thu gọn' },
    { id: 'vault', labelKey: 'Kho dự phòng' },
    { id: 'settings', labelKey: 'Cài đặt' }
  ];

  async function refreshBootstrap() {
    try {
      const b = await getBootstrapState();
      bootstrap = b;
      i18n.init(b.language, b.translations, b.theme_mode);
      showFirstRun = b.needs_first_run;
    } catch (err) {
      console.error('Bootstrap initialization warning:', err);
    }
  }

  onMount(() => {
    refreshBootstrap();
    const unlistenPromise = listenWatchdogAlert((alert) => {
      activeAlert = alert;
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  });
</script>

<div
  class={`flex h-screen w-screen overflow-hidden ${i18n.isDark ? 'dark' : ''}`}
  dir={i18n.isRtl ? 'rtl' : 'ltr'}
>
  <!-- Sidebar Navigation -->
  <aside class="w-64 border-e border-border bg-subtle p-4 flex flex-col justify-between shrink-0">
    <div>
      <div class="flex items-center gap-2 mb-6 px-2">
        <div class="h-6 w-6 rounded bg-accent flex items-center justify-center text-white text-xs font-bold">
          G
        </div>
        <span class="font-bold text-lg text-primary">GreenC</span>
      </div>
      <nav class="space-y-1">
        {#each tabDefs as tab}
          <button
            type="button"
            class={`w-full text-start px-3 py-2 rounded-md font-medium text-sm transition-colors ${
              activeTab === tab.id
                ? 'bg-card text-accent shadow-sm'
                : 'text-secondary hover:bg-card/50'
            }`}
            onclick={() => (activeTab = tab.id)}
          >
            {t(tab.labelKey)}
          </button>
        {/each}
      </nav>
    </div>
    <div dir="ltr" class="text-xs text-disabled px-2 ltr-isolate">
      GreenC v{bootstrap?.app_version ?? '2.0.0'} (Rust)
    </div>
  </aside>

  <!-- Main Content Area -->
  <main class="flex-1 flex flex-col overflow-hidden bg-canvas">
    <header class="h-12 border-b border-border bg-card px-6 flex items-center justify-between shrink-0">
      <h1 class="text-base font-semibold text-primary">
        {t(tabDefs.find((item) => item.id === activeTab)?.labelKey ?? 'Tổng quan')}
      </h1>
      {#if bootstrap?.current_data_root}
        <span dir="ltr" class="text-xs text-secondary font-mono ltr-isolate">
          {bootstrap.current_data_root}
        </span>
      {/if}
    </header>

    {#if activeAlert}
      <div
        class={`mx-6 mt-4 rounded-lg border px-4 py-2.5 text-xs flex items-center justify-between gap-3 ${
          activeAlert.severity === 'critical'
            ? 'border-danger/30 bg-danger/10 text-danger'
            : 'border-accent/30 bg-accent/10 text-primary'
        }`}
      >
        <div class="flex items-center gap-2 min-w-0">
          <AlertTriangle class="h-4 w-4 shrink-0" />
          <div class="truncate">
            <strong>{activeAlert.title}</strong> — {activeAlert.detail}
          </div>
        </div>
        <button
          type="button"
          onclick={() => (activeAlert = null)}
          class="p-1 rounded hover:bg-black/5"
          aria-label={t('Đóng')}
        >
          <X class="h-3.5 w-3.5" />
        </button>
      </div>
    {/if}

    <div class="flex-1 overflow-auto p-6">
      {#if activeTab === 'overview'}
        <OverviewPage onNavigateCleanup={() => (activeTab = 'cleanup')} />
      {:else if activeTab === 'cleanup'}
        <CleanupPage />
      {:else if activeTab === 'relocate'}
        <RelocatePage />
      {:else if activeTab === 'compact'}
        <CompactPage />
      {:else if activeTab === 'vault'}
        <VaultPage />
      {:else}
        <SettingsPage />
      {/if}
    </div>
  </main>
</div>

{#if showFirstRun && bootstrap}
  <FirstRunModal
    drives={bootstrap.drives}
    recommendedDrive={bootstrap.recommended_drive}
    initialRetentionDays={bootstrap.retention_days}
    onCompleted={() => {
      showFirstRun = false;
      refreshBootstrap();
    }}
    onDefer={() => {
      showFirstRun = false;
    }}
  />
{/if}
