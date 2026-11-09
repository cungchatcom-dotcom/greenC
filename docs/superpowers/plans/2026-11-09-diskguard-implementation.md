# DiskGuard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Xây dựng ứng dụng Desktop Windows **DiskGuard** (`C:\DiskGuard`) để theo dõi biến động dung lượng ổ đĩa theo ngày dạng biểu đồ cột tương tác, truy vết thư mục/nguyên nhân/ứng dụng/service làm đầy đĩa (tích hợp nhận diện chuyên sâu Zalo & DingTalk liên ổ đĩa), giới hạn Soft Rolling Quota tự động nén các file cũ không bị lock (hỗ trợ nhóm NTFS HardLink) vào Safe Vault, tự xóa sau $D$ ngày nếu an toàn hoặc tự động Rollback ngay lập tức nếu phát hiện sự cố.

**Architecture:** Kiến trúc tách lớp gồm tầng lưu trữ SQLite (`diskguard/storage/database.py`), 5 module lõi (`disk_tracker.py`, `attribution.py`, `app_profiles.py`, `win_locks.py`, `quota_enforcer.py`, `safety_watchdog.py`, `simulator.py`) và giao diện Desktop PySide6 + System Tray (`diskguard/ui/`). Mọi thao tác thu hồi file đều qua kiểm tra khóa file cấp Windows API (`CreateFileW` với `dwShareMode=0`), hỗ trợ khử trùng lặp và khôi phục nguyên trạng nhóm `NTFS HardLink` (`st_ino`), đảm bảo ứng dụng mục tiêu chạy liên tục không gián đoạn.

**Tech Stack:** Python 3.12 (quản lý qua `uv`), `PySide6`, `psutil`, `watchdog`, `pytest`, `sqlite3`, `ctypes` (Win32 API `kernel32` & `Rstrtmgr`).

**Spec:** `C:\DiskGuard\docs\superpowers\specs\2026-11-09-diskguard-design.md`

## Global Constraints

- **Python & Dependency Management:** Sử dụng `uv` với Python 3.12 tại `C:\DiskGuard\.venv` (`uv init`, `uv add PySide6 psutil watchdog`, `uv add --dev pytest`). Không dùng `pip install` toàn cục.
- **Zero Disruption Rule:** Không bao giờ xóa hoặc nén các file đang bị khóa (`is_file_locked(path) == True`), các file cơ sở dữ liệu/cấu hình trong `exclude_patterns` (`.db`, `.db-wal`, `.db-shm`, `.sqlite`, `.rescache`, `lockfile`, `.exe`, `.dll`, `.json`, `.ini`), hoặc các file trong `protected_files`.
- **HardLink Integrity:** Khi đo dung lượng thực tế phải khử trùng lặp theo `(st_dev, st_ino)`. Khi nén một file có nhiều đường dẫn HardLink nằm trong vùng giám sát, phải lưu toàn bộ `hardlink_paths` vào manifest, xóa đồng bộ tất cả các nhánh liên kết sau khi kiểm tra `.zip` hợp lệ, và dùng `os.link()` khôi phục lại toàn bộ nhánh khi Rollback.
- **Transactional Zip-then-Delete:** Luôn kiểm tra `zipfile.ZipFile(zip_path).testzip() is None` trước khi xóa file gốc.

---

### Task 1: Khởi tạo Dự án Python (`uv`), Mô hình Dữ liệu & Tầng Lưu trữ SQLite (`database.py`)

**Files:**
- Create: `C:\DiskGuard\pyproject.toml`
- Create: `C:\DiskGuard\diskguard\__init__.py`
- Create: `C:\DiskGuard\diskguard\config.py`
- Create: `C:\DiskGuard\diskguard\models.py`
- Create: `C:\DiskGuard\diskguard\storage\__init__.py`
- Create: `C:\DiskGuard\diskguard\storage\database.py`
- Test: `C:\DiskGuard\tests\test_database_and_daily_delta.py`

**Interfaces:**
- Produces:
  - `DiskGuardDB(db_path: Path)` with methods:
    - `upsert_daily_snapshot(drive: str, snapshot_date: str, total_bytes: int, used_bytes: int, free_bytes: int, net_change_bytes: int | None = None) -> DailySnapshot`
    - `get_daily_snapshots(drive: str, limit_days: int = 14) -> list[DailySnapshot]`
    - `record_write_event(event: WriteEvent) -> int`
    - `get_day_folder_breakdown(drive: str, event_date: str) -> list[dict]`
    - `get_day_actor_breakdown(drive: str, event_date: str) -> list[dict]`
    - `save_quota_rule(rule: QuotaRule) -> int`
    - `list_quota_rules(enabled_only: bool = False) -> list[QuotaRule]`
    - `get_quota_rule(rule_id: int) -> QuotaRule | None`
    - `add_protected_files(rule_id: int, paths: list[str]) -> None`
    - `delete_quota_rule(rule_id: int) -> None`
    - `create_vault_archive(archive: VaultArchive, files: list[FileManifestItem]) -> int`
    - `list_vault_archives(status: str | None = None) -> list[VaultArchive]`
    - `get_vault_archive(archive_id: int) -> VaultArchive | None`
    - `get_vault_files(archive_id: int) -> list[FileManifestItem]`
    - `update_vault_status(archive_id: int, status: str, reason: str) -> None`

- [ ] **Step 1: Khởi tạo môi trường `uv` với Python 3.12 và cài đặt thư viện**
  - Run: `Set-Location C:\DiskGuard; uv venv --python 3.12; uv init --no-readme --bare`
  - Cài đặt gói: `uv add PySide6 psutil watchdog` và `uv add --dev pytest`

- [ ] **Step 2: Viết unit test kiểm tra `DiskGuardDB` (`tests/test_database_and_daily_delta.py`)**
  - Kiểm tra lưu/cập nhật `daily_snapshots` và tự động tính `net_change_bytes` so với ngày hôm trước.
  - Kiểm tra ghi `write_events` và truy vấn drill-down theo ngày (`get_day_folder_breakdown` & `get_day_actor_breakdown`).
  - Kiểm tra CRUD cho `quota_rules`, `vault_archives` và `vault_files` (bao gồm `hardlink_paths`).

- [ ] **Step 3: Chạy `pytest` để xác nhận test fail trước khi cài đặt**
  - Run: `uv run pytest tests/test_database_and_daily_delta.py -v`

- [ ] **Step 4: Cài đặt `config.py`, `models.py` và `storage/database.py`**
  - Đảm bảo kết nối SQLite bật `WAL` mode, đầy đủ bảng và index theo đúng Spec.

- [ ] **Step 5: Chạy lại `pytest` xác nhận PASS và commit**
  - Run: `uv run pytest tests/test_database_and_daily_delta.py -v`
  - Commit: `feat: initialize project and implement SQLite storage layer`

---

### Task 2: Kiểm tra Khóa File Win32, Khử Trùng Lặp `NTFS HardLink` & Truy Vết Ứng Dụng / Service / Zalo / DingTalk (`win_locks.py`, `attribution.py`, `app_profiles.py`, `disk_tracker.py`)

**Files:**
- Create: `C:\DiskGuard\diskguard\core\__init__.py`
- Create: `C:\DiskGuard\diskguard\core\win_locks.py`
- Create: `C:\DiskGuard\diskguard\core\attribution.py`
- Create: `C:\DiskGuard\diskguard\core\app_profiles.py`
- Create: `C:\DiskGuard\diskguard\core\disk_tracker.py`
- Test: `C:\DiskGuard\tests\test_attribution_engine.py`

**Interfaces:**
- Produces:
  - `is_file_locked(path: str | Path) -> bool`
  - `get_locking_processes(path: str | Path) -> list[dict]`
  - `compute_physical_usage(paths: list[str | Path]) -> tuple[int, int, dict[tuple[int, int], list[Path]]]` (trả về `(physical_bytes, naive_bytes, inode_map)`)
  - `classify_write_reason(file_path: str | Path) -> tuple[str, str]`
  - `AttributionEngine` (ánh xạ đường dẫn/sự kiện ghi sang `process_name`, `pid`, `service_name`, `service_display_name`, `reason_code`, `reason_detail`)
  - `discover_installed_heavy_apps() -> list[AppProfile]` (tự động phát hiện cấu trúc thực tế của **Zalo** qua `%APPDATA%\ZaloData\config.json` trên cả `C:\` lẫn `F:\Zalo Data`, **DingTalk** tại `%APPDATA%\DingTalk\*_v2` & `%LOCALAPPDATA%\DingTalk\dumps`, cùng các thư mục hệ thống nóng)
  - `DiskTracker` (chụp dung lượng ổ đĩa, quét ban đầu phân bổ theo ngày từ dấu thời gian thực tế của các thư mục ứng dụng trên ổ đĩa để vẽ ngay biểu đồ lịch sử nhiều ngày).

- [ ] **Step 1: Viết unit test trong `tests/test_attribution_engine.py`**
  - Kiểm tra `is_file_locked` phát hiện đúng file đang bị khóa độc quyền bởi Win32 API `CreateFileW`.
  - Kiểm tra `compute_physical_usage` không đếm trùng dung lượng khi có `os.link()` (NTFS HardLink).
  - Kiểm tra `classify_write_reason` phân loại chính xác `.log`, `.dmp`, `.amr`, `fileNoise`, `.db-wal`, `Cache`, v.v.
  - Kiểm tra `discover_installed_heavy_apps` đọc đúng file `config.json` mẫu của Zalo (cross-drive) và cấu trúc thư mục DingTalk.

- [ ] **Step 2: Chạy `pytest tests/test_attribution_engine.py -v` để xác nhận fail**

- [ ] **Step 3: Cài đặt `win_locks.py`, `attribution.py`, `app_profiles.py`, và `disk_tracker.py`**

- [ ] **Step 4: Chạy `pytest tests/test_attribution_engine.py -v` xác nhận PASS và commit**
  - Commit: `feat: add Win32 file lock detector, NTFS hardlink scanner, attribution engine, and Zalo/DingTalk profiles`

---

### Task 3: Soft Rolling Quota Enforcer, HardLink-Aware Safe Zipper & Safety Watchdog (Auto-Rollback & Auto-Purge sau $D$ ngày)

**Files:**
- Create: `C:\DiskGuard\diskguard\core\quota_enforcer.py`
- Create: `C:\DiskGuard\diskguard\core\safety_watchdog.py`
- Create: `C:\DiskGuard\diskguard\core\simulator.py`
- Test: `C:\DiskGuard\tests\test_win_locks_and_zipper.py`
- Test: `C:\DiskGuard\tests\test_safety_rollback_and_purge.py`

**Interfaces:**
- Consumes: `DiskGuardDB`, `is_file_locked`, `compute_physical_usage`
- Produces:
  - `QuotaEnforcer(db: DiskGuardDB, vault_dir: Path)`:
    - `evaluate_rule_usage(rule: QuotaRule) -> dict`
    - `enforce_rule(rule: QuotaRule, now_ts: float | None = None) -> VaultArchive | None`
    - `enforce_all_rules() -> list[VaultArchive]`
  - `SafetyWatchdog(db: DiskGuardDB)`:
    - `check_archive_safety(archive: VaultArchive) -> tuple[bool, str]`
    - `rollback_archive(archive_id: int, reason: str, auto_triggered: bool = False) -> dict`
    - `purge_expired_archives(now_dt: datetime | None = None) -> list[int]`
    - `run_safety_cycle(now_dt: datetime | None = None) -> dict`
  - `SandboxSimulator(db: DiskGuardDB, base_dir: Path)`:
    - Tạo kịch bản mô phỏng thực tế (bao gồm cả mô phỏng Zalo HardLink Media & Log Service) để kiểm nghiệm trực quan trên GUI.

- [ ] **Step 1: Viết unit tests trong `tests/test_win_locks_and_zipper.py` và `tests/test_safety_rollback_and_purge.py`**
  - Kiểm tra khi thư mục vượt `max_bytes`:
    - File đang bị lock (`CreateFileW` giữ handle), file trong `exclude_patterns` (`.db`, `.rescache`, `.json`) và file quá mới (`< min_file_age_seconds`) **không bị nén**.
    - Các file cũ nhất (kể cả nhóm file `NTFS HardLink` như `fileNoise` + `resource\<id>\fileNoise`) được gom vào `.zip` + `.manifest.json`, xóa toàn bộ các nhánh HardLink để dung lượng thực giảm về $\le 85\%$ `max_bytes`.
  - Kiểm tra `SafetyWatchdog`:
    - Khi phát hiện ứng dụng ghi lỗi thiếu file (`FileNotFoundError` / `Cannot find file` chứa tên file đã nén) hoặc yêu cầu truy cập file thiếu $\rightarrow$ **Tự động Rollback ngay lập tức**, khôi phục đúng nội dung (`SHA-256` khớp 100%), khôi phục cả các nhánh `os.link()` (HardLink), và thêm file vào `protected_files` của Rule.
    - Khi chạy lại `enforce_rule` sau khi Rollback, các file trong `protected_files` tuyệt đối không bị nén lại.
    - Khi hết hạn $D$ ngày mà không có lỗi $\rightarrow$ `purge_expired_archives` tự động xóa file `.zip` và chuyển trạng thái sang `PURGED`.

- [ ] **Step 2: Chạy `pytest` xác nhận fail**

- [ ] **Step 3: Cài đặt `quota_enforcer.py`, `safety_watchdog.py`, và `simulator.py`**

- [ ] **Step 4: Chạy toàn bộ `pytest` xác nhận PASS 100% và commit**
  - Commit: `feat: implement soft rolling quota zipper, hardlink rollback, and D-day safety watchdog`

---

### Task 4: Giao diện Desktop PySide6 (Biểu đồ Cột Tương tác theo Ngày, Drill-down, Quản lý Quota Zalo/DingTalk, Safe Vault & System Tray)

**Files:**
- Create: `C:\DiskGuard\diskguard\ui\__init__.py`
- Create: `C:\DiskGuard\diskguard\ui\chart_widget.py`
- Create: `C:\DiskGuard\diskguard\ui\daily_tab.py`
- Create: `C:\DiskGuard\diskguard\ui\quota_tab.py`
- Create: `C:\DiskGuard\diskguard\ui\vault_tab.py`
- Create: `C:\DiskGuard\diskguard\ui\dialogs.py`
- Create: `C:\DiskGuard\diskguard\ui\main_window.py`
- Create: `C:\DiskGuard\main.py`
- Create: `C:\DiskGuard\README.md`
- Test: `C:\DiskGuard\tests\test_ui_headless.py`

**Interfaces:**
- Produces:
  - `DailyBarChartWidget(QWidget)`: Biểu đồ cột vẽ bằng `QPainter` hiển thị mức sử dụng & mức tăng/giảm từng ngày, hỗ trợ hover tooltip và sự kiện click chuột `daySelected = Signal(str)` để lọc bảng chi tiết bên dưới.
  - `MainWindow(QMainWindow)`:
    - Thanh công cụ chọn ổ đĩa (`C:\`, `E:\`, `F:\`...), nút **[🔍 Quét Dữ Liệu Thật (Zalo / DingTalk / Ổ đĩa)]**, nút **[⚡ Tạo nhanh Quota cho Zalo / DingTalk]**, nút **[🧪 Chạy Mô phỏng Sandbox (Demo Nén & Auto-Rollback)]**.
    - **Tab 1 (Biểu đồ Ngày & Truy vết Nguyên nhân):** Biểu đồ cột + 2 bảng drill-down theo ngày đang chọn (Bảng Thư mục/Nguyên nhân & Bảng Ứng dụng/Windows Service kèm nút đặt Quota 1-click).
    - **Tab 2 (Quản lý Quota Xoay vòng):** Hiển thị dung lượng vật lý thực tế (đã khử trùng lặp HardLink) so với trần Quota, chỉnh sửa ngưỡng MB/GB, số ngày $D$, danh sách miễn trừ, nút thu hồi ngay.
    - **Tab 3 (Kho Nén Tạm Safe Vault & Rollback):** Danh sách gói `.zip`, dung lượng gốc/nén, bộ đếm ngược $D$ ngày, nút xem danh sách file trong `.zip`, nút **[⏪ Rollback khôi phục ngay]**, nút **[🧪 Giả lập ứng dụng cần lại file -> Test Auto-Rollback]**, nút **[🗑️ Xóa vĩnh viễn ngay]**.
    - Tích hợp `QSystemTrayIcon` chạy ngầm và thông báo hệ thống.

- [ ] **Step 1: Viết headless UI test (`tests/test_ui_headless.py`) kiểm tra khởi tạo `MainWindow`, chọn ngày trên biểu đồ cột, nạp dữ liệu thực và thao tác Quota/Rollback**
- [ ] **Step 2: Cài đặt các module giao diện trong `diskguard/ui/` và `main.py`**
- [ ] **Step 3: Chạy toàn bộ bộ test `uv run pytest -v` xác nhận PASS 100% và khởi chạy thử ứng dụng**
- [ ] **Step 4: Commit hoàn chỉnh mã nguồn**
  - Commit: `feat: add PySide6 desktop UI with interactive daily bar chart, Zalo/DingTalk presets, and system tray`
