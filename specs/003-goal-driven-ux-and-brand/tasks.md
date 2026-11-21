# Implementation Tasks: Goal-Driven UX, System Tray Controls & Drive Gauge + C Logo

- [x] **T001** Create `BrandLogo.svelte` (Drive Gauge + C), update `ui/index.html` favicon, and regenerate all native Tauri icons (`icon.ico`, `icon.icns`, `icon.png`, `32x32.png`, `128x128.png`, `128x128@2x.png`).
- [x] **T002** Update `c:\greenc-web\public\favicon.svg` and `c:\greenc-web\src\main.ts` with the matching Drive Gauge + C logo.
- [x] **T003** Add `hide_to_tray` Tauri command in `crates/greenc-app` and wire it in `ui/src/lib/api.ts`.
- [x] **T004** Fix `DriveQuotaCard.svelte` empty drive bug + redesign with visual drive cards, 1-slider simple goal mode, and collapsible advanced settings; decouple `SettingsPage.svelte` fetches.
- [x] **T005** Redesign `FirstRunModal.svelte` into 2-step Goal-First Onboarding (Step 1: Set Drive Goal, Step 2: Vault & Autostart) saving both first-run config and drive quota.
- [x] **T006** Add `GoalContextStrip.svelte` and integrate Goal-Driven Hero/Strips across `OverviewPage.svelte`, `CleanupPage.svelte`, `RelocatePage.svelte`, `CompactPage.svelte`, and `VaultPage.svelte`.
- [x] **T007** Add Header "Chạy ngầm" (Hide to System Tray) button and Sidebar quick "Khởi động cùng OS" toggle + new logo in `App.svelte`.
- [x] **T008** Add all 49 new i18n keys to all 50 locales in `crates/greenc-i18n/locales/` and verify with `greenc-i18n check` and `cargo test -p greenc-i18n`.
