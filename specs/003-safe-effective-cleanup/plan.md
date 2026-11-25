# Implementation Plan: Safe & Effective Cleanup (`GC-GUARD`)

**Branch**: `the build agent/004-gc-guard` | **Date**: 2026-11-25 | **Spec**: [spec.md](./spec.md)

---

## Summary

Hợp nhất cơ chế dọn dẹp theo hạn mức ổ đĩa giữa `greenc-cli` và `greenc-app` vào một lõi dùng chung trong `greenc-vault`, kết hợp phát hiện công cụ lập trình đang chạy trong `greenc-platform`, quét cache lập trình theo đơn vị nguyên khối, đo thực lợi trước/sau, cooldown tăng dần lưu SQLite (schema v4), chế độ `regenerable_cache_mode` (`vault` | `direct`) theo ổ, công cụ gợi ý output build `guard suggest`, và script cron Linux mỏng kèm tài liệu cài đặt.

---

## Architecture & Module Map

| Crate / Path | Changes |
|---|---|
| `crates/greenc-core` | Thêm `RegenerableCacheMode` (`Vault` mặc định, `Direct`), trường `regenerable_cache_mode` vào `DriveQuota`, mô hình `GuardCooldownState`, `DirectDeleteRecord`, bổ sung hằng số bảo vệ lock/metadata. |
| `crates/greenc-platform` | Thêm `dev_tools.rs`: `DevTool`, `ProcessSnapshot`, `BusyToolsStatus`, hàm thuần `detect_busy_dev_tools`, trait `DevToolProbe` + `NativeDevToolProbe` (Linux `/proc` cùng UID, macOS `ps`, Windows qua `NativeProcessServiceProbe`, khác -> `Unknown`) + `MockDevToolProbe`. Cải thiện `unix/handle.rs::is_file_locked` kiểm tra thêm `fcntl(F_GETLK)` và ghi rõ giới hạn trên Unix. |
| `crates/greenc-storage` | Migration v3 → v4: thêm cột `drive_quotas.regenerable_cache_mode` (`DEFAULT 'vault'`), bảng `guard_cooldowns`, bảng `direct_delete_log`; CRUD tương ứng trong `db.rs`. |
| `crates/greenc-vault` | 1) `cache_units.rs`: quét đơn vị nguyên khối cho Gradle (transform, bản Gradle cũ), Cargo (crate), npm (`index-v5` + refcount `content-v2`), pip/uv; tuổi `max(mtime, atime đáng tin)` ≥ 7 ngày; bộ lọc cấm tuyệt đối; `profiles.rs` cập nhật `package_managers` (7 ngày + dùng đơn vị nguyên khối) và `app_logs` Linux (chỉ `*.log`, `*.log.*`).<br>2) `guard.rs`: lõi `GuardEngine` dùng chung cho CLI và app — lọc đúng ổ, ưu tiên tái tạo + lâu không dùng, tính thực lợi dự kiến khi Vault cùng ổ, TOCTOU trước mỗi đơn vị (tuổi, busy tool, dev/ino, no-symlink), xoá thẳng an toàn chỉ cho đơn vị mục 2 khi `direct`, đo `free_before`/`free_after` → `net_freed_bytes`, `target_reached`, escalation cooldown `30m → 2h → 12h` vào DB, thông báo lý do tiếng Việt.<br>3) `suggest.rs`: `suggest_large_build_outputs` chỉ đọc, tìm `build`/`target`/`.gradle`/`node_modules` bị git bỏ qua trong repo/worktree cũ ≥ 3 ngày không có tool bận, giới hạn độ sâu/thời gian/cùng ổ/không symlink, kèm dung lượng Vault. |
| `crates/greenc-cli` | Thêm lệnh `guard run [--drive X] [--dry-run] [--extra-busy-tools ...] --json`, `guard suggest [--drive X] [--root PATH] --json`, và cờ `--regenerable-cache-mode` trên `quota set` + `guard set-mode --drive X --mode <vault|direct>`. Bảo đảm `clean --all` tuân thủ toàn bộ quy tắc cấm mục 2. |
| `crates/greenc-app` | Kết nối `guardian.rs` (`enforce_drive_quota` và vòng `BackgroundGuardian`) sử dụng lõi dùng chung `greenc-vault::guard`, giữ tương thích API/DTO `drive_quota.rs` và toàn bộ test hiện có. |
| `packaging/linux/` | Thêm `greenc-disk-guard.sh` và `README.md` (hướng dẫn cài đặt cron/systemd, không tự cài, không sửa file `packaging/` hiện có). |
| `docs/reports/gc-guard.md` | Báo cáo nghiệm thu cuối cùng (thiết kế, file:dòng, kết quả test thật, phân tích sự cố `@reboot` và `is_file_locked`, kiểm tra `cfg` Windows/macOS, rủi ro). |

---

## Technical Constraints & Verification

- **No new dependencies**: dùng toàn bộ crate sẵn có trong workspace (`rusqlite`, `chrono`, `serde`, `libc`, `clap`, `tempfile`, ...).
- **Safe test execution**: mọi lệnh build/test chạy tuần tự qua `cargo`, mọi test dùng `tempfile::tempdir()`, không chạm dữ liệu thật.
- **Quality gates**: `gc-cargo.sh fmt --check`, `gc-cargo.sh clippy -p <crate> --all-targets -- -D warnings`, `gc-cargo.sh test` cho mọi crate thay đổi và `greenc-app`, kiểm tra `bash -n` + kịch bản giả lập cho `greenc-disk-guard.sh`, chạy `ocr review` nếu khả dụng.
