# Feature Specification: Goal-Driven UX, System Tray Controls & Drive Gauge + C Brand Logo

**Feature Branch**: `main` (Spec `003-goal-driven-ux-and-brand`)
**Status**: Approved for Implementation

## 1. Goals & Requirements

- **FR-001 (Fix Drive Selector Bug in Drive Quota)**: Fix empty drive selector in `DriveQuotaCard.svelte` by decoupling `SettingsPage.svelte` data fetches, adding an automatic `getBootstrapState()` fallback in `DriveQuotaCard.svelte`, and replacing the bare `<select>` with visual drive selection cards.
- **FR-002 (Simplified Goal-First Quota Configuration & Onboarding)**:
  - Simplify drive quota setup to a 1-slider target ("Giữ ổ luôn xanh dưới X%"), automatically computing sensible defaults (`warn = clean - 5%`, `target = clean - 10%`, auto-clean to Canary Vault enabled). Keep advanced controls collapsible.
  - Surface goal setup directly in `FirstRunModal.svelte` (2-step onboarding: Step 1 = Set Drive Goal, Step 2 = Canary Vault Storage & Autostart/Background Protection). Completing onboarding persists both `complete_first_run` and `save_drive_quota`.
- **FR-003 (Goal-Driven UX Across All Screens)**:
  - **OverviewPage**: Add a top **Goal Hero Card** showing active drive goal, current green/over-target status, progress bar with target threshold marker, gap to target (GB), and primary CTA ("Đưa về mức xanh ngay").
  - **CleanupPage / RelocatePage / CompactPage**: Add a compact **Goal Context Strip** showing remaining GB needed to meet the active goal and live projected post-action drive usage.
  - **VaultPage**: Show how much reclaimed space is currently safely held in the Canary Vault contributing to the green goal.
- **FR-004 (System Tray / Background Mode & OS Autostart Controls)**:
  - Add Tauri IPC command `hide_to_tray` in `crates/greenc-app` to unload the WebView window into background tray mode (`< 15 MB` RAM) while keeping `BackgroundGuardian` active.
  - Add a visible **"Chạy ngầm"** (System Tray) button in the top header and a quick **"Khởi động cùng hệ điều hành"** toggle in the Sidebar footer (OS-agnostic wording for Windows/macOS/Linux), plus Onboarding and Settings.
- **FR-005 (Drive Gauge + C Brand Logo Across App & Web)**:
  - Design a crisp SVG **Drive Gauge + C** logo and apply it uniformly to:
    - App UI (`BrandLogo.svelte`, Sidebar, Onboarding, `ui/index.html` favicon)
    - Native OS app icons (`icon.ico`, `icon.icns`, `icon.png`, `32x32.png`, `128x128.png`, `128x128@2x.png`)
    - Landing page (`c:\greenc-web\public\favicon.svg` and navbar/footer brand mark).
