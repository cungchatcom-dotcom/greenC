# Implementation Plan: GreenC v2 — Viết Lại Toàn Bộ Bằng Rust

**Branch**: `001-greenc-complete-platform` | **Date**: 2026-11-16 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-greenc-complete-platform/spec.md`

---

## Summary

Viết lại toàn bộ ứng dụng **GreenC** bằng **Rust** đặt tại `C:\DiskGuard\greenc-v2` theo kiến trúc **Cargo Multi-Crate Workspace + Tauri v2 (Svelte 5 Fluent UI + WebView2 Unload-on-Tray)**. Bản Rust giữ nguyên 100% tính năng và độ tương thích dữ liệu (`greenc.db`, gói `.zip` trong Kho dự phòng, liên kết thư mục, khóa đơn phiên bản `GreenCAppMutex`, bộ cài Inno Setup `AppId`) của bản Python v2.0 hiện hành, đồng thời nâng cấp toàn diện về chiều sâu và hiệu năng hệ thống:
1. **Quét ổ đĩa siêu tốc đa tầng (`greenc-scanner`)**: kết hợp `FindFirstFileExW` đa luồng `rayon` (không cần Admin) và chế độ tăng tốc đọc bảng `USN/MFT` trực tiếp, phục vụ **Bản đồ không gian trực quan (Space Lens)** và **Dự báo số ngày đầy ổ C:**.
2. **Kho dự phòng tốc độ cao & Khôi phục lẻ từng tệp (`greenc-vault`)**: đóng gói song song `ZIP64` (`STORED` / `DEFLATE` / `ZSTD`) tốc độ `> 250 MB/s`, bảo toàn nhóm `NTFS HardLink`, hỗ trợ duyệt tìm và khôi phục riêng lẻ từng tệp, kèm **Safety Watchdog** tự khôi phục nguyên tử khi phát hiện lỗi ứng dụng/dịch vụ.
3. **Di chuyển dữ liệu 25+ hồ sơ & Tự phục hồi sau sự cố (`greenc-relocate`)**: Known Folders (`SHSetKnownFolderPath`), cấu hình bộ đệm Dev/AI (`npm`, `pip`, `uv`, `cargo`, `pnpm`, `yarn`, `nuget`, `gradle`, `ollama`, `huggingface`), hướng dẫn từng bước (`Zalo`, `WSL2`, `Docker Desktop`), và liên kết thư mục trực tiếp qua `DeviceIoControl(FSCTL_SET_REPARSE_POINT)` kèm kiểm tra sức khỏe liên kết định kỳ.
4. **Thu gọn tại chỗ không đổi đường dẫn (`greenc-compact`)**: nén lạnh trong suốt cấp hệ thống tệp (`WOF XPRESS8K/XPRESS16K` / `LZNT1`) trên ổ `C:` và thu gọn khoảng trống rỗng đĩa ảo `.vhdx` (`virtdisk.dll`), kèm **Radar phát hiện dữ liệu mồ côi** sau gỡ cài đặt.
5. **Siêu nhẹ & Đa ngôn ngữ tức thì (`greenc-i18n`, `greenc-cli`, `greenc-app`)**: bộ cài `< 10 MB`, chạy ngầm khay hệ thống `< 15 MB RAM` (nhờ giải phóng WebView2 khi đóng cửa sổ chính), 50 ngôn ngữ + 4 ngôn ngữ RTL đổi trực tiếp `< 100 ms` không cần khởi động lại, chế độ Sáng/Tối/Hệ thống và giao diện dòng lệnh `greenc-cli`.

---

## Technical Context

**Language/Version**: Rust `1.83+` (Edition 2021 / 2024-ready, target `x86_64-pc-windows-msvc` & `aarch64-pc-windows-msvc`), TypeScript `5.7+` + Svelte `5.x` (chỉ cho tầng hiển thị giao diện).

**Primary Dependencies**:
- **Win32 & Hệ thống**: `windows` (`0.58+`: `Storage::FileSystem`, `System::RestartManager`, `System::Services`, `UI::Shell`, `Storage::Vhd`, `System::Threading`), `sysinfo`, `notify` (`ReadDirectoryChangesW`).
- **Xử lý song song & Bất đồng bộ**: `rayon` (quét đĩa & nén đa nhân), `tokio` (điều phối tác vụ nền, chu trình 45 giây & kênh sự kiện), `crossbeam-channel`, `tokio-util` (`CancellationToken`).
- **Lưu trữ & Đóng gói**: `rusqlite` (`bundled` SQLite 3.46+ WAL), `zip` (`zip64`, `deflate-zlib-ng`, `zstd`), `zstd`, `sha2` (tăng tốc phần cứng SHA-NI), `blake3`, `serde`, `serde_json`.
- **Desktop Shell & CLI**: `tauri` (`2.2+`), `tauri-plugin-autostart`, `tauri-plugin-notification`, `clap` (`4.5+` cho `greenc-cli`), `tracing` + `tracing-appender`.

**Storage**: SQLite (`%LOCALAPPDATA%\GreenC\greenc.db`, chế độ `WAL`, tự động nâng cấp lược đồ từ bản Python v2.0 và tự kế thừa từ `%LOCALAPPDATA%\Green C\greenc.db` / `C:\DiskGuard\data\diskguard.db`) + Kho dự phòng `ZIP64` kèm `.manifest.json` tại `<Ổ_dữ_liệu>:\GreenC\Vault\`.

**Testing**: `cargo test --workspace` (unit test & integration test trên thư mục tạm `tempfile::TempDir` với trait giả lập Win32/Shell/Runner), `proptest` (kiểm thử bất biến khử trùng lặp HardLink & khôi phục xung đột tên tệp), `vitest` + `@testing-library/svelte` (kiểm thử giao diện & RTL), bộ kiểm thử hợp đồng 50 ngôn ngữ (`greenc-i18n check`).

**Target Platform**: Windows 10 (1903+) và Windows 11 (64-bit), cài đặt theo người dùng hiện tại (`%LOCALAPPDATA%\Programs\GreenC`, `PrivilegesRequired=lowest`).

**Project Type**: Native Windows Desktop Application + System Tray Guardian + Headless CLI (`workspace` đa crate).

**Performance Goals**:
- Quét 500.000 tệp trên SSD NVMe: `< 3 giây` (chế độ chuẩn không cần Admin) và `< 0,6 giây` (chế độ tăng tốc USN/MFT).
- Đóng gói Kho dự phòng (12.000 tệp, 1,5 GB): `>= 250 MB/s` trên NVMe (nhanh gấp `8–12 lần` bản Python).
- Đổi ngôn ngữ / chế độ Sáng-Tối trực tiếp: `< 100 ms` không cần khởi động lại; giao diện duy trì `60 FPS`.
- Dừng tác vụ hợp tác (`Stop`): `< 150 ms` và dọn sạch 100% tệp tạm.

**Constraints**:
- RAM chạy nền ở khay hệ thống khi đóng cửa sổ chính: `< 15 MB` (mục tiêu trần Hiến pháp `< 25 MB`).
- Kích thước bộ cài độc lập `GreenC-Setup-<version>.exe`: `< 10 MB` (mục tiêu trần `< 15 MB`).
- Luôn bảo toàn tối thiểu `512 MiB` dung lượng trống dự phòng trên ổ Kho dự phòng và ổ đích di chuyển.
- Tuyệt đối không yêu cầu quyền Admin cho cài đặt và vận hành hằng ngày; không gửi bất kỳ dữ liệu nào ra ngoài mạng.

**Scale/Scope**: Hỗ trợ ổ đĩa từ `128 GB` đến `16 TB` (`> 2 triệu` tệp), gói dự phòng đơn lẻ vượt `> 4 GiB` (`ZIP64`), `25+` hồ sơ ứng dụng/công cụ chuyên sâu, `6` màn hình giao diện chính và `50` ngôn ngữ.

---

## Constitution Check

*GATE: Đánh giá trước Nghiên cứu Phase 0 và kiểm tra lại sau Thiết kế Phase 1.*

| Nguyên tắc trong `constitution.md` | Trạng thái trước Phase 0 | Giải pháp Kiến trúc sau Phase 1 | Kết quả Gate |
|---|---|---|---|
| **I. Zero-Data-Loss & Reversible Operations** | ✅ PASS | Kiểm tra khóa `CreateFileW(DELETE, ShareMode=0)` bằng RAII trước & sau khi nén; đối chiếu `SHA-256` trực tuyến; `RelocationRecord(moving)` ghi trước `rename` kèm `recover_interrupted()` khi khởi động. | ✅ PASS |
| **II. Official-Mechanism-First & NTFS Reparse Safety** | ✅ PASS | Thứ tự cứng trong `greenc-relocate`: Known Folders → Tool Config → Guided → Verified Junction (`FSCTL_SET_REPARSE_POINT`); `greenc-scanner` chặn mọi điểm reparse lồng nhau (`0x400`) và khử trùng lặp 128-bit `FileId`. | ✅ PASS |
| **III. Crate-First Modularity, Trait Boundaries & Test-First** | ✅ PASS | Tách biệt hoàn toàn 8 crate miền nghiệp vụ khỏi tầng giao diện; đóng gói OS qua các trait `Win32Fs`, `ShellFolderApi`, `ProcessProbe`, `ToolRunner`; cung cấp `greenc-cli` xuất `--json`. | ✅ PASS |
| **IV. Sub-Second Responsiveness & Low Footprint** | ✅ PASS | Tác vụ chạy trên `tokio::spawn_blocking` + `rayon`, báo cáo tiến độ tiết lưu `100 ms`, hủy hợp tác qua `CancellationToken`; giải phóng cửa sổ WebView2 khi ẩn xuống khay hệ thống để giữ RAM `~8–12 MB`. | ✅ PASS |
| **V. Per-User Least Privilege & 50-Locale Accessibility** | ✅ PASS | Chạy hoàn toàn ở quyền người dùng chuẩn (`PrivilegesRequired=lowest`); nén lạnh `WOF` hoạt động không cần Admin trên tệp người dùng; tái sử dụng và kiểm chứng tự động 50 tệp ngôn ngữ + RTL CSS Logical Properties. | ✅ PASS |

*Không có vi phạm Hiến pháp nào cần ngoại lệ.*

---

## Project Structure

### Documentation (this feature)

```text
C:\DiskGuard\greenc-v2\specs\001-greenc-complete-platform\
├── spec.md              # Đặc tả toàn diện tính năng hiện tại + chiều sâu mở rộng
├── plan.md              # Kế hoạch kiến trúc & lộ trình viết lại bằng Rust (tệp này)
├── research.md          # Quyết định công nghệ & đối chiếu phương án (Phase 0)
├── data-model.md        # Lược đồ SQLite WAL v2 & kiểu miền Rust (Phase 1)
├── quickstart.md        # Hướng dẫn build, kiểm thử & kịch bản nghiệm thu (Phase 1)
├── checklists/
│   └── requirements.md  # Danh sách kiểm tra chất lượng đặc tả (16/16 PASS)
├── contracts/
│   ├── ipc-commands.md  # Hợp đồng lệnh & kênh sự kiện Tauri v2 IPC
│   └── cli-contract.md  # Hợp đồng giao diện dòng lệnh greenc-cli
└── tasks.md             # Danh sách tác vụ triển khai chi tiết theo từng User Story
```

### Source Code (`C:\DiskGuard\greenc-v2`)

```text
C:\DiskGuard\greenc-v2\
├── Cargo.toml                       # Cargo Workspace gốc, quản lý phiên bản & lints tập trung
├── rust-toolchain.toml              # Khóa phiên bản Rust stable & components (rustfmt, clippy)
├── README.md                        # Tài liệu phát triển & kiến trúc GreenC v2
├── CHANGELOG.md                     # Nhật ký phiên bản theo Keep a Changelog
├── crates\
│   ├── greenc-core\                 # Kiểu dữ liệu miền, cấu hình, TaskController, lỗi chuẩn
│   │   └── src\
│   │       ├── config.rs            # APP_NAME="GreenC", đường dẫn, hằng số bảo vệ mặc định
│   │       ├── models.rs            # DriveInfo, DailySnapshot, QuotaRule, VaultArchive, RelocationRecord...
│   │       ├── task.rs              # TaskControl (Pause / Resume / CancellationToken / Progress)
│   │       └── error.rs             # GreenCError phân loại chi tiết bằng thiserror
│   ├── greenc-win32\                # Tầng bọc an toàn RAII cho Windows API & Trait giả lập kiểm thử
│   │   └── src\
│   │       ├── handle.rs            # OwnedHandle, is_file_locked (DELETE + ShareMode=0)
│   │       ├── restart_mgr.rs       # Restart Manager (RmGetList) truy vết PID khóa tệp
│   │       ├── volume.rs            # GetLogicalDrives, GetVolumeInformationW, kiểm tra NTFS/Fixed
│   │       ├── reparse.rs           # FSCTL_SET_REPARSE_POINT / GET_REPARSE_POINT (Junction & Cloud tag)
│   │       ├── known_folders.rs     # SHGetKnownFolderPath / SHSetKnownFolderPath + kiểm tra OneDrive
│   │       ├── wof_compact.rs       # WofSetFileDataLocation (XPRESS8K/16K), FSCTL_SET_COMPRESSION, VHDX
│   │       ├── power_session.rs     # Kiểm tra Pin yếu, Fullscreen app & Registry Autostart
│   │       └── mutex.rs             # CreateMutexW("GreenCAppMutex") đơn phiên bản
│   ├── greenc-storage\              # Tầng lưu trữ SQLite WAL & Di chuyển dữ liệu từ bản cũ
│   │   └── src\
│   │       ├── db.rs                # GreenCDb, kết nối WAL, CRUD cho 8 bảng
│   │       ├── migrations.rs        # Tự nâng cấp lược đồ v1 (Python) -> v2 (Rust) không mất dữ liệu
│   │       ├── settings.rs          # AppSettings bọc bảng app_settings
│   │       └── legacy_migration.rs  # Di chuyển DB & Vault từ DiskGuard / "Green C" / đổi ổ lưu dữ liệu
│   ├── greenc-i18n\                 # Bộ dịch thuật 50 ngôn ngữ, kiểm tra biến {…}, định dạng số/RTL
│   │   ├── locales\                 # 49 tệp <code_ngôn_ngữ>.json + messages.json kế thừa & mở rộng
│   │   └── src\
│   │       ├── catalog.rs           # Nạp & chuyển đổi ngôn ngữ trực tiếp < 1 ms trong bộ nhớ
│   │       ├── format.rs            # format_size, format_eta, decimal_point, is_rtl
│   │       └── validator.rs         # Công cụ kiểm tra thiếu khóa / lệch biến cho CI
│   ├── greenc-scanner\              # Quét ổ đĩa siêu tốc, Truy vết biến động & Hồ sơ ứng dụng
│   │   └── src\
│   │       ├── walker.rs            # Duyệt đa luồng rayon + khử trùng lặp 128-bit NTFS FileId
│   │       ├── usn_mft.rs           # Chế độ tăng tốc tùy chọn đọc trực tiếp USN / MFT
│   │       ├── space_lens.rs        # Cây phân bổ dung lượng cho biểu đồ Space Treemap
│   │       ├── attribution.rs       # Phân loại 11 mã nguyên nhân + ánh xạ Tiến trình / Dịch vụ
│   │       ├── tracker.rs           # DailySnapshot, sự kiện ReadDirectoryChangesW & Dự báo đầy ổ
│   │       ├── profiles.rs          # Hồ sơ Zalo, DingTalk, Trình duyệt, Dev/AI caches
│   │       └── orphans.rs           # Radar phát hiện thư mục mồ côi đối chiếu Uninstall Registry
│   ├── greenc-vault\                # Kho dự phòng song song ZIP64, Canary Probe & Safety Watchdog
│   │   └── src\
│   │       ├── sniff.rs             # Nhận diện định dạng nén sẵn & đo thử độ nén 64 KiB đầu
│   │       ├── archiver.rs          # Pipeline đọc/băm SHA-256/nén song song (STORED/DEFLATE/ZSTD)
│   │       ├── canary.rs            # SmartAppAnalyzer + kiểm chứng mẫu 1 chu trình trên hộp cát
│   │       ├── enforcer.rs          # QuotaEnforcer (watermark 85%, dự phòng 512 MiB, nhóm HardLink)
│   │       ├── watchdog.rs          # Giám sát dịch vụ/nhật ký lỗi, Tự động Rollback, Khôi phục lẻ 1 tệp
│   │       └── simulator.rs         # Hộp cát mô phỏng thực tế (Sandbox Demo)
│   ├── greenc-relocate\             # Di chuyển dữ liệu 4 chiến lược + Thu gọn tại chỗ
│   │   └── src\
│   │       ├── strategy.rs          # Trait RelocationStrategy, CheckResult, RelocationCandidate
│   │       ├── verified_copy.rs     # Sao chép luồng đối chiếu SHA-256 & phục hồi khi hủy
│   │       ├── known_folder.rs      # Chiến lược thư mục chuẩn Windows (Downloads, Documents…)
│   │       ├── tool_config.rs       # npm, pip, uv, cargo, pnpm, yarn, nuget, gradle, ollama, hf
│   │       ├── guided.rs            # Zalo, WSL2, Docker Desktop, Steam
│   │       ├── junction.rs          # Sao chép -> .greenc-moving -> Junction -> Xác minh -> Hoàn tác
│   │       ├── recovery.rs          # Tự phục hồi sau sập nguồn & kiểm tra sức khỏe liên kết
│   │       ├── catalog.rs           # Bộ khám phá ứng viên > 200 MB trên ổ hệ thống
│   │       └── compact.rs           # Thu gọn tại chỗ (WOF XPRESS) & thu gọn đĩa ảo .vhdx
│   ├── greenc-cli\                  # Giao diện dòng lệnh độc lập (status, scan, clean, vault, relocate…)
│   │   └── src\main.rs
│   └── greenc-app\                  # Ứng dụng Desktop Tauri v2, Khay hệ thống & Chu trình chạy ngầm
│       ├── Cargo.toml
│       ├── build.rs
│       ├── tauri.conf.json          # Cấu hình cửa sổ 1280x820, icon, quyền IPC tối thiểu
│       └── src\
│           ├── main.rs              # Khóa đơn phiên bản, di chuyển DB cũ, khởi tạo Tauri & Tray
│           ├── state.rs             # AppState chia sẻ an toàn luồng & quản lý tác vụ nền
│           ├── guardian.rs          # Chu trình ngầm 45 giây (nhường người dùng, nhận biết Pin/Fullscreen)
│           ├── tray.rs              # Khay hệ thống & cơ chế giải phóng WebView2 khi đóng cửa sổ
│           └── commands\            # Các hàm #[tauri::command] theo hợp đồng ipc-commands.md
├── ui\                              # Giao diện Svelte 5 + TypeScript chuẩn Windows 11 Fluent
│   ├── package.json
│   ├── vite.config.ts
│   └── src\
│       ├── lib\
│       │   ├── ipc.ts               # Client bọc kiểu tĩnh cho lệnh & kênh sự kiện Tauri
│       │   ├── i18n.svelte.ts       # Store ngôn ngữ phản ứng tức thì + tự đảo dir="rtl"
│       │   ├── theme.css            # Hệ thống biến màu Fluent Light/Dark & chữ số thẳng cột
│       │   └── components\          # Sidebar, ExpandableTableCard, PathCell, StatusBar, SpaceTreemap, DailyBarChart
│       └── pages\
│           ├── OverviewPage.svelte  # Dung lượng, Dự báo đầy đĩa, Space Lens, Đề xuất giải phóng, Biểu đồ ngày
│           ├── CleanupPage.svelte   # Phân tích thông minh, Dữ liệu mồ côi, Canary badge & Dọn 1 chạm
│           ├── RelocatePage.svelte  # 4 nhóm chiến lược chuyển ổ, Mục nâng cao, Sức khỏe liên kết & Hoàn tác
│           ├── CompactPage.svelte   # Nén lạnh tại chỗ (WOF) & Thu gọn đĩa ảo .vhdx
│           ├── VaultPage.svelte     # Danh sách gói dự phòng, Tìm kiếm & Khôi phục lẻ từng tệp, Sandbox Demo
│           └── SettingsPage.svelte  # Ngôn ngữ tức thì, Giao diện, Ổ dữ liệu, Thời gian giữ, Quy tắc ngầm, Giới thiệu
├── packaging\
│   └── greenc.iss                   # Bộ cài Inno Setup 6 giữ nguyên AppId={{6F1C2B7A-3E4D-4C5B-9A8E-7D2F1B0C9E51}
└── scripts\
    ├── build.ps1                    # Kiểm thử Rust + UI -> cargo tauri build -> ISCC -> GreenC-Setup-<ver>.exe + .sha256
    └── check-i18n.ps1               # Kiểm tra đồng bộ 50 tệp ngôn ngữ
```

**Structure Decision**: Chọn kiến trúc **Multi-Crate Rust Workspace kết hợp thư mục `ui/` mỏng** tại `C:\DiskGuard\greenc-v2`. Việc tách 7 crate lõi (`greenc-core`, `greenc-win32`, `greenc-storage`, `greenc-i18n`, `greenc-scanner`, `greenc-vault`, `greenc-relocate`) giúp biên dịch gia tăng cực nhanh trên Windows, cho phép kiểm thử toàn bộ thuật toán hệ thống bằng `cargo test` và `greenc-cli` trong vài giây mà không cần bật giao diện đồ họa, đồng thời cô lập hoàn toàn mã `unsafe` Win32 vào duy nhất một crate `greenc-win32`.

---

## Lộ Trình Chuyển Đổi & Đảm Bảo Tương Thích Ngược (Migration Roadmap)

1. **Giai đoạn Nền tảng (`greenc-core`, `greenc-win32`, `greenc-storage`, `greenc-i18n`)**:
   - Đọc trực tiếp tệp `greenc.db` hiện có của bản Python v2.0, tự nâng cấp lược đồ lên `v2` bằng `ALTER TABLE` không phá hủy và sao chép danh mục 50 ngôn ngữ sang `crates/greenc-i18n/locales/`.
2. **Giai đoạn Động cơ Lõi (`greenc-scanner`, `greenc-vault`, `greenc-relocate`, `greenc-cli`)**:
   - Đảm bảo `greenc-vault` giải nén và hoàn tác được cả gói `.zip` cũ do Python tạo ra lẫn gói `ZIP64 + ZSTD` mới do Rust tạo ra.
   - Đảm bảo `greenc-relocate` đọc đúng các bản ghi `relocations` cũ và hỗ trợ hoàn tác liền mạch các liên kết thư mục đã tạo từ phiên bản Python.
3. **Giai đoạn Giao diện & Đóng gói (`greenc-app`, `ui`, `packaging/greenc.iss`)**:
   - Giữ nguyên `AppId` và `AppMutex=GreenCAppMutex` trong Inno Setup; thêm chỉ thị `[InstallDelete]` dọn sạch thư mục `_internal` (của PyInstaller cũ) khi người dùng cài đè bản Rust v2 lên bản Python v2.0, giúp dung lượng thư mục cài đặt giảm ngay từ `~240 MB` xuống `< 15 MB`!
