<script lang="ts">
  import { onMount } from "svelte";
  import { i18n, t } from "$lib/i18n.svelte";
  import {
    getBootstrapState,
    hideToTray,
    listenWatchdogAlert,
    checkForUpdates,
    downloadAndInstallUpdate,
    type BootstrapState,
    type WatchdogAlertEvent,
    type UpdateCheckDto,
  } from "$lib/api";
  import { taskStore } from "$lib/tasks.svelte";
  import HomePage from "./pages/HomePage.svelte";
  import RestorePage from "./pages/RestorePage.svelte";
  import AdvancedPage from "./pages/AdvancedPage.svelte";
  import SettingsPage from "./pages/SettingsPage.svelte";
  import FirstRunModal from "./lib/components/FirstRunModal.svelte";
  import BrandLogo from "./lib/components/BrandLogo.svelte";
  import TaskProgressOverlay from "./lib/components/TaskProgressOverlay.svelte";
  import {
    Home,
    RotateCcw,
    SlidersHorizontal,
    Settings,
    Minimize2,
    AlertTriangle,
    X,
    Download,
    RefreshCw,
  } from "lucide-svelte";

  type NavView = "home" | "restore" | "advanced" | "settings";

  let activeView = $state<NavView>("home");
  let bootstrap = $state<BootstrapState | null>(null);
  let showFirstRun = $state(false);
  let activeAlert = $state<WatchdogAlertEvent | null>(null);
  let autostartEnabled = $state(false);
  let goalRefreshKey = $state(0);
  let updateInfo = $state<UpdateCheckDto | null>(null);
  let updateDismissed = $state(false);
  let installingUpdate = $state(false);
  let updateError = $state<string | null>(null);

  const primaryTabs: ReadonlyArray<{ id: NavView; labelKey: string; icon: typeof Home }> = [
    { id: "home", labelKey: "Trang chủ", icon: Home },
    { id: "restore", labelKey: "Khôi phục", icon: RotateCcw },
    { id: "advanced", labelKey: "Tùy chỉnh", icon: SlidersHorizontal },
  ];

  async function refreshBootstrap() {
    try {
      const b = await getBootstrapState();
      bootstrap = b;
      autostartEnabled = Boolean(b.autostart_enabled);
      i18n.init(b.language, b.translations, b.theme_mode);
      showFirstRun = b.needs_first_run;
      goalRefreshKey += 1;
    } catch (err) {
      console.error("Bootstrap initialization warning:", err);
    }
  }

  async function handleHideToTray() {
    try {
      await hideToTray();
    } catch (err) {
      console.error("Failed to hide to tray:", err);
    }
  }

  async function handleInstallUpdate() {
    if (
      !updateInfo?.download_url ||
      !updateInfo?.asset_name ||
      installingUpdate
    )
      return;
    installingUpdate = true;
    updateError = null;
    try {
      await downloadAndInstallUpdate(
        updateInfo.download_url,
        updateInfo.asset_name,
      );
    } catch (err) {
      updateError = err instanceof Error ? err.message : String(err);
    } finally {
      installingUpdate = false;
    }
  }

  onMount(() => {
    const unlistenTasks = taskStore.initListener();
    let updateTimer: number | undefined;
    void refreshBootstrap().then(() => {
      if (bootstrap?.is_store_build) return;
      updateTimer = window.setTimeout(async () => {
        try {
          const res = await checkForUpdates();
          if (res.has_update && !res.managed_by_store) {
            updateInfo = res;
          }
        } catch {
          // Offline or rate-limited
        }
      }, 4000);
    });
    const unlistenAlert = listenWatchdogAlert((alert) => {
      activeAlert = alert;
    });
    return () => {
      unlistenTasks();
      if (updateTimer !== undefined) {
        window.clearTimeout(updateTimer);
      }
      unlistenAlert.then((unlisten) => unlisten());
    };
  });
</script>

<div
  class={`flex flex-col h-screen w-screen overflow-hidden bg-canvas ${i18n.isDark ? "dark" : ""}`}
  dir={i18n.isRtl ? "rtl" : "ltr"}
>
  <!-- Compact Top Navigation Bar (No Left Sidebar) -->
  <header
    class="h-14 border-b border-border bg-card px-4 sm:px-6 flex items-center justify-between gap-3 shrink-0"
  >
    <!-- Brand -->
    <button
      type="button"
      onclick={() => (activeView = "home")}
      class="flex items-center gap-2 text-left shrink-0"
    >
      <BrandLogo size={24} />
      <span class="font-bold text-base text-primary tracking-tight">GreenC</span>
    </button>

    <!-- Primary Compact Icon Tabs -->
    <nav class="flex items-center gap-1 bg-subtle/80 p-1 rounded-xl border border-border/80">
      {#each primaryTabs as tab}
        {@const Icon = tab.icon}
        <button
          type="button"
          onclick={() => (activeView = tab.id)}
          class={`inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold transition-colors ${
            activeView === tab.id
              ? "bg-card text-accent shadow-sm"
              : "text-secondary hover:text-primary"
          }`}
        >
          <Icon class="h-4 w-4" />
          <span class="hidden sm:inline">{t(tab.labelKey)}</span>
        </button>
      {/each}
    </nav>

    <!-- Right Compact Controls: Background Task Pill, Settings, Hide to Tray -->
    <div class="flex items-center gap-2 shrink-0">
      {#if taskStore.active && taskStore.active.minimized}
        <button
          type="button"
          onclick={() => taskStore.restore()}
          class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-accent/15 text-accent border border-accent/30 text-xs font-semibold hover:bg-accent/25 transition-colors"
          title={t("Mở xem tiến trình đang chạy ngầm")}
        >
          <RefreshCw class="h-3.5 w-3.5 animate-spin" />
          <span class="tabular-numbers">{taskStore.active.percent}%</span>
        </button>
      {/if}

      <button
        type="button"
        onclick={() => (activeView = activeView === "settings" ? "home" : "settings")}
        class={`p-2 rounded-lg border transition-colors ${
          activeView === "settings"
            ? "border-accent/40 bg-accent/10 text-accent"
            : "border-border bg-subtle hover:bg-card text-secondary hover:text-primary"
        }`}
        title={t("Cài đặt")}
        aria-label={t("Cài đặt")}
      >
        <Settings class="h-4 w-4" />
      </button>

      <button
        type="button"
        onclick={handleHideToTray}
        class="inline-flex items-center gap-1.5 px-2.5 py-2 rounded-lg border border-border bg-subtle hover:bg-card text-xs font-medium text-primary transition-colors"
        title={t("Thu gọn xuống khay hệ thống và chạy ngầm (< 15 MB RAM)")}
      >
        <Minimize2 class="h-4 w-4 text-accent" />
        <span class="hidden md:inline">{t("Chạy ngầm")}</span>
      </button>
    </div>
  </header>

  <!-- Optional Update Notification Strip -->
  {#if !bootstrap?.is_store_build && updateInfo?.has_update && !updateInfo?.managed_by_store && !updateDismissed}
    <div
      class="mx-4 sm:mx-6 mt-3 rounded-xl border border-accent/35 bg-accent/10 px-4 py-2 text-xs flex items-center justify-between gap-3 max-w-6xl w-auto self-center"
    >
      <div class="flex items-center gap-2 min-w-0 text-primary">
        <Download class="h-4 w-4 shrink-0 text-accent" />
        <div class="truncate">
          <strong>{t("Có phiên bản mới")} v{updateInfo.latest_version}</strong>
          {#if updateError}
            <span class="text-danger ms-2">— {updateError}</span>
          {:else}
            <span class="text-secondary ms-1.5"
              >({t("Hiện tại")}: v{updateInfo.current_version})</span
            >
          {/if}
        </div>
      </div>
      <div class="flex items-center gap-2 shrink-0">
        {#if updateInfo.download_url && updateInfo.asset_name}
          <button
            type="button"
            disabled={installingUpdate}
            onclick={handleInstallUpdate}
            class="px-3 py-1 rounded-md bg-accent hover:bg-accent-hover text-white font-semibold disabled:opacity-60"
          >
            {installingUpdate ? t("Đang tải bản cập nhật...") : t("Cập nhật ngay")}
          </button>
        {:else}
          <a
            href={updateInfo.release_url}
            target="_blank"
            rel="noreferrer"
            class="px-3 py-1 rounded-md bg-accent hover:bg-accent-hover text-white font-semibold"
          >
            {t("Tải bản mới")}
          </a>
        {/if}
        <button
          type="button"
          onclick={() => (updateDismissed = true)}
          class="p-1 rounded hover:bg-black/5 text-secondary"
          aria-label={t("Đóng")}
        >
          <X class="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  {/if}

  <!-- Optional Watchdog Alert Strip -->
  {#if activeAlert}
    <div
      class={`mx-4 sm:mx-6 mt-3 rounded-xl border px-4 py-2 text-xs flex items-center justify-between gap-3 ${
        activeAlert.severity === "critical"
          ? "border-danger/30 bg-danger/10 text-danger"
          : "border-accent/30 bg-accent/10 text-primary"
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
        aria-label={t("Đóng")}
      >
        <X class="h-3.5 w-3.5" />
      </button>
    </div>
  {/if}

  <!-- Full-Width Content Canvas -->
  <main class="flex-1 overflow-auto px-4 sm:px-6 py-5">
    {#if activeView === "home"}
      <HomePage
        {goalRefreshKey}
        onNavigateRestore={() => (activeView = "restore")}
        onNavigateAdvanced={() => (activeView = "advanced")}
        onNavigateSettings={() => (activeView = "settings")}
      />
    {:else if activeView === "restore"}
      <RestorePage />
    {:else if activeView === "advanced"}
      <AdvancedPage
        {goalRefreshKey}
        onNavigateSettings={() => (activeView = "settings")}
      />
    {:else}
      <div class="max-w-5xl mx-auto w-full">
        <SettingsPage
          {autostartEnabled}
          onAutostartChanged={(v) => (autostartEnabled = v)}
        />
      </div>
    {/if}
  </main>
</div>

<!-- Global Background-Capable Progress Overlay & Toast -->
<TaskProgressOverlay />

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
