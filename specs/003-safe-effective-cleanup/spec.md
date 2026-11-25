# Feature Specification: GreenC — Dọn Đúng Thứ, Đúng Mức, Không Phá Build (`GC-GUARD`)

**Feature Branch**: `the build agent/004-gc-guard`

**Created**: 2026-11-25

**Status**: Approved for Implementation

**Input**: Khắc phục sự cố thật ngày 2026-11-24 trên Ubuntu (`/home` 70 GB, ngưỡng warn 70 / clean 80 / target 75) khi cron chạy `greenc-cli clean --all`: dọn vào Vault cùng ổ không tạo thực lợi đáng kể, xé lẻ cache Gradle/npm khi đang build gây lỗi `Could not read workspace metadata` / `NoSuchFileException`, không nhận diện được ~12 GB output build trong git worktree, và `@reboot` không ghi nhật ký.

---

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Phát Hiện Công Cụ Lập Trình Đang Chạy & Bảo Vệ Cache Nguyên Khối (Priority: P1)

Khi nhà phát triển đang build dự án (Gradle/Kotlin, Cargo/Rustc, npm/pnpm/yarn, pip/uv — kể cả khi `GradleDaemon` đang rảnh), GreenC không bao giờ xoá hay xé lẻ các tệp cache bên trong thư mục cache của công cụ đó. Khi công cụ rảnh, GreenC chỉ chọn theo **đơn vị nguyên khối** (một thư mục transform, một phiên bản Gradle cũ, một crate Cargo, một mục cache npm qua `index-v5`, một mục pip/uv) đã không dùng ≥ 7 ngày theo `max(mtime, atime đáng tin)`, tuyệt đối không chạm các tệp điều khiển/khoá (`*.lock`, `*.lck`, `journal-*`, `gc.properties`, `CACHEDIR.TAG`, `fileHashes`, `modules-2/metadata-*`, `file-access.bin`, phiên bản cache mới nhất đang dùng ở cấp điều khiển). Trên Linux, profile `app_logs` chỉ chọn `*.log` và `*.log.*` cũ thay vì quét toàn bộ `~/.local/state`.

**Acceptance Scenarios**:
1. **Given** một cây `~/.gradle/caches` giả chứa `8.11.1/transforms/old_hash` (10 ngày), `8.11.1/fileHashes`, `journal-1`, `gc.properties`, `CACHEDIR.TAG`, `modules-2/metadata-2.106`, `file-access.bin`, `*.lock`, **When** `GradleDaemon` (kể cả rảnh) đang chạy, **Then** GreenC không chọn bất kỳ đơn vị Gradle nào.
2. **Given** cùng cây Gradle giả nhưng không có tiến trình Gradle nào chạy, **When** quét ứng viên, **Then** chỉ các đơn vị nguyên khối đủ ≥ 7 ngày (như `transforms/old_hash` hoặc bản Gradle cũ `7.6`) được chọn; các tệp khoá/journal/metadata/fileHashes/CACHEDIR.TAG/gc.properties/file-access.bin và cấp điều khiển bản mới nhất không bao giờ bị chọn.
3. **Given** công cụ bắt đầu chạy giữa chừng trong lúc đang dọn từng đơn vị, **When** bước kiểm tra TOCTOU trước khi xoá đơn vị kế tiếp chạy, **Then** GreenC dừng ngay lập tức và giữ nguyên các đơn vị còn lại.
4. **Given** môi trường không xác định được danh sách tiến trình (`Unknown`), **When** dọn tự động chạy, **Then** bỏ qua toàn bộ profile developer.

---

### User Story 2 - Lõi Dọn Theo Quota Dùng Chung, Đo Thực Lợi, Cooldown Bền Vững & Chế Độ Vault Cùng Ổ (Priority: P1)

`greenc-cli guard run [--drive X] [--dry-run] --json` và `BackgroundGuardian` của `greenc-app` dùng chung một lõi dọn theo quota. Mỗi ổ chỉ dọn ứng viên trên đúng ổ đó, ưu tiên cache tái tạo được và lâu không dùng nhất, dừng ngay khi đủ về `target_ratio`. Hệ thống đo dung lượng trống trước/sau để báo `logical_bytes`, `vault_bytes`, `net_freed_bytes` và `target_reached`. Nếu Vault nằm cùng ổ, lượng cần dọn được tính theo thực lợi dự kiến. Nếu người dùng bật `regenerable_cache_mode = direct` trên một ổ, chỉ các đơn vị cache lập trình tái tạo được (mục 2) được xoá thẳng kèm nhật ký DB đầy đủ; mọi dữ liệu không tái tạo được luôn đi qua Vault. Khi thực lợi < 5% nhu cầu hoặc không đạt `target_ratio`, hệ thống ghi cảnh báo tiếng Việt rõ nguyên nhân và tăng cooldown bền vững trong DB (`30 phút → 2 giờ → 12 giờ`).

**Acceptance Scenarios**:
1. **Given** ổ `/home` vượt `clean_ratio` (80%) với `target_ratio` (75%), **When** chạy `guard run --drive /home --json`, **Then** chỉ chọn đơn vị thuộc ổ `/home`, dừng khi đạt `target_ratio`, không chạm ổ khác.
2. **Given** Vault nằm cùng ổ với ổ cần dọn và chế độ mặc định `vault`, **When** dọn xong, **Then** báo chính xác `logical_bytes`, `vault_bytes`, `net_freed_bytes` và `target_reached`.
3. **Given** chế độ `regenerable_cache_mode = direct` bật trên ổ đó, **When** có cả đơn vị cache lập trình và tệp dữ liệu thường (`chat_media`/`app_logs`), **Then** đơn vị cache lập trình xoá thẳng có ghi nhật ký DB, còn dữ liệu không tái tạo được vẫn bắt buộc qua Vault.
4. **Given** lần dọn không đạt `target_ratio` hoặc `net_freed_bytes < 5%` nhu cầu, **When** chạy lại sau 15 phút, **Then** lần 1 đặt cooldown 30 phút, lần 2 tăng 2 giờ, lần 3 tăng 12 giờ (lưu SQLite), các lượt giữa chừng bị bỏ qua ngay kèm lý do tiếng Việt.

---

### User Story 3 - Gợi Ý Output Build Lớn (`guard suggest`) & Script Cron Linux Tin Cậy (Priority: P2)

`greenc-cli guard suggest --json` chỉ báo cáo (tuyệt đối không xoá) các thư mục output build tái tạo được bị git bỏ qua (`build`, `target`, `.gradle`, `node_modules`) trong repo/worktree không đổi ≥ 3 ngày và không có công cụ bận, kèm dung lượng từng mục và dung lượng Vault. Script `packaging/linux/greenc-disk-guard.sh` mỏng gọi `guard run`, có `flock`, xoay nhật ký ≤ 1 MB, ghi dòng khởi động ở `@reboot`, giữ phòng thủ hotfix kiểm tra tiến trình ở shell, trả mã lỗi đúng và có tài liệu hướng dẫn cài đặt.

**Acceptance Scenarios**:
1. **Given** một git repo/worktree giả có `app/build` bị `.gitignore` bỏ qua, cũ 5 ngày, không có Gradle chạy, **When** chạy `guard suggest --json`, **Then** báo cáo đúng đường dẫn và dung lượng mà không xoá bất kỳ tệp nào; không theo symlink, không vượt ra ngoài ổ/độ sâu/thời gian quét.
2. **Given** chạy `packaging/linux/greenc-disk-guard.sh --reboot`, **When** script khởi chạy, **Then** luôn ghi dòng khởi động/kiểm tra vào log, xoay log khi > 1 MB, giữ lock `flock`, không lặp trong cooldown và trả đúng mã thoát của `greenc-cli`.

---

## Requirements & Safety Invariants

- **FR-001**: Hàm thuần `detect_busy_dev_tools(&[ProcessSnapshot]) -> BTreeSet<DevTool>` trong `greenc-platform`, kiểm thử được không cần `/proc`; trên Linux đọc `/proc` chỉ của UID hiện tại; `GradleDaemon` rảnh vẫn tính bận.
- **FR-002**: Đơn vị cache nguyên khối mặc định tuổi ≥ 7 ngày (`max(mtime, atime đáng tin)`); danh sách cấm tuyệt đối áp dụng cho cả `guard run` lẫn `clean --all`.
- **FR-003**: TOCTOU trước khi xoá từng đơn vị: kiểm lại tuổi, tiến trình bận, không symlink, cùng ổ và định danh `(dev, ino)`; dừng ngay nếu công cụ bắt đầu chạy.
- **FR-004**: Schema SQLite v4 thêm `drive_quotas.regenerable_cache_mode` (mặc định `'vault'`), bảng `guard_cooldowns`, bảng `direct_delete_log`.
- **FR-005**: Không thay đổi hành vi mặc định trên Windows/macOS; không đổi phiên bản/tag/winget manifest hiện có; không thêm dependency ngoài nếu không cần thiết.
