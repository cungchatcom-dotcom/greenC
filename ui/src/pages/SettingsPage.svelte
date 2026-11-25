<!-- ui/src/pages/SettingsPage.svelte (Task T066) -->
<script lang="ts">
  import { onMount } from "svelte";
  import { i18n, t, formatSize } from "$lib/i18n.svelte";
  import {
    getAppSettings,
    updateSettings,
    changeDataRoot,
    listQuotaRules,
    deleteQuotaRule,
    saveQuotaRule,
    runSandboxDemo,
    getBootstrapState,
    checkForUpdates,
    downloadAndInstallUpdate,
    type AppSettingsDto,
    type QuotaRuleStatusDto,
    type DriveInfo,
    type SandboxDemoResultDto,
    type UpdateCheckDto,
  } from "$lib/api";
  import BrandLogo from "$lib/components/BrandLogo.svelte";
  import DriveQuotaCard from "$lib/components/DriveQuotaCard.svelte";
  import QuotaRuleModal, {
    type QuotaRulePrefill,
  } from "$lib/components/QuotaRuleModal.svelte";
  import {
    Globe,
    SunMoon,
    HardDrive,
    Shield,
    Plus,
    Trash2,
    Edit3,
    FlaskConical,
    Info,
    CheckCircle2,
    AlertTriangle,
    RefreshCw,
    Download,
  } from "lucide-svelte";

  let {
    autostartEnabled = undefined,
    onAutostartChanged,
  }: {
    autostartEnabled?: boolean;
    onAutostartChanged?: (enabled: boolean) => void;
  } = $props();

  let settings = $state<AppSettingsDto | null>(null);
  let drives = $state<DriveInfo[]>([]);
  let refreshingDrives = $state(false);
  let rules = $state<QuotaRuleStatusDto[]>([]);
  let loadingRules = $state(true);
  let rulesLoadError = $state<string | null>(null);
  let newDataRoot = $state("");
  let isMovingVault = $state(false);
  let statusBanner = $state<{ kind: "ok" | "err"; text: string } | null>(null);
  let editingRule = $state<QuotaRulePrefill | null>(null);
  let showRuleModal = $state(false);
  let demoRunning = $state(false);
  let demoResult = $state<SandboxDemoResultDto | null>(null);
  let checkingUpdate = $state(false);
  let installingUpdate = $state(false);
  let updateResult = $state<UpdateCheckDto | null>(null);
  let updateCheckError = $state<string | null>(null);

  // Explicit draft state for Card 3 (Background Guardian & Autostart) with dedicated Save button
  let draftAutostart = $state(false);
  let draftPauseBattery = $state(true);
  let draftLowDiskGb = $state(10);
  let savingGuardian = $state(false);
  let guardianFeedback = $state<{ kind: "ok" | "err"; text: string } | null>(
    null,
  );

  const hasGuardianChanges = $derived(
    settings !== null &&
      (draftAutostart !== settings.autostart_enabled ||
        draftPauseBattery !== settings.pause_on_battery_fullscreen ||
        draftLowDiskGb !== settings.low_disk_shield_gb),
  );

  // Sync from sidebar quick-toggle only when the sidebar prop changes independently
  let lastSyncedProp = $state<boolean | undefined>(undefined);
  $effect(() => {
    if (autostartEnabled !== undefined && autostartEnabled !== lastSyncedProp) {
      lastSyncedProp = autostartEnabled;
      if (settings && settings.autostart_enabled !== autostartEnabled) {
        settings = { ...settings, autostart_enabled: autostartEnabled };
        draftAutostart = autostartEnabled;
      }
    }
  });

  async function loadRulesAsync() {
    loadingRules = true;
    rulesLoadError = null;
    try {
      rules = await listQuotaRules();
    } catch (err: unknown) {
      console.error("Failed to load quota rules:", err);
      rulesLoadError = err instanceof Error ? err.message : String(err);
    } finally {
      loadingRules = false;
    }
  }

  async function loadAll() {
    // Load fast settings & drives immediately without blocking on disk-walking listQuotaRules()
    const [cfgRes, bootRes] = await Promise.allSettled([
      getAppSettings(),
      getBootstrapState(),
    ]);

    if (bootRes.status === "fulfilled") {
      drives = bootRes.value.drives ?? [];
    }
    if (cfgRes.status === "fulfilled") {
      const cfg = cfgRes.value;
      settings = cfg;
      draftAutostart = cfg.autostart_enabled;
      draftPauseBattery = cfg.pause_on_battery_fullscreen;
      draftLowDiskGb = cfg.low_disk_shield_gb;
      lastSyncedProp = cfg.autostart_enabled;
      onAutostartChanged?.(cfg.autostart_enabled);
      newDataRoot =
        cfg.data_root ||
        (drives[0]?.root ? `${drives[0].root}GreenC` : "D:\\GreenC");
      i18n.init(cfg.language, cfg.translations, cfg.theme_mode);
    } else {
      statusBanner = {
        kind: "err",
        text:
          cfgRes.reason instanceof Error
            ? cfgRes.reason.message
            : String(cfgRes.reason),
      };
    }

    // Load quota rules usage asynchronously so it never blocks Settings controls
    void loadRulesAsync();
  }

  async function refreshDrives() {
    if (refreshingDrives) return;
    refreshingDrives = true;
    try {
      const boot = await getBootstrapState();
      drives = boot.drives ?? [];
    } catch (err: unknown) {
      console.error("Failed to refresh drives:", err);
    } finally {
      refreshingDrives = false;
    }
  }

  function dataRootForDrive(root: string): string {
    const sep = root.includes("\\") || /^[A-Za-z]:$/.test(root) ? "\\" : "/";
    const normalized =
      root.endsWith("\\") || root.endsWith("/") ? root : `${root}${sep}`;
    return `${normalized}GreenC`;
  }

  function formatDrivePrefix(d: DriveInfo): string {
    if (d.root.startsWith("/") || !/^[A-Za-z]$/.test(d.drive_letter ?? "")) {
      return d.root;
    }
    return `${d.drive_letter}:`;
  }

  onMount(() => {
    loadAll();
    const handleWindowFocus = () => {
      void refreshDrives();
    };
    window.addEventListener("focus", handleWindowFocus);
    return () => {
      window.removeEventListener("focus", handleWindowFocus);
    };
  });

  async function onLanguageChange(newLang: string) {
    try {
      const updated = await updateSettings({ language: newLang });
      settings = updated;
      i18n.setLocale(updated.language, updated.translations);
    } catch (err: unknown) {
      statusBanner = { kind: "err", text: String(err) };
    }
  }

  async function onThemeChange(mode: "system" | "light" | "dark") {
    try {
      i18n.setThemeMode(mode);
      const updated = await updateSettings({ theme_mode: mode });
      settings = updated;
    } catch (err: unknown) {
      statusBanner = { kind: "err", text: String(err) };
    }
  }

  async function handleSaveGuardianSettings() {
    if (savingGuardian) return;
    savingGuardian = true;
    guardianFeedback = null;
    try {
      const parsedGb = Math.round(Number(draftLowDiskGb));
      const clampedGb = Math.min(
        100,
        Math.max(2, Number.isFinite(parsedGb) ? parsedGb : 10),
      );
      const updated = await updateSettings({
        autostart_enabled: draftAutostart,
        pause_on_battery_fullscreen: draftPauseBattery,
        low_disk_shield_gb: clampedGb,
      });
      settings = updated;
      draftAutostart = updated.autostart_enabled;
      draftPauseBattery = updated.pause_on_battery_fullscreen;
      draftLowDiskGb = updated.low_disk_shield_gb;
      lastSyncedProp = updated.autostart_enabled;
      onAutostartChanged?.(updated.autostart_enabled);
      guardianFeedback = {
        kind: "ok",
        text: updated.autostart_enabled
          ? t("Đã lưu cài đặt — Khởi động cùng hệ điều hành đang bật.")
          : t("Đã lưu cài đặt."),
      };
    } catch (err: unknown) {
      guardianFeedback = {
        kind: "err",
        text: err instanceof Error ? err.message : String(err),
      };
      // Refresh live OS autostart status (e.g. DisabledByUser / DisabledByPolicy) on failure
      try {
        const refreshed = await getAppSettings();
        settings = refreshed;
        draftAutostart = refreshed.autostart_enabled;
      } catch {
        // Keep existing UI state if refresh also fails
      }
    } finally {
      savingGuardian = false;
    }
  }

  async function onRetentionChange(days: number) {
    try {
      const clamped = Math.min(90, Math.max(1, Number(days) || 7));
      const updated = await updateSettings({ retention_days_default: clamped });
      settings = updated;
    } catch (err: unknown) {
      statusBanner = { kind: "err", text: String(err) };
    }
  }

  async function handleCheckUpdates() {
    if (checkingUpdate) return;
    checkingUpdate = true;
    updateCheckError = null;
    try {
      updateResult = await checkForUpdates();
    } catch (err: unknown) {
      updateCheckError = err instanceof Error ? err.message : String(err);
    } finally {
      checkingUpdate = false;
    }
  }

  async function handleInstallUpdate() {
    if (
      !updateResult?.download_url ||
      !updateResult?.asset_name ||
      installingUpdate
    )
      return;
    installingUpdate = true;
    updateCheckError = null;
    try {
      await downloadAndInstallUpdate(
        updateResult.download_url,
        updateResult.asset_name,
      );
    } catch (err: unknown) {
      updateCheckError = err instanceof Error ? err.message : String(err);
    } finally {
      installingUpdate = false;
    }
  }

  async function handleMoveDataRoot() {
    if (!newDataRoot.trim() || isMovingVault) return;
    isMovingVault = true;
    statusBanner = null;
    try {
      const res = await changeDataRoot(newDataRoot.trim());
      if (res.success) {
        statusBanner = {
          kind: "ok",
          text: t(
            "Đã chuyển vị trí lưu dữ liệu sang {path} ({count} gói Vault)",
            {
              path: res.new_data_root,
              count: res.moved_archives_count,
            },
          ),
        };
        await loadAll();
      } else {
        statusBanner = { kind: "err", text: res.errors.join("; ") };
      }
    } catch (err: unknown) {
      statusBanner = {
        kind: "err",
        text: err instanceof Error ? err.message : String(err),
      };
    } finally {
      isMovingVault = false;
    }
  }

  async function handleToggleRuleEnabled(rule: QuotaRuleStatusDto) {
    await saveQuotaRule({
      id: rule.id,
      app_name: rule.app_name,
      process_name: rule.process_name,
      service_name: rule.service_name,
      target_paths: rule.target_paths,
      max_bytes: rule.max_bytes,
      watermark_ratio: rule.watermark_ratio,
      retention_days: rule.retention_days,
      min_file_age_seconds: rule.min_file_age_seconds,
      exclude_patterns: rule.exclude_patterns,
      enabled: !rule.enabled,
    });
    rules = await listQuotaRules();
  }

  async function handleDeleteRule(ruleId: number) {
    await deleteQuotaRule(ruleId);
    rules = await listQuotaRules();
  }

  async function triggerSandboxDemo(simulateCrash: boolean) {
    demoRunning = true;
    demoResult = null;
    try {
      demoResult = await runSandboxDemo(simulateCrash);
    } catch (err: unknown) {
      statusBanner = { kind: "err", text: String(err) };
    } finally {
      demoRunning = false;
    }
  }
</script>

<div class="max-w-5xl mx-auto space-y-6 pb-8">
  <div>
    <h1 class="text-2xl font-bold text-primary">{t("Cài đặt")}</h1>
    <p class="text-xs text-secondary mt-0.5">
      {t(
        "Ngôn ngữ tức thì, giao diện Sáng/Tối, vị trí Kho dự phòng và quy tắc hạn mức chạy ngầm",
      )}
    </p>
  </div>

  {#if statusBanner}
    <div
      class={`rounded-lg border px-4 py-3 text-xs flex items-center gap-2 ${
        statusBanner.kind === "ok"
          ? "border-accent/30 bg-accent/10 text-accent"
          : "border-danger/30 bg-danger/10 text-danger"
      }`}
    >
      {#if statusBanner.kind === "ok"}
        <CheckCircle2 class="h-4 w-4 shrink-0" />
      {:else}
        <AlertTriangle class="h-4 w-4 shrink-0" />
      {/if}
      <span>{statusBanner.text}</span>
    </div>
  {/if}

  <!-- Card 1: Instant 50-Language & Theme Selector -->
  <section
    class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-4"
  >
    <div class="grid grid-cols-1 md:grid-cols-2 gap-5">
      <!-- Language Selector (50 Locales, zero restart) -->
      <div class="space-y-1.5">
        <label
          for="lang-select"
          class="flex items-center gap-2 text-sm font-bold text-primary"
        >
          <Globe class="h-4 w-4 text-accent" />
          <span>{t("Ngôn ngữ hiển thị")}</span>
        </label>
        <p class="text-xs text-secondary">
          {t(
            "Áp dụng ngay lập tức cho 50 ngôn ngữ (hỗ trợ RTL tự động), không cần khởi động lại.",
          )}
        </p>
        <select
          id="lang-select"
          value={settings?.language ?? i18n.locale}
          onchange={(e) =>
            onLanguageChange((e.currentTarget as HTMLSelectElement).value)}
          class="w-full px-3 py-2 rounded-lg border border-border bg-subtle text-sm font-medium text-primary focus:border-accent outline-none"
        >
          {#if settings?.supported_languages?.length}
            {#each settings.supported_languages as lang}
              <option value={lang.code}>
                {lang.native_name} ({lang.english_name}){lang.is_rtl
                  ? " · RTL"
                  : ""}
              </option>
            {/each}
          {:else}
            <option value="vi">Tiếng Việt (Vietnamese)</option>
            <option value="en">English</option>
          {/if}
        </select>
      </div>

      <!-- Light / Dark / System Theme Mode -->
      <div class="space-y-1.5">
        <div class="flex items-center gap-2 text-sm font-bold text-primary">
          <SunMoon class="h-4 w-4 text-accent" />
          <span>{t("Giao diện")}</span>
        </div>
        <p class="text-xs text-secondary">
          {t("Đồng bộ theo Windows 11 hoặc cố định nền Sáng / Tối.")}
        </p>
        <div class="grid grid-cols-3 gap-2 pt-0.5">
          {#each [{ id: "system", label: t("Theo hệ thống") }, { id: "light", label: t("Sáng") }, { id: "dark", label: t("Tối") }] as const as opt}
            <button
              type="button"
              onclick={() => onThemeChange(opt.id)}
              class={`px-3 py-2 rounded-lg border text-xs font-semibold transition-colors ${
                (settings?.theme_mode ?? i18n.themeMode) === opt.id
                  ? "border-accent bg-accent/10 text-accent"
                  : "border-border bg-subtle text-secondary hover:text-primary"
              }`}
            >
              {opt.label}
            </button>
          {/each}
        </div>
      </div>
    </div>
  </section>

  <!-- Card 2: Data Root & Quarantine Retention -->
  <section
    class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-4"
  >
    <div class="flex items-center gap-2 text-sm font-bold text-primary">
      <HardDrive class="h-4 w-4 text-accent" />
      <span>{t("Vị trí lưu dữ liệu & Kho dự phòng (Canary Vault)")}</span>
    </div>

    <div class="flex flex-col sm:flex-row gap-2.5">
      <div class="flex items-center gap-1.5">
        <select
          onchange={(e) => {
            const target = e.currentTarget as HTMLSelectElement;
            const root = target.value;
            if (root) {
              newDataRoot = dataRootForDrive(root);
              target.value = "";
            }
          }}
          class="px-3 py-2 rounded-lg border border-border bg-subtle text-xs font-semibold text-primary max-w-[280px]"
        >
          <option value="">{t("Chọn nhanh ổ đĩa...")}</option>
          {#each drives.filter((d) => d.is_eligible) as d}
            <option value={d.root}>
              {formatDrivePrefix(d)}{d.label ? ` ${d.label}` : ""} ({formatSize(
                d.free_bytes,
              )}
              {t("trống")})
            </option>
          {/each}
          {#each drives.filter((d) => !d.is_eligible) as d}
            <option value="" disabled>
              {formatDrivePrefix(d)}{d.label ? ` ${d.label}` : ""} — {d.ineligible_reason ??
                d.filesystem}
            </option>
          {/each}
        </select>

        <button
          type="button"
          onclick={() => void refreshDrives()}
          disabled={refreshingDrives}
          title={t("Làm mới danh sách ổ đĩa")}
          aria-label={t("Làm mới danh sách ổ đĩa")}
          class="p-2 rounded-lg border border-border bg-subtle hover:bg-card text-secondary hover:text-primary disabled:opacity-50 transition-colors"
        >
          <RefreshCw
            class={`h-3.5 w-3.5 ${refreshingDrives ? "animate-spin text-accent" : ""}`}
          />
        </button>
      </div>

      <input
        type="text"
        dir="ltr"
        bind:value={newDataRoot}
        class="flex-1 px-3 py-2 rounded-lg border border-border bg-subtle text-xs font-mono text-primary ltr-isolate focus:border-accent outline-none"
      />

      <button
        type="button"
        onclick={handleMoveDataRoot}
        disabled={isMovingVault}
        class="px-4 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm disabled:opacity-50"
      >
        {isMovingVault ? t("Đang di chuyển...") : t("Đổi vị trí lưu")}
      </button>
    </div>

    <div
      class="flex items-center justify-between flex-wrap gap-3 pt-2 border-t border-border text-xs"
    >
      <div>
        <div class="font-semibold text-primary">
          {t("Thời gian giữ bản dự phòng mặc định")}
        </div>
        <div class="text-secondary">
          {t(
            "Sau thời hạn này, các gói không phát sinh lỗi sẽ tự động xóa vĩnh viễn.",
          )}
        </div>
      </div>
      <div class="flex items-center gap-2">
        <input
          type="number"
          dir="ltr"
          min="1"
          max="90"
          value={settings?.retention_days_default ?? 7}
          onchange={(e) =>
            onRetentionChange(
              Number((e.currentTarget as HTMLInputElement).value),
            )}
          class="w-20 px-2.5 py-1.5 rounded-lg border border-border bg-subtle text-center font-bold text-primary tabular-numbers"
        />
        <span class="text-secondary">{t("ngày")}</span>
      </div>
    </div>
  </section>

  <!-- Card 3: System Tray Guardian, Battery/Fullscreen & Low-Disk Shield -->
  <section
    class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-4"
  >
    <div class="flex items-center justify-between gap-3 flex-wrap">
      <div class="flex items-center gap-2 text-sm font-bold text-primary">
        <Shield class="h-4 w-4 text-accent" />
        <span>{t("Chạy ngầm thông minh & Lá chắn ổ C:")}</span>
      </div>
      {#if hasGuardianChanges}
        <span
          class="px-2 py-0.5 rounded-full bg-amber-500/15 text-amber-600 dark:text-amber-400 text-[11px] font-semibold"
        >
          {t("Có thay đổi chưa lưu")}
        </span>
      {/if}
    </div>

    <div class="space-y-3 text-xs">
      <label class="flex items-center justify-between gap-4 cursor-pointer">
        <div>
          <div class="font-semibold text-primary">
            {t("Khởi động cùng hệ điều hành")}
          </div>
          <div class="text-secondary">
            {t(
              "Chạy ngầm ở khay hệ thống (< 15 MB RAM) để tự bảo vệ ổ đĩa mỗi ngày.",
            )}
          </div>
          {#if settings?.autostart_status === "disabled_by_user"}
            <div class="text-danger mt-0.5">
              {t(
                "Đang bị tắt trong Windows Settings / Task Manager. Hãy bật lại trong Settings → Apps → Startup.",
              )}
            </div>
          {:else if settings?.autostart_status === "disabled_by_policy"}
            <div class="text-danger mt-0.5">
              {t("Bị chặn bởi chính sách quản trị hệ thống.")}
            </div>
          {:else if settings?.autostart_status === "enabled_by_policy"}
            <div class="text-secondary mt-0.5">
              {t("Đang được bật bắt buộc bởi chính sách quản trị hệ thống.")}
            </div>
          {/if}
        </div>
        <input
          type="checkbox"
          bind:checked={draftAutostart}
          disabled={settings?.autostart_status === "disabled_by_policy" ||
            settings?.autostart_status === "enabled_by_policy"}
          onchange={() => (guardianFeedback = null)}
          class="h-4 w-4 rounded border-border text-accent cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
        />
      </label>

      <label
        class="flex items-center justify-between gap-4 pt-2 border-t border-border/60 cursor-pointer"
      >
        <div>
          <div class="font-semibold text-primary">
            {t("Tạm hoãn nén khi dùng pin hoặc chơi game toàn màn hình")}
          </div>
          <div class="text-secondary">
            {t(
              "Tự động nhường tài nguyên trong dưới 100 ms khi bạn thao tác trực tiếp.",
            )}
          </div>
        </div>
        <input
          type="checkbox"
          bind:checked={draftPauseBattery}
          onchange={() => (guardianFeedback = null)}
          class="h-4 w-4 rounded border-border text-accent cursor-pointer"
        />
      </label>

      <div
        class="flex items-center justify-between gap-4 pt-2 border-t border-border/60"
      >
        <div>
          <div class="font-semibold text-primary">
            {t("Ngưỡng cảnh báo khẩn cấp ổ C: (Low-Disk Shield)")}
          </div>
          <div class="text-secondary">
            {t(
              "Thông báo ngay khi dung lượng trống ổ C: tụt xuống dưới ngưỡng này.",
            )}
          </div>
        </div>
        <div class="flex items-center gap-2">
          <input
            type="number"
            dir="ltr"
            min="2"
            max="100"
            bind:value={draftLowDiskGb}
            oninput={() => (guardianFeedback = null)}
            class="w-20 px-2.5 py-1.5 rounded-lg border border-border bg-subtle text-center font-bold text-primary tabular-numbers"
          />
          <span dir="ltr" class="text-secondary font-medium ltr-isolate"
            >GB</span
          >
        </div>
      </div>

      <div
        class="flex items-center justify-between gap-3 pt-3 border-t border-border flex-wrap"
      >
        <div class="min-w-0 flex-1">
          {#if guardianFeedback}
            <div
              class={`inline-flex items-center gap-1.5 text-xs font-medium ${
                guardianFeedback.kind === "ok" ? "text-accent" : "text-danger"
              }`}
            >
              {#if guardianFeedback.kind === "ok"}
                <CheckCircle2 class="h-4 w-4 shrink-0" />
              {:else}
                <AlertTriangle class="h-4 w-4 shrink-0" />
              {/if}
              <span>{guardianFeedback.text}</span>
            </div>
          {/if}
        </div>
        <button
          type="button"
          onclick={handleSaveGuardianSettings}
          disabled={savingGuardian || !settings}
          class={`px-4 py-2 rounded-lg text-xs font-semibold shadow-sm transition-colors disabled:opacity-50 ${
            hasGuardianChanges
              ? "bg-accent hover:bg-accent-hover text-white"
              : "border border-border bg-subtle hover:bg-card text-primary"
          }`}
        >
          {savingGuardian ? t("Đang lưu...") : t("Lưu cài đặt")}
        </button>
      </div>
    </div>
  </section>

  <!-- Card 3b: Per-drive quotas -->
  <DriveQuotaCard {drives} />

  <!-- Card 4: Background Rolling Quota Rules Table -->
  <section
    class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-4"
  >
    <div class="flex items-center justify-between">
      <div>
        <h2 class="text-sm font-bold text-primary">
          {t("Quy tắc hạn mức xoay vòng tự động")}
        </h2>
        <p class="text-xs text-secondary">
          {t(
            "Khi thư mục vượt trần, GreenC tự nén tệp cũ nhất vào Kho dự phòng để đưa dung lượng về mức 85%.",
          )}
        </p>
      </div>
      <button
        type="button"
        onclick={() => {
          editingRule = null;
          showRuleModal = true;
        }}
        class="inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm"
      >
        <Plus class="h-3.5 w-3.5" />
        {t("Thêm quy tắc")}
      </button>
    </div>

    {#if loadingRules && rules.length === 0}
      <div
        class="rounded-lg border border-border/60 bg-subtle/50 py-6 text-center text-xs text-secondary"
      >
        {t("Đang tải danh sách quy tắc...")}
      </div>
    {:else if rulesLoadError && rules.length === 0}
      <div
        class="rounded-lg border border-danger/30 bg-danger/10 px-4 py-3 text-xs text-danger"
      >
        {rulesLoadError}
      </div>
    {:else if rules.length === 0}
      <div
        class="rounded-lg border border-border/60 bg-subtle/50 py-6 text-center text-xs text-secondary"
      >
        {t(
          'Chưa có quy tắc hạn mức nào. Bạn có thể tạo tại đây hoặc bấm "Tạo quy tắc" trực tiếp từ biểu đồ biến động.',
        )}
      </div>
    {:else}
      <div class="space-y-2">
        {#each rules as rule (rule.id)}
          <div
            class="flex items-center justify-between gap-3 rounded-lg border border-border bg-subtle/40 px-3.5 py-2.5 text-xs"
          >
            <div class="min-w-0 flex-1 space-y-0.5">
              <div class="flex items-center gap-2">
                <span class="font-bold text-primary">{rule.app_name}</span>
                {#if rule.is_exceeded}
                  <span
                    class="px-1.5 py-0.5 rounded bg-danger/10 text-danger text-[10px] font-semibold"
                  >
                    {t("Vượt hạn mức")}
                  </span>
                {/if}
              </div>
              <div
                dir="ltr"
                class="font-mono text-[11px] text-secondary truncate ltr-isolate"
              >
                {rule.target_paths.join("; ")}
              </div>
              <div class="text-[11px] text-disabled tabular-numbers">
                {t("Hiện tại")}: {formatSize(rule.current_bytes)} / {formatSize(
                  rule.max_bytes,
                )}
                · {t("Thu về")}: {Math.round(rule.watermark_ratio * 100)}% · {t(
                  "Dự phòng",
                )}: {rule.retention_days}
                {t("ngày")}
              </div>
            </div>

            <div class="flex items-center gap-2 shrink-0">
              <input
                type="checkbox"
                checked={rule.enabled}
                onchange={() => handleToggleRuleEnabled(rule)}
                class="h-4 w-4 rounded border-border text-accent cursor-pointer"
                title={t("Bật / Tắt quy tắc")}
              />
              <button
                type="button"
                onclick={() => {
                  editingRule = {
                    id: rule.id,
                    app_name: rule.app_name,
                    process_name: rule.process_name,
                    service_name: rule.service_name,
                    target_paths: rule.target_paths,
                    max_bytes: rule.max_bytes,
                    watermark_ratio: rule.watermark_ratio,
                    retention_days: rule.retention_days,
                    exclude_patterns: rule.exclude_patterns,
                    enabled: rule.enabled,
                  };
                  showRuleModal = true;
                }}
                class="p-1.5 rounded border border-border bg-card text-secondary hover:text-primary"
                title={t("Chỉnh sửa")}
              >
                <Edit3 class="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                onclick={() => handleDeleteRule(rule.id)}
                class="p-1.5 rounded border border-border bg-card text-secondary hover:text-danger"
                title={t("Xóa quy tắc")}
              >
                <Trash2 class="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </section>

  <!-- Card 5: Isolated Sandbox Safety Demo -->
  <section
    class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-3"
  >
    <div class="flex items-center justify-between flex-wrap gap-2">
      <div class="flex items-center gap-2 text-sm font-bold text-primary">
        <FlaskConical class="h-4 w-4 text-accent" />
        <span
          >{t(
            "Kiểm chứng cơ chế Canary Vault & Tự động Rollback (Sandbox)",
          )}</span
        >
      </div>
      <div class="flex items-center gap-2">
        <button
          type="button"
          disabled={demoRunning}
          onclick={() => triggerSandboxDemo(false)}
          class="px-3 py-1.5 rounded-lg border border-border bg-subtle hover:bg-card text-xs font-medium text-primary disabled:opacity-50"
        >
          {t("Thử Canary an toàn")}
        </button>
        <button
          type="button"
          disabled={demoRunning}
          onclick={() => triggerSandboxDemo(true)}
          class="px-3 py-1.5 rounded-lg bg-accent/15 hover:bg-accent hover:text-white text-accent text-xs font-semibold transition-colors disabled:opacity-50"
        >
          {t("Mô phỏng lỗi & Tự khôi phục")}
        </button>
      </div>
    </div>
    {#if demoResult}
      <div
        class="rounded-lg border border-accent/30 bg-accent/10 p-3 text-xs text-primary"
      >
        {demoResult.detail}
      </div>
    {/if}
  </section>

  <!-- Card 6: About GreenC & Updates (Cùng Chat · cungchat.com) -->
  <section
    class="rounded-xl border border-border bg-card p-5 shadow-sm space-y-3"
  >
    <div class="flex items-center justify-between flex-wrap gap-4">
      <div class="flex items-center gap-3">
        <BrandLogo size={40} />
        <div>
          <div class="flex items-center gap-2">
            <span class="font-bold text-base text-primary">GreenC</span>
            <span
              dir="ltr"
              class="px-2 py-0.5 rounded bg-subtle border border-border text-[11px] text-secondary ltr-isolate"
            >
              v{settings?.app_version ?? "2.1.2"} · Rust Engine
            </span>
          </div>
          <div class="text-xs text-secondary mt-0.5 flex items-center gap-1.5">
            <Info class="h-3.5 w-3.5 text-accent" />
            <span>{t("Phát triển bởi")} <strong>Cùng Chat</strong></span>
            <span>·</span>
            <a
              href="https://cungchat.com"
              target="_blank"
              rel="noreferrer"
              dir="ltr"
              class="text-accent hover:underline font-medium ltr-isolate"
            >
              cungchat.com
            </a>
          </div>
        </div>
      </div>

      <div class="flex items-center gap-2">
        {#if settings?.is_store_build}
          <a
            href={settings.store_updates_url ??
              "ms-windows-store://downloadsandupdates"}
            target="_blank"
            rel="noreferrer"
            class="inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg border border-accent/35 bg-accent/10 hover:bg-accent/15 text-xs font-semibold text-accent transition-colors"
          >
            <RefreshCw class="h-3.5 w-3.5" />
            <span>{t("Cập nhật qua Microsoft Store")}</span>
          </a>
        {:else}
          <button
            type="button"
            disabled={checkingUpdate}
            onclick={handleCheckUpdates}
            class="inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg border border-border bg-subtle hover:bg-card text-xs font-semibold text-primary disabled:opacity-60 transition-colors"
          >
            <RefreshCw
              class={`h-3.5 w-3.5 text-accent ${checkingUpdate ? "animate-spin" : ""}`}
            />
            <span
              >{checkingUpdate
                ? t("Đang kiểm tra...")
                : t("Kiểm tra cập nhật")}</span
            >
          </button>

          {#if updateResult?.has_update}
            {#if updateResult.download_url && updateResult.asset_name}
              <button
                type="button"
                disabled={installingUpdate}
                onclick={handleInstallUpdate}
                class="inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm disabled:opacity-60"
              >
                <Download class="h-3.5 w-3.5" />
                <span
                  >{installingUpdate
                    ? t("Đang tải bản cập nhật...")
                    : `${t("Cập nhật ngay")} v${updateResult.latest_version}`}</span
                >
              </button>
            {:else}
              <a
                href={updateResult.release_url}
                target="_blank"
                rel="noreferrer"
                class="inline-flex items-center gap-1.5 px-3.5 py-2 rounded-lg bg-accent hover:bg-accent-hover text-white text-xs font-semibold shadow-sm"
              >
                <Download class="h-3.5 w-3.5" />
                <span>{t("Tải bản mới")}</span>
              </a>
            {/if}
          {/if}
        {/if}
      </div>
    </div>

    {#if updateCheckError}
      <div
        class="rounded-lg border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger"
      >
        {updateCheckError}
      </div>
    {:else if updateResult}
      <div
        class="rounded-lg border border-accent/25 bg-accent/5 px-3 py-2 text-xs text-primary flex items-center justify-between gap-2 flex-wrap"
      >
        {#if updateResult.has_update}
          <span>
            <strong
              >{t("Có phiên bản mới")} v{updateResult.latest_version}</strong
            >
            ({t("Hiện tại")}: v{updateResult.current_version})
          </span>
        {:else}
          <span class="flex items-center gap-1.5 text-accent font-medium">
            <CheckCircle2 class="h-3.5 w-3.5" />
            {t("Bạn đang dùng phiên bản mới nhất")} (v{updateResult.current_version})
          </span>
        {/if}
      </div>
    {/if}
  </section>
</div>

{#if showRuleModal}
  <QuotaRuleModal
    initialRule={editingRule}
    onClose={() => (showRuleModal = false)}
    onSaved={async () => {
      showRuleModal = false;
      rules = await listQuotaRules();
    }}
  />
{/if}
