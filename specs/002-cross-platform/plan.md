# Kế hoạch đa nền tảng: Linux, macOS, Android, iOS

Trạng thái: **bản nháp để duyệt** — chưa có mã nào được viết cho kế hoạch này.

## 1. Hiện trạng (đo từ mã nguồn)

| Crate | Số dòng | Phụ thuộc Windows | Ghi chú |
|---|---|---|---|
| greenc-core | ~850 | không | Mô hình, lỗi, TaskControl — dùng lại 100% |
| greenc-i18n | ~480 | không | 50 ngôn ngữ — dùng lại 100% |
| greenc-storage | ~1850 | không | SQLite bundled — dùng lại 100% (đổi thư mục dữ liệu) |
| greenc-scanner | ~2150 | một phần (USN/MFT, đường dẫn) | Walker/forecast/treemap dùng lại; USN/MFT chỉ Windows |
| greenc-vault | ~2400 | một phần (khóa tệp, thư mục hồ sơ) | ZIP64/zstd/rollback dùng lại; hồ sơ dọn dẹp cần bản theo OS |
| greenc-relocate | ~2100 | nhiều (NTFS junction, WOF, VHDX) | Cần viết lại theo OS (symlink/bind mount/APFS clone) |
| greenc-win32 | ~2700 | toàn bộ | Là tầng trừu tượng bằng trait → chỗ cắm backend mới |
| greenc-app + ui | ~4250 + Svelte | tray, autostart, thông báo | Tauri v2 + Svelte chạy được desktop khác; UI dùng lại |
| greenc-cli | ~1140 | ít | Chạy được trên Linux/macOS sau khi có backend |

Điểm thuận lợi: mọi lời gọi hệ điều hành đều đi qua các trait trong `greenc-win32/traits.rs`
(`Win32VolumeApi`, `FileLockProbe`, `ProcessServiceProbe`, `ReparsePointApi`, `PowerSessionApi`).
Đây là đường nối tự nhiên để thêm backend cho từng OS.

## 2. Nguyên tắc

1. **Tách theo nền tảng, không phải viết lại.** Đổi tên `greenc-win32` thành `greenc-platform` với ba backend
   `windows`, `unix` (Linux + macOS dùng chung phần lớn), và `mobile` (stub an toàn).
2. **Dùng `cfg(target_os)` + trait**, mỗi backend nằm trong module riêng; CI biên dịch cả ba.
3. **Không hứa tính năng mà hệ điều hành không cho phép.** Điện thoại là hộp cát (sandbox): app không đọc được
   dữ liệu của app khác, nên bản di động có phạm vi khác.
4. Mọi thao tác phá hủy vẫn đi qua Canary + Vault như bản Windows.

## 3. Phân tầng tính năng theo nền tảng

| Tính năng | Windows | Linux | macOS | Android | iOS |
|---|---|---|---|---|---|
| Quét ổ, treemap, dự báo đầy | có | có | có | chỉ bộ nhớ của app + MediaStore | chỉ dung lượng của app |
| Quota theo ổ + cảnh báo | có | có | có | có (dung lượng thiết bị) | có (dung lượng thiết bị) |
| Dọn dẹp vào Vault (có Canary) | có | có | có | chỉ cache/tệp của chính app, media do người dùng chọn | chỉ cache của chính app |
| Rollback Vault | có | có | có | có | có |
| Di dời thư mục (relocate) | junction | symlink / bind mount | symlink | không | không |
| Nén lạnh tại chỗ | WOF/LZNT1 | btrfs/zfs/ext4 (tùy FS) | APFS (clone/transparent) | không | không |
| Guardian nền 45 giây | có | systemd user service | launchd agent | WorkManager (tối thiểu 15 phút) | BGTaskScheduler (không đảm bảo thời điểm) |
| Khay hệ thống / thông báo | có | có (libappindicator) | có (menu bar) | thông báo | thông báo |

Điện thoại **không** làm: dọn dữ liệu app khác, di dời thư mục, nén tại chỗ. Các hạn chế này do hệ điều hành,
không phải do mã.

## 4. Lộ trình

### Giai đoạn A — Tách tầng nền tảng (nền móng, ~1–2 tuần)
- Đổi `greenc-win32` → `greenc-platform`; giữ nguyên trait, chuyển code Windows vào `platform/windows/`.
- Loại bỏ đường dẫn cứng (`C:\`, `%LOCALAPPDATA%`) khỏi `greenc-core/config.rs`, `guardian.rs`, hồ sơ dọn dẹp;
  dùng crate `directories` + trait `PathProvider`.
- Thêm CI ma trận (windows, ubuntu, macos) chạy `cargo check` + test thuần logic.
- **Tiêu chí xong:** toàn bộ workspace biên dịch trên 3 OS desktop; bản Windows không đổi hành vi (570 test E2E
  chạy bằng binary thật vẫn đạt).

### Giai đoạn B — Linux (~2 tuần)
- Backend `unix`: `statvfs` (dung lượng), kiểm tra khóa bằng `fcntl`/`/proc/*/fd`, thông tin tiến trình qua `/proc`,
  trạng thái pin qua `/sys/class/power_supply`, symlink cho relocate.
- Hồ sơ dọn dẹp: `~/.cache`, thùng rác XDG, `journalctl --vacuum`, apt/dnf/pacman cache (cần sudo → chỉ gợi ý lệnh),
  Docker/Flatpak/Snap.
- Đóng gói: AppImage + `.deb`; dịch vụ systemd user cho guardian; tray qua libappindicator.
- **Rủi ro:** Wayland/tray khác nhau theo môi trường desktop → fallback chỉ thông báo.

### Giai đoạn C — macOS (~2 tuần, cần máy Mac để build và ký)
- Dùng lại backend `unix`; thêm: `NSFileManager` volume info, APFS clone, purgeable space.
- Hồ sơ: `~/Library/Caches`, Xcode DerivedData, Homebrew, Time Machine local snapshots (cần quyền).
- Quyền: Full Disk Access (hướng dẫn người dùng cấp), notarization + Developer ID (bắt buộc để phát hành).
- Đóng gói `.dmg`, launchd agent cho guardian.
- **Rủi ro:** TCC (quyền riêng tư) chặn quét nhiều thư mục nếu chưa được cấp.

### Giai đoạn D — Android (~4–6 tuần)
- Tauri v2 hỗ trợ Android (`tauri android init`); UI Svelte dùng lại, cần responsive cho màn hình nhỏ.
- Lõi Rust (core/storage/scanner/vault) biên dịch cho `aarch64-linux-android`; thư mục dữ liệu = nội bộ app.
- Plugin Kotlin: `StorageStatsManager` (dung lượng), `MediaStore`/SAF (người dùng chọn thư mục để quét/dọn),
  WorkManager (guardian), thông báo.
- Phạm vi dọn: cache của chính app, tệp trong thư mục người dùng cấp quyền SAF, ảnh/video trùng lặp do người dùng
  duyệt. **Không** có quyền xóa cache app khác (Google Play cấm).
- **Rủi ro:** chính sách Google Play với quyền `MANAGE_EXTERNAL_STORAGE` — nên tránh, dùng SAF.

### Giai đoạn E — iOS (~4–6 tuần, cần máy Mac + tài khoản Apple Developer)
- Tauri v2 hỗ trợ iOS (`tauri ios init`), cần Xcode.
- Phạm vi rất hẹp: hiển thị dung lượng thiết bị (`volumeAvailableCapacityForImportantUsage`), dọn cache của chính app,
  quota + cảnh báo, và **chế độ đồng hành**: điện thoại hiển thị trạng thái ổ đĩa của máy tính qua đồng bộ (xem mục 5).
- App Store review: tránh mô tả "dọn dẹp iPhone" gây hiểu nhầm; định vị là "theo dõi dung lượng".
- **Rủi ro cao nhất:** iOS không cho tác vụ nền định kỳ; cảnh báo chủ yếu đến từ máy tính qua push.

### Giai đoạn F — Đồng bộ liên thiết bị (tùy chọn, sau D/E)
- Đồng bộ quota + trạng thái ổ đĩa giữa các thiết bị để điện thoại nhận cảnh báo từ PC.
- Cần chọn: kết nối trực tiếp trong LAN (không cần máy chủ) hay dịch vụ đám mây (cần máy chủ, chính sách quyền riêng tư).

## 5. Quyết định cần bạn chốt

1. **Phạm vi di động:** chỉ "theo dõi + dọn dữ liệu của chính app" (khả thi, đề xuất) hay bạn kỳ vọng dọn sâu như trên PC
   (không khả thi trên cả Android và iOS)?
2. **Đồng bộ PC ↔ điện thoại:** có làm không, và theo mô hình LAN hay đám mây?
3. **Thứ tự ưu tiên:** đề xuất A → B (Linux) → C (macOS) → D (Android) → E (iOS).
4. **Tài nguyên:** macOS và iOS bắt buộc có máy Mac và tài khoản Apple Developer (99 USD/năm); Google Play 25 USD một lần.
5. **Phát hành:** ký mã Windows (chưa có), notarization macOS, kênh phân phối (GitHub Releases, App Store, Play Store).

## 6. Kiểm thử

- Giữ bộ E2E hiện tại nhưng **buộc chạy với binary thật** (xóa chế độ rơi về `MockCliEngine` khi thiếu binary;
  hiện `tests/e2e/harness.py` tự chuyển sang mô phỏng nếu không tìm thấy `greenc-cli.exe`).
- Mỗi OS chạy suite trên runner CI riêng; di động dùng thiết bị thật hoặc emulator (ARTEMIS cho Android).
- Thử nghiệm thao tác phá hủy trong thư mục tạm / máy ảo, không bao giờ trên dữ liệu thật.
