<script lang="ts">
  import { onMount } from "svelte";
  import { i18n, t } from "$lib/i18n.svelte";
  import {
    getBootstrapState,
    hideToTray,
    listenWatchdogAlert,
    updateSettings,
    checkForUpdates,
    downloadAndInstallUpdate,
    type BootstrapState,
    type WatchdogAlertEvent,
    type UpdateCheckDto,
  } from "$lib/api";
  import OverviewPage from "./pages/OverviewPage.svelte";
  import CleanupPage from "./pages/CleanupPage.svelte";
  import RelocatePage from "./pages/RelocatePage.svelte";
  import CompactPage from "./pages/CompactPage.svelte";
  import VaultPage from "./pages/VaultPage.svelte";
  import SettingsPage from "./pages/SettingsPage.svelte";
  import FirstRunModal from "./lib/components/FirstRunModal.svelte";
  import BrandLogo from "./lib/components/BrandLogo.svelte";
  import GoalContextStrip from "./lib/components/GoalContextStrip.svelte";
  import { AlertTriangle, X, Minimize2, Power, Download } from "lucide-svelte";

  type TabId =
    | "overview"
    | "cleanup"
    | "relocate"
    | "compact"
    | "vault"
    | "settings";

  let activeTab = $state<TabId>("overview");
  let bootstrap = $state<BootstrapState | null>(null);
  let showFirstRun = $state(false);
  let activeAlert = $state<WatchdogAlertEvent | null>(null);
  let autostartEnabled = $state(false);
  let togglingAutostart = $state(false);
  let goalRefreshKey = $state(0);
  let updateInfo = $state<UpdateCheckDto | null>(null);
  let updateDismissed = $state(false);
  let installingUpdate = $state(false);
  let updateError = $state<string | null>(null);

  const tabDefs: ReadonlyArray<{ id: TabId; labelKey: string }> = [
    { id: "overview", labelKey: "Tổng quan" },
    { id: "cleanup", labelKey: "Dọn dẹp" },
    { id: "relocate", labelKey: "Di chuyển" },
    { id: "compact", labelKey: "Thu gọn" },
    { id: "vault", labelKey: "Kho dự phòng" },
    { id: "settings", labelKey: "Cài đặt" },
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

  async function handleToggleAutostart() {
    if (togglingAutostart) return;
    togglingAutostart = true;
    const next = !autostartEnabled;
    try {
      const updated = await updateSettings({ autostart_enabled: next });
      autostartEnabled = updated.autostart_enabled;
    } catch (err) {
      console.error("Failed to toggle autostart:", err);
      activeAlert = {
        kind: "autostart_error",
        severity: "critical",
        title: t("Khởi động cùng hệ điều hành"),
        detail: err instanceof Error ? err.message : String(err),
        archive_id: null,
      };
    } finally {
      togglingAutostart = false;
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
    refreshBootstrap();
    const unlistenPromise = listenWatchdogAlert((alert) => {
      activeAlert = alert;
    });
    // Silently auto-check for new releases 4s after startup
    const updateTimer = window.setTimeout(async () => {
      try {
        const res = await checkForUpdates();
        if (res.has_update) {
          updateInfo = res;
        }
      } catch {
        // Offline or rate-limited — stay silent on auto-check
      }
    }, 4000);
    return () => {
      window.clearTimeout(updateTimer);
      unlistenPromise.then((unlisten) => unlisten());
    };
  });
</script>

<div
  class={`flex h-screen w-screen overflow-hidden ${i18n.isDark ? "dark" : ""}`}
  dir={i18n.isRtl ? "rtl" : "ltr"}
>
  <!-- Sidebar Navigation -->
  <aside
    class="w-64 border-e border-border bg-subtle p-4 flex flex-col justify-between shrink-0"
  >
    <div>
      <div class="flex items-center gap-2.5 mb-6 px-2">
        <BrandLogo size={28} />
        <div class="leading-tight">
          <div class="font-bold text-base text-primary">GreenC</div>
          <div class="text-[10px] text-secondary">{t("Ổ đĩa luôn xanh")}</div>
        </div>
      </div>
      <nav class="space-y-1">
        {#each tabDefs as tab}
          <button
            type="button"
            class={`w-full text-start px-3 py-2 rounded-md font-medium text-sm transition-colors ${
              activeTab === tab.id
                ? "bg-card text-accent shadow-sm"
                : "text-secondary hover:bg-card/50"
            }`}
            onclick={() => (activeTab = tab.id)}
          >
            {t(tab.labelKey)}
          </button>
        {/each}
      </nav>
    </div>

    <!-- Bottom Quick Controls: Autostart & Version -->
    <div class="space-y-3 pt-3 border-t border-border/60">
      <button
        type="button"
        disabled={togglingAutostart}
        onclick={handleToggleAutostart}
        class="w-full flex items-center justify-between gap-2 px-2.5 py-2 rounded-lg border border-border bg-card hover:bg-subtle/80 text-xs transition-colors disabled:opacity-60"
        title={t("Khởi động cùng hệ điều hành")}
      >
        <span
          class="flex items-center gap-1.5 text-primary font-medium truncate"
        >
          <Power
            class={`h-3.5 w-3.5 shrink-0 ${autostartEnabled ? "text-accent" : "text-disabled"}`}
          />
          <span class="truncate">{t("Khởi động cùng OS")}</span>
        </span>
        <span
          class={`px-1.5 py-0.5 rounded text-[10px] font-bold shrink-0 ${
            autostartEnabled
              ? "bg-accent/15 text-accent"
              : "bg-subtle text-disabled"
          }`}
        >
          {autostartEnabled ? t("Bật") : t("Tắt")}
        </span>
      </button>

      <div dir="ltr" class="text-xs text-disabled px-2 ltr-isolate">
        GreenC v{bootstrap?.app_version ?? "2.1.2"} (Rust)
      </div>
    </div>
  </aside>

  <!-- Main Content Area -->
  <main class="flex-1 flex flex-col overflow-hidden bg-canvas">
    <header
      class="h-12 border-b border-border bg-card px-6 flex items-center justify-between gap-3 shrink-0"
    >
      <h1 class="text-base font-semibold text-primary">
        {t(
          tabDefs.find((item) => item.id === activeTab)?.labelKey ??
            "Tổng quan",
        )}
      </h1>

      <div class="flex items-center gap-3">
        {#if bootstrap?.current_data_root}
          <span
            dir="ltr"
            class="hidden md:inline text-xs text-secondary font-mono ltr-isolate"
          >
            {bootstrap.current_data_root}
          </span>
        {/if}

        <button
          type="button"
          onclick={handleHideToTray}
          class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border bg-subtle hover:bg-card text-xs font-medium text-primary transition-colors"
          title={t("Thu gọn xuống khay hệ thống và chạy ngầm (< 15 MB RAM)")}
        >
          <Minimize2 class="h-3.5 w-3.5 text-accent" />
          <span>{t("Chạy ngầm")}</span>
        </button>
      </div>
    </header>

    {#if updateInfo?.has_update && !updateDismissed}
      <div
        class="mx-6 mt-4 rounded-lg border border-accent/35 bg-accent/10 px-4 py-2.5 text-xs flex items-center justify-between gap-3"
      >
        <div class="flex items-center gap-2 min-w-0 text-primary">
          <Download class="h-4 w-4 shrink-0 text-accent" />
          <div class="truncate">
            <strong>{t("Có phiên bản mới")} v{updateInfo.latest_version}</strong
            >
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
              {installingUpdate
                ? t("Đang tải bản cập nhật...")
                : t("Cập nhật ngay")}
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

    {#if activeAlert}
      <div
        class={`mx-6 mt-4 rounded-lg border px-4 py-2.5 text-xs flex items-center justify-between gap-3 ${
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

    <div class="flex-1 overflow-auto p-6">
      {#if activeTab === "overview"}
        <OverviewPage
          {goalRefreshKey}
          onNavigateCleanup={() => (activeTab = "cleanup")}
          onNavigateSettings={() => (activeTab = "settings")}
        />
      {:else if activeTab === "cleanup"}
        <GoalContextStrip
          mode="strip"
          refreshKey={goalRefreshKey}
          onConfigureGoal={() => (activeTab = "settings")}
        />
        <CleanupPage />
      {:else if activeTab === "relocate"}
        <GoalContextStrip
          mode="strip"
          refreshKey={goalRefreshKey}
          onConfigureGoal={() => (activeTab = "settings")}
        />
        <RelocatePage />
      {:else if activeTab === "compact"}
        <GoalContextStrip
          mode="strip"
          refreshKey={goalRefreshKey}
          onConfigureGoal={() => (activeTab = "settings")}
        />
        <CompactPage />
      {:else if activeTab === "vault"}
        <GoalContextStrip
          mode="strip"
          refreshKey={goalRefreshKey}
          onConfigureGoal={() => (activeTab = "settings")}
        />
        <VaultPage />
      {:else}
        <SettingsPage
          {autostartEnabled}
          onAutostartChanged={(v) => (autostartEnabled = v)}
        />
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
