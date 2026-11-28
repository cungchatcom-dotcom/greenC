# GreenC v2.4.0 — Goal-Driven Tiered Cleanup, USB Backup & Unused-App Uninstall Suggestions

**Date:** 2026-11-28
**Status:** Approved & In Implementation
**Target Repo:** `/home/duong/projects/greenc`

---

## 1. Goal & Philosophy

Instead of blindly sweeping everything in one rigid pass, GreenC calculates a **goal budget** (`bytes_needed = max(0, used_bytes - target_limit_bytes)`) from the user's drive goal (default `< 80%`, target ratio from `drive_quotas` or `75%` when over `80%`) and works strictly from **least impactful → most impactful**, **stopping immediately once the goal is reached**.

When drive `C:` is **already green** (`bytes_needed == 0`) and the user clicks **"Làm xanh ổ C: ngay"**, GreenC asks cleanly with 2 choices:
1. **Dọn nhẹ (Chỉ rác thuần túy — Tầng 1):** Keeps the drive tidy without touching apps or personal folders.
2. **Dọn sâu tối đa:** Runs all tiers (`max_space` mode) to maximize free space.

---

## 2. The 5-Tier Goal-Driven Ladder

| Tier | Name | Scope & Ordering | Stop Condition |
|---|---|---|---|
| **Tier 0** | **Analyze Goal** | Compute `bytes_needed`, resolve fixed secondary drive (`D:`/`E:`), detect any attached removable USB/external drives. | — |
| **Tier 1** | **Pure Junk (Least Important)** | `windows_temp`, `crash_dumps`, `app_logs`, `old_versions` (excluding `Windows.old` on direct-delete), `orphan_radar` (uninstalled app leftovers > 30 days). | Stop if `!max_space && freed >= bytes_needed`. |
| **Tier 2** | **Shallow High-Confidence App Caches (Least-Recently-Used First)** | Group app caches (`browser_cache`, `chat_media`, `package_managers`, Zalo safe cache/old versions) by app, compute each app's `last_used_epoch` (max file `mtime` in its data/cache dir), sort **ascending (least-recently-used first)**, clean only 100%-confident regenerable caches app-by-app. | Stop after any app once `!max_space && freed >= bytes_needed`. |
| **Tier 3** | **Deep Per-App Pass (Least-Recently-Used First)** | Revisit apps in **LRU order**: WOF `XPRESS8K` compaction on non-overlapping cold directories + relocate unlocked curated app data (`!is_advanced`) and non-OneDrive Known Folders to secondary fixed NTFS drive (`D:`). | Stop after any item once `!max_space && freed >= bytes_needed`. |
| **Tier 4** | **USB / External Drive Backup Prompt** *(only if still short of goal)* | Detect attached removable/external drives (USB / external HDD/SSD with free space >= 1 GiB, NTFS or exFAT). Surface in report: either pick an attached USB drive or prompt user to plug one in + **"Kiểm tra lại USB"**. User selects heavy folders (Downloads, Videos, Pictures, or LRU app data) and clicks **"Sao lưu sang USB & giải phóng ổ C:"** (`backup_to_external_drive`), which creates a verified ZIP+manifest backup on `<USB>\GreenC-Backup\...` (registered in Vault DB for 1-click Restore) and frees `C:` ONLY after verified flush. **NEVER creates NTFS junctions to removable USB drives.** | User-initiated when Tier 1–3 wasn't enough. |
| **Tier 5** | **Unused-App Uninstall Suggestions** *(only if still short of goal)* | Scan installed desktop apps (Windows Uninstall Registry `HKLM/HKCU\...\Uninstall` + `WOW6432Node`; Linux `dpkg-query` / desktop entries), estimate size & `last_used_days` (from install/AppData `mtime`), filter out OS components & GreenC itself, rank **least-recently-used & largest first**. Show top suggestions in report with 1-click **"Gỡ cài đặt"** (`launch_app_uninstaller`) that opens the app's official uninstaller (never silent-uninstalls). | User-initiated when still short of goal. |

---

## 3. UI Bug Fixes Included

1. **`SavingsScorecard.svelte`**: Replace percentage height inside un-sized flex container with explicit pixel bar heights (`barPx = Math.round((bytes / maxBucketBytes) * 132)`) inside a fixed `h-36` chart frame with subtle dashed guide lines and value labels above non-zero bars.
2. **`SettingsPage.svelte`**: Bind `<select value={selectedQuickDrive}>` derived from `newDataRoot` and remove `target.value = ""` so selecting a drive displays the selected drive cleanly.
