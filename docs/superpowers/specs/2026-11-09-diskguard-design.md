# Đặc Tả Thiết Kế (Design Specification): DiskGuard — Ứng Dụng Theo Dõi Biến Động Ổ Cứng, Truy Vết Tác Nhân & Quản Lý Quota Xoay Vòng An Toàn

- **Ngày lập:** 2026-11-09
- **Nền tảng mục tiêu:** Windows 10 / 11 Desktop (Python 3.12 + PySide6 + SQLite + Windows API / ETW / Watchdog / psutil)
- **Thư mục dự án:** `C:\DiskGuard`
- **Chế độ vận hành:** Ứng dụng Desktop PySide6 tích hợp chạy nền qua System Tray (Khay hệ thống), hỗ trợ tự khởi động cùng Windows và quyền Administrator.

---

## 1. Mục tiêu & Phạm vi Hệ thống

**DiskGuard** giải quyết trọn vẹn 5 bài toán cốt lõi trong quản lý dung lượng ổ cứng trên Windows:
1. **Theo dõi mức độ sử dụng ổ đĩa theo từng ngày bằng biểu đồ cột tương tác:** Hiển thị tổng dung lượng đã dùng và mức biến động ròng (`+GB / -GB`) của từng ngày; cho phép bấm vào cột của bất kỳ ngày nào để xem phân tích chi tiết của ngày đó.
2. **Truy vết nguyên nhân làm đầy ổ cứng (Ở đâu? Vì sao? Ngày nào? Do ứng dụng hay Windows Service nào?):** Ghi nhận các đợt tăng dung lượng theo thư mục/file, đối chiếu với tiến trình (`.exe`) và Windows Service đang ghi đĩa, đồng thời tự động phân loại nguyên nhân (Log phình to, Cache/Temp, Crash Dump, Database/WAL, Container/VM, Media/Download).
3. **Thiết lập hạn mức dung lượng tối đa (Soft Rolling Quota) cho từng ứng dụng/service:** Ngăn ứng dụng ghi dữ liệu phình to vượt mức cho phép trên ổ đĩa mà vẫn giữ tổng dung lượng thực tế xoay quanh ngưỡng cấu hình.
4. **Tự động thu hồi (Recycle) các file cũ nhất không gây ảnh hưởng đến việc chạy của ứng dụng:** Khi vượt ngưỡng Quota, hệ thống chỉ chọn các file cũ nhất hiện **không bị khóa (unlocked)** bởi bất kỳ tiến trình nào và không nằm trong danh sách file cấu hình/thực thi thiết yếu.
5. **Chu trình Nén Zip tạm (Safe Vault) $\rightarrow$ Giám sát an toàn $D$ ngày $\rightarrow$ Tự động Xóa hoặc Tự động Rollback ngay lập tức:**
   - Nén các file cũ thành gói `.zip` kèm `manifest.json` (lưu đường dẫn gốc, `SHA-256`, dấu thời gian `mtime`, thông tin App/Service, thời hạn $D$ ngày).
   - Trong $D$ ngày: nếu phát hiện dấu hiệu "không an toàn" (ứng dụng/service cố đọc lại đường dẫn file đã nén gây lỗi thiếu file, hoặc service dừng lỗi bất thường sau khi thu hồi), hệ thống **tự động Rollback giải nén trả lại ngay lập tức** về đúng vị trí cũ và đưa file đó vào danh sách miễn trừ vĩnh viễn.
   - Nếu sau $D$ ngày mọi thứ vận hành ổn định, hệ thống **tự động xóa vĩnh viễn** gói `.zip`. Người dùng cũng có thể bấm **Rollback 1-Click** hoặc **Xóa ngay** bất kỳ lúc nào.

---

## 2. Kiến trúc Mô-đun & Cấu trúc Thư mục

```text
C:\DiskGuard\
├── pyproject.toml
├── README.md
├── main.py                          # Điểm khởi chạy ứng dụng (GUI + System Tray + Background Workers)
├── diskguard\
│   ├── __init__.py
│   ├── config.py                    # Cấu hình đường dẫn DB, Safe Vault, chu kỳ quét, ngưỡng mặc định
│   ├── models.py                    # Dataclasses cho Snapshot, WriteEvent, QuotaRule, VaultArchive, FileManifestItem
│   ├── storage\
│   │   ├── __init__.py
│   │   └── database.py              # Tầng lưu trữ SQLite (Thread-safe, WAL mode)
│   ├── core\
│   │   ├── __init__.py
│   │   ├── disk_tracker.py          # Chụp dung lượng ổ đĩa hàng ngày & tổng hợp biến động theo ngày
│   │   ├── attribution.py           # Truy vết Tiến trình / Windows Service & Phân loại nguyên nhân ghi đĩa
│   │   ├── win_locks.py             # Kiểm tra khóa file cấp Windows API (CreateFileW ShareMode=0 + Restart Manager)
│   │   ├── quota_enforcer.py        # Kiểm tra ngưỡng Soft Rolling Quota, chọn file cũ an toàn & nén vào Safe Vault
│   │   ├── safety_watchdog.py       # Giám sát an toàn trong D ngày, phát hiện lỗi thiếu file/service crash, Tự Rollback & Tự Xóa
│   │   └── simulator.py             # Bộ giả lập Sandbox thực tế để kiểm chứng nhanh toàn bộ luồng trên UI
│   └── ui\
│       ├── __init__.py
│       ├── main_window.py           # Cửa sổ chính PySide6 (Thanh chọn ổ đĩa, 3 Tab, System Tray, Thông báo)
│       ├── chart_widget.py          # Biểu đồ cột tương tác theo ngày (QPainter custom bar chart hỗ trợ click chọn ngày)
│       ├── daily_tab.py             # Tab 1: Biểu đồ ngày + Bảng chi tiết Thư mục/Nguyên nhân & Bảng Ứng dụng/Service
│       ├── quota_tab.py             # Tab 2: Quản lý Rule Quota cho từng ứng dụng/service
│       ├── vault_tab.py             # Tab 3: Kho Nén Tạm (Safe Vault), bộ đếm D ngày, Rollback 1-Click & lịch sử tự Rollback
│       └── dialogs.py               # Hộp thoại tạo/sửa Rule Quota & xem chi tiết các file bên trong gói .zip
└── tests\
    ├── __init__.py
    ├── test_database_and_daily_delta.py
    ├── test_attribution_engine.py
    ├── test_win_locks_and_zipper.py
    └── test_safety_rollback_and_purge.py
```

---

## 3. Thiết kế Cơ sở Dữ liệu SQLite (`diskguard.db`)

Cơ sở dữ liệu sử dụng chế độ `PRAGMA journal_mode=WAL;` để hỗ trợ đọc/ghi đồng thời giữa các luồng nền và giao diện PySide6:

1. **`daily_snapshots` (Ảnh chụp dung lượng ổ đĩa theo ngày):**
   - `id` (INTEGER PK), `drive_letter` (TEXT, ví dụ `C:\`), `snapshot_date` (TEXT `YYYY-MM-DD`, UNIQUE cùng `drive_letter`), `total_bytes` (INTEGER), `used_bytes` (INTEGER), `free_bytes` (INTEGER), `net_change_bytes` (INTEGER — chênh lệch so với ngày liền trước), `updated_at` (TEXT ISO8601).
2. **`write_events` (Sổ cái truy vết biến động thư mục/file & tác nhân ghi đĩa):**
   - `id` (INTEGER PK), `event_date` (TEXT `YYYY-MM-DD`), `timestamp` (TEXT ISO8601), `drive_letter` (TEXT), `folder_path` (TEXT), `file_path` (TEXT), `delta_bytes` (INTEGER), `process_name` (TEXT), `pid` (INTEGER), `service_name` (TEXT), `service_display_name` (TEXT), `reason_code` (TEXT), `reason_detail` (TEXT).
3. **`quota_rules` (Cấu hình giới hạn dung lượng xoay vòng cho ứng dụng/service):**
   - `id` (INTEGER PK), `app_name` (TEXT), `process_name` (TEXT), `service_name` (TEXT), `target_paths_json` (TEXT — danh sách thư mục của ứng dụng), `max_bytes` (INTEGER), `watermark_ratio` (REAL, mặc định `0.85`), `retention_days` (INTEGER — số ngày $D$ giữ bản zip trước khi xóa, mặc định `7`), `min_file_age_seconds` (INTEGER, mặc định `60`), `exclude_patterns_json` (TEXT), `protected_files_json` (TEXT — các đường dẫn đã từng phải Rollback để miễn trừ vĩnh viễn), `enabled` (INTEGER `0/1`), `created_at` (TEXT).
4. **`vault_archives` (Quản lý các gói `.zip` trong Safe Vault):**
   - `id` (INTEGER PK), `rule_id` (INTEGER FK), `app_name` (TEXT), `zip_path` (TEXT), `manifest_path` (TEXT), `file_count` (INTEGER), `original_bytes` (INTEGER), `compressed_bytes` (INTEGER), `created_at` (TEXT ISO8601), `expires_at` (TEXT ISO8601), `status` (TEXT: `QUARANTINED`, `ROLLED_BACK`, `PURGED`), `status_reason` (TEXT), `resolved_at` (TEXT).
5. **`vault_files` (Chi tiết từng file nằm trong gói `.zip` để phục vụ tra cứu nhanh & phát hiện truy cập file thiếu):**
   - `id` (INTEGER PK), `archive_id` (INTEGER FK), `original_path` (TEXT INDEXED), `rel_arcname` (TEXT), `size_bytes` (INTEGER), `sha256` (TEXT), `mtime` (REAL), `atime` (REAL).

---

## 4. Chi tiết Thiết kế 4 Phân hệ Lõi

### 4.1. Disk Usage & Daily Delta Tracker (`core/disk_tracker.py`)
- Khi khởi động và định kỳ mỗi 60 giây (cũng như khi chuyển sang ngày mới), cập nhật bản ghi `daily_snapshots` của ổ đĩa đang theo dõi.
- Tính `net_change_bytes = used_bytes(hôm nay) - used_bytes(hôm qua)` (hoặc tổng `delta_bytes` ghi nhận trong ngày nếu là ngày đầu tiên cài đặt).
- Hỗ trợ truy vấn dữ liệu chuỗi thời gian $N$ ngày gần nhất (7 / 14 / 30 ngày) để hiển thị trên biểu đồ cột và truy vấn drill-down chi tiết khi người dùng bấm vào một cột ngày cụ thể.

### 4.2. Process & Service Attribution Engine (`core/attribution.py`)
- **Theo dõi hệ thống file (`watchdog` / `ReadDirectoryChangesW`):** Lắng nghe sự kiện tạo mới (`on_created`) và ghi/sửa đổi (`on_modified`) trên các thư mục trọng điểm hoặc toàn ổ đĩa (loại trừ chính thư mục `Safe Vault` và file `diskguard.db` để tránh vòng lặp).
- **Theo dõi tốc độ ghi tiến trình & ánh xạ Windows Service (`ProcessIOMonitor`):**
  - Duy trì bảng bộ nhớ đệm `PID -> (process_name, exe_path, write_bytes_last, timestamp)` qua `psutil.process_iter(['pid', 'name', 'exe', 'io_counters'])`.
  - Duy trì bảng ánh xạ `PID -> (service_name, display_name)` và `folder_prefix -> (app_name, process_name, service_name)` qua `psutil.win_service_iter()` cùng danh mục chữ ký đường dẫn phổ biến trên Windows (`AppData\Local`, `AppData\Roaming`, `ProgramData`, `Windows\Temp`, `Windows\Logs`, `WER`, Docker/WSL, SQL, IIS, trình duyệt...).
- **Thuật toán gán tác nhân (Hybrid Attribution Resolution):**
  1. Kiểm tra tiến trình nào đang mở trực tiếp file đó (qua Windows Restart Manager `RmGetList` trong `win_locks.py`).
  2. Nếu file đã đóng ngay sau khi ghi xong, chọn tiến trình có lượng `write_bytes` tăng khớp với thời điểm sự kiện và/hoặc khớp chữ ký đường dẫn thư mục ứng dụng/service.
- **Bộ phân loại nguyên nhân (`classify_write_reason`):**
  - Phân tích phần mở rộng file, tên thư mục cha và mẫu tên file để gắn nhãn:
    - `LOG_GROWTH`: `.log`, `.etl`, `.evtx`, `.trace`, hoặc nằm trong thư mục `logs/`
    - `CACHE_TEMP`: `.tmp`, `.cache`, `.temp`, hoặc nằm trong `Temp/`, `Cache/`, `Code Cache/`, `ShaderCache/`
    - `CRASH_DUMP`: `.dmp`, `.hdmp`, `.mdmp`, hoặc nằm trong `CrashDumps/`, `WER/`
    - `DB_STORAGE`: `.db`, `.sqlite`, `.wal`, `.shm`, `.mdf`, `.ldf`, `.ndf`
    - `CONTAINER_VM`: `.vhdx`, `.vhd`, `.vmdk`, `.qcow2`
    - `MEDIA_DOWNLOAD`: `.iso`, `.zip`, `.7z`, `.rar`, `.mp4`, `.mkv`, `.exe`, `.msi`
    - `APP_DATA`: Dữ liệu phát sinh khác của ứng dụng.

### 4.3. Soft Rolling Quota & Safe Zipper (`core/win_locks.py` & `core/quota_enforcer.py`)
- **Kiểm tra khóa file thực tế (`is_file_locked(path)` trong `win_locks.py`):**
  - Sử dụng `ctypes.windll.kernel32.CreateFileW` với quyền `GENERIC_READ | GENERIC_WRITE`, `dwShareMode = 0` (độc quyền tuyệt đối), `OPEN_EXISTING`. Nếu trả về `INVALID_HANDLE_VALUE` (`ERROR_SHARING_VIOLATION = 32` hoặc `ERROR_LOCK_VIOLATION = 33`), kết luận file đang được ứng dụng mở $\rightarrow$ **bỏ qua ngay, không chạm vào**. Nếu mở được handle thì đóng ngay (`CloseHandle`) và xác nhận file rảnh.
  - Cung cấp thêm hàm `get_locking_processes(path)` sử dụng `Rstrtmgr.dll` (`RmStartSession`, `RmRegisterResources`, `RmGetList`) để biết chính xác PID/App/Service nào đang giữ file.
- **Thuật toán thu hồi xoay vòng (`enforce_rule(rule)`):**
  1. Tính tổng dung lượng hiện tại `current_bytes` của mọi file trong `rule.target_paths`.
  2. Nếu `current_bytes <= rule.max_bytes`: không cần thu hồi.
  3. Nếu `current_bytes > rule.max_bytes`:
     - Mục tiêu dung lượng sau thu hồi: `target_bytes = int(rule.max_bytes * rule.watermark_ratio)` (mặc định 85% `max_bytes`).
     - Lượng cần thu hồi: `bytes_to_free = current_bytes - target_bytes`.
     - Lọc danh sách file ứng viên:
       - Không khớp `exclude_patterns` (mặc định bảo vệ `.exe`, `.dll`, `.sys`, `.ini`, `.cfg`, `.config`, `.json`, `.yaml`, `.toml`, `.db`, `.sqlite`) và không nằm trong `rule.protected_files`.
       - Tuổi file `(now - mtime) >= rule.min_file_age_seconds`.
       - `not is_file_locked(file_path)`.
     - Sắp xếp ứng viên theo `mtime` tăng dần (cũ nhất xếp trước), lấy lần lượt cho tới khi tổng kích thước $\ge$ `bytes_to_free`.
     - Đóng gói vào thư mục `Safe Vault` dưới tên `vault_<rule_id>_<YYYYMMDD_HHMMSS>.zip` (`ZIP_DEFLATED`) + `vault_<rule_id>_<YYYYMMDD_HHMMSS>.manifest.json`.
     - Kiểm tra `zipfile.ZipFile(zip_path).testzip() is None`. Chỉ khi gói `.zip` nguyên vẹn 100% mới xóa từng file gốc (kiểm tra lại `not is_file_locked` ngay trước lệnh `os.remove` để chống race condition).

### 4.4. Safety Watchdog, Auto-Rollback & Auto-Purge (`core/safety_watchdog.py`)
Chạy định kỳ hoặc theo sự kiện cho mọi gói `.zip` đang ở trạng thái `QUARANTINED`:
1. **Phát hiện dấu hiệu "Không an toàn" (Unsafe Condition Detection):**
   - **Kiểm tra 1 — Truy cập file đã nén (Missing-File Access / Log Error Scan):**
     - Theo dõi các file log/stderr mới cập nhật trong `rule.target_paths` (hoặc bộ lắng nghe sự kiện lỗi mở file): nếu ứng dụng ghi lỗi `FileNotFoundError`, `ENOENT`, `Cannot find file`, `The system cannot find the file specified` có nhắc đến tên file hoặc đường dẫn nằm trong `vault_files` của gói `.zip`, hoặc xuất hiện file yêu cầu khôi phục/sự kiện truy cập thất bại vào đường dẫn gốc.
   - **Kiểm tra 2 — Sức khỏe Windows Service / Tiến trình (`Service Health Check`):**
     - Nếu `rule.service_name` được cấu hình, kiểm tra trạng thái Service qua `psutil.win_service_get(service_name)`. Nếu trước khi nén Service đang `running` mà sau khi nén bị chuyển sang `stopped` bất thường (hoặc tiến trình mục tiêu thoát với exit code lỗi).
2. **Thực thi Rollback nguyên tử (`rollback_archive(archive_id, reason, auto_triggered)`):**
   - Đọc danh sách file từ `vault_files` và `manifest.json`.
   - Với mỗi file:
     - Tạo lại thư mục cha nếu chưa tồn tại.
     - Nếu tại `original_path` chưa có file mới: giải nén trả về đúng `original_path`, khôi phục `(atime, mtime)` bằng `os.utime`, đối chiếu mã băm `SHA-256`.
     - Nếu tại `original_path` ứng dụng đã tạo một file mới khác trong thời gian chờ: giải nén file cũ ra `original_path + ".restored_<timestamp>"` (đồng thời nếu file mới có kích thước 0 byte do lỗi mở file thì khôi phục trực tiếp bản gốc) để bảo toàn cả dữ liệu cũ lẫn mới.
   - Thêm các đường dẫn `original_path` vào `protected_files_json` của `quota_rules` để hệ thống không bao giờ nén lại các file nhạy cảm đó nữa.
   - Xóa file `.zip` sau khi đã khôi phục thành công, cập nhật trạng thái gói thành `ROLLED_BACK` kèm `status_reason` chi tiết và phát thông báo khẩn cho người dùng.
3. **Tự động xóa sau $D$ ngày an toàn (`purge_expired_archives()`):**
   - Với mọi gói `QUARANTINED` có `now >= expires_at` và không phát sinh bất kỳ cảnh báo mất an toàn nào: xóa file `.zip` và `.manifest.json` khỏi đĩa, cập nhật `status = 'PURGED'` và `status_reason = 'An toàn sau D ngày — đã xóa tự động'`.

---

## 5. Thiết kế Giao diện Người dùng (PySide6 Desktop + System Tray)

1. **Thanh công cụ trên cùng (Header Bar):**
   - Bộ chọn ổ đĩa (`C:\`, `D:\`...) kèm thanh dung lượng trực quan (`Đã dùng / Tổng dung lượng`, `Biến động hôm nay: +X MB/GB`).
   - Nút **[🧪 Chạy Mô phỏng Thực tế (Sandbox Demo)]**: Tự động tạo một thư mục ứng dụng mẫu, sinh dữ liệu theo ngày, kích hoạt vượt ngưỡng Quota để người dùng quan sát trực tiếp quá trình Nén Zip, đếm ngược $D$ ngày và thử nghiệm Tự động Rollback ngay trên giao diện.
   - Nút **[🔄 Quét & Làm mới]** và chỉ báo trạng thái chạy nền.
2. **Tab 1 — Biểu đồ Sử dụng & Truy vết theo Ngày:**
   - **Biểu đồ cột tương tác (`DailyBarChartWidget`):** Mỗi cột biểu diễn một ngày; hiển thị dung lượng sử dụng và nhãn tăng/giảm (`+MB/GB`). Hỗ trợ click chuột vào từng cột để chọn ngày cần phân tích.
   - **Bảng trái — Chỗ nào đầy lên & Nguyên nhân:** Hiển thị `Thư mục / File`, `Mức tăng (+MB/GB)`, `Nguyên nhân` (badge màu theo phân loại), `Thời gian`.
   - **Bảng phải — Do Ứng dụng / Service nào:** Hiển thị `Ứng dụng / Tiến trình`, `Windows Service`, `Dung lượng đã ghi`, `Đường dẫn tiêu biểu`, và nút **[⚡ Đặt Quota]**.
3. **Tab 2 — Quản lý Quota Ứng dụng:**
   - Danh sách các Rule Quota kèm thanh tiến trình % dung lượng hiện tại so với trần cho phép, số ngày bảo lưu $D$, số file đang được miễn trừ bảo vệ, cùng các nút **[➕ Tạo Rule Quota]**, **[⚙️ Sửa]**, **[🧹 Kiểm tra & Thu hồi ngay]**, **[🗑️ Xóa Rule]**.
4. **Tab 3 — Kho Nén Tạm (Safe Vault) & Rollback:**
   - Bảng danh sách các gói `.zip` kèm trạng thái màu sắc (`QUARANTINED` — Đang theo dõi an toàn, `ROLLED_BACK` — Đã khôi phục, `PURGED` — Đã xóa sau $D$ ngày), dung lượng tiết kiệm được, thời gian còn lại trước khi tự xóa, cùng các nút **[🔍 Xem danh sách file]**, **[⏪ Rollback khôi phục ngay]**, **[🧪 Giả lập sự cố -> Test Auto-Rollback]** và **[🗑️ Xóa vĩnh viễn ngay]**.

---

## 6. Tiêu chí Nghiệm thu & Kiểm thử Tự động

1. Tất cả unit test & integration test trong `tests/` chạy qua `pytest` đạt 100% PASS:
   - Ghi nhận và truy vấn chính xác biến động theo từng ngày (`daily_snapshots` & `write_events`).
   - Truy vết đúng tiến trình/service và phân loại đúng nguyên nhân (`LOG_GROWTH`, `CACHE_TEMP`, `CRASH_DUMP`, `DB_STORAGE`...).
   - Kiểm tra khóa file Win32 hoạt động chính xác: file đang mở (`open(path, 'r+')` hoặc giữ lock Win32) tuyệt đối không bị nén/xóa; chỉ các file cũ đã đóng mới được đưa vào `.zip` và kéo dung lượng về mức 85% Quota.
   - Kiểm tra Rollback tự động & thủ công khôi phục đúng 100% nội dung (`SHA-256` khớp tuyệt đối), khôi phục `mtime` và tự động đưa file vào danh sách bảo vệ (`protected_files`) để lần quét Quota tiếp theo không nén lại file đó.
   - Kiểm tra tự động xóa gói `.zip` khi hết hạn $D$ ngày mà vẫn an toàn.
2. Ứng dụng GUI khởi chạy mượt mà không lỗi, biểu đồ cột bấm chọn được từng ngày và cập nhật bảng chi tiết tức thời.
