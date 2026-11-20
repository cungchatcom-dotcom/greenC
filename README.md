# GreenC v2 (Rust Edition)

**Green your C drive forever.** Thế hệ tiếp theo của **GreenC** được thiết kế và viết lại toàn bộ bằng **Rust** theo phương pháp **Spec-Driven Development** (GitHub Spec Kit).

Tác giả: **Cùng Chat** · [cungchat.com](https://cungchat.com)

---

## Tài liệu Đặc tả & Kế hoạch Kiến trúc (Spec Kit)

Toàn bộ nguyên tắc bất biến, đặc tả tính năng (bao gồm đầy đủ tính năng hiện có + chiều sâu mở rộng), mô hình dữ liệu, hợp đồng giao tiếp và danh sách tác vụ triển khai nằm tại:

- **Hiến pháp dự án (Constitution)**: [`.specify/memory/constitution.md`](.specify/memory/constitution.md)
- **Đặc tả tính năng toàn diện (Specification)**: [`specs/001-greenc-complete-platform/spec.md`](specs/001-greenc-complete-platform/spec.md)
- **Checklist chất lượng đặc tả**: [`specs/001-greenc-complete-platform/checklists/requirements.md`](specs/001-greenc-complete-platform/checklists/requirements.md)
- **Kế hoạch kiến trúc Rust (Implementation Plan)**: [`specs/001-greenc-complete-platform/plan.md`](specs/001-greenc-complete-platform/plan.md)
- **Nghiên cứu kỹ thuật (Phase 0 Research)**: [`specs/001-greenc-complete-platform/research.md`](specs/001-greenc-complete-platform/research.md)
- **Mô hình dữ liệu SQLite WAL v2 & Kiểu Rust**: [`specs/001-greenc-complete-platform/data-model.md`](specs/001-greenc-complete-platform/data-model.md)
- **Hợp đồng Tauri IPC & CLI**: [`specs/001-greenc-complete-platform/contracts/`](specs/001-greenc-complete-platform/contracts/)
- **Hướng dẫn khởi chạy & nghiệm thu**: [`specs/001-greenc-complete-platform/quickstart.md`](specs/001-greenc-complete-platform/quickstart.md)
- **Danh sách tác vụ thực thi chi tiết (Tasks)**: [`specs/001-greenc-complete-platform/tasks.md`](specs/001-greenc-complete-platform/tasks.md)

---

## Cấu trúc Mã nguồn Dự kiến (`C:\DiskGuard`)

- `crates/greenc-core`: Kiểu miền cốt lõi, cấu hình `GreenC`, bộ điều phối tác vụ `TaskControl` (Pause/Resume/Stop) và phân loại lỗi.
- `crates/greenc-win32`: Tầng bọc an toàn RAII cho Windows API (`CreateFileW` khóa độc quyền, Restart Manager, NTFS Junction `FSCTL_SET_REPARSE_POINT`, Known Folders, nén lạnh `WOF XPRESS`, thu gọn `.vhdx`, khóa đơn phiên bản `GreenCAppMutex`).
- `crates/greenc-storage`: Lưu trữ SQLite WAL nhúng tĩnh (`rusqlite`), tự động nâng cấp lược đồ và di chuyển dữ liệu từ bản cũ (`DiskGuard`, `Green C`, `GreenC` Python v2.0).
- `crates/greenc-i18n`: Danh mục 50 ngôn ngữ, hỗ trợ RTL và đổi ngôn ngữ trực tiếp không cần khởi động lại.
- `crates/greenc-scanner`: Quét ổ đĩa đa luồng `rayon` + tăng tốc `USN/MFT`, khử trùng lặp `NTFS HardLink`, Space Lens Treemap, dự báo đầy ổ đĩa, hồ sơ ứng dụng và phát hiện dữ liệu mồ côi.
- `crates/greenc-vault`: Đóng gói song song `ZIP64` (`STORED`/`DEFLATE`/`ZSTD`), Canary Probe 1 chu trình, khôi phục lẻ từng tệp và Safety Watchdog tự động Rollback.
- `crates/greenc-relocate`: Di chuyển dữ liệu 4 chiến lược (Known Folders, 10+ bộ đệm công cụ Dev/AI, hướng dẫn Zalo/WSL2/Docker, liên kết thư mục chống sập nguồn) & nén lạnh tại chỗ.
- `crates/greenc-cli`: Công cụ dòng lệnh chẩn đoán, tự động hóa và xuất/nhập cấu hình quy tắc (`--json`).
- `crates/greenc-app` & `ui/`: Ứng dụng Desktop Tauri v2 + Svelte 5 chuẩn Windows 11 Fluent, giải phóng WebView2 khi thu xuống khay hệ thống (`< 15 MB RAM`).

---

## Quy trình Triển khai với Antigravity & Spec Kit

Dự án đã được cấu hình sẵn tích hợp Spec Kit cho Antigravity (`agy`). Để bắt đầu viết mã từng giai đoạn theo kế hoạch đã lập, sử dụng kỹ năng `/speckit-implement` dựa trên [`specs/001-greenc-complete-platform/tasks.md`](specs/001-greenc-complete-platform/tasks.md).
