# DiskGuard — Ứng dụng Theo Dõi Biến Động Ổ Cứng, Truy Vết Tác Nhân (Zalo/DingTalk) & Quản Lý Quota An Toàn

Ứng dụng Desktop Windows (`PySide6` + `SQLite` + `Win32 API`) giải quyết trọn vẹn 5 bài toán:
1. **Biểu đồ cột sử dụng & biến động ổ đĩa theo từng ngày:** Bấm vào bất kỳ cột ngày nào để xem chi tiết ngày đó.
2. **Truy vết nguyên nhân làm đầy ổ cứng:** Thư mục nào đầy lên, nguyên nhân vì sao (`MEDIA_CHAT`, `EAPP_BUNDLE`, `CACHE_TEMP`, `OLD_APP_VERSION`, `CRASH_DUMP`, `LOG_GROWTH`...), vào ngày nào, và do Ứng dụng (`.exe`) hay Windows Service nào.
3. **Nhận diện chuyên sâu Zalo & DingTalk (Multi-Drive & NTFS HardLink):**
   - Tự động đọc `%APPDATA%\ZaloData\config.json` để tìm kho Media Zalo liên ổ đĩa (ví dụ `F:\Zalo Data\media`), bộ nhớ đệm và các bản cài đặt Zalo cũ bị bỏ quên (`%LOCALAPPDATA%\Programs\Zalo\Zalo-*`).
   - Tự động phát hiện các gói Mini-App `.amr` lưu cữu trong `DingTalk\*_v2\EAppFiles\download`, `EAppFiles\unziped`, `resource_cache` và `dumps`.
   - Khử trùng lặp `NTFS HardLink` (`st_ino`) để không đếm ảo dung lượng Zalo và thu hồi/khôi phục đồng bộ toàn bộ các nhánh HardLink.
4. **Soft Rolling Quota không gây gián đoạn ứng dụng:** Khi vượt trần Quota, chỉ chọn các file cũ nhất hiện **không bị khóa** (kiểm tra qua Win32 API `CreateFileW` với `dwShareMode=0`) và không nằm trong danh sách cơ sở dữ liệu/chỉ mục bảo vệ (`.db`, `.db-wal`, `.rescache`).
5. **Chu trình Nén Zip tạm (Safe Vault) $\rightarrow$ Đếm ngược $D$ ngày $\rightarrow$ Tự động Xóa hoặc Tự động Rollback:**
   - Lưu file cũ vào `.zip` kèm `manifest.json` và mã băm `SHA-256`.
   - Trong $D$ ngày: nếu phát hiện ứng dụng báo lỗi thiếu file (`FileNotFoundError` / `ENOENT`) hoặc Windows Service dừng bất thường $\rightarrow$ **Tự động Rollback giải nén trả lại ngay lập tức** (kèm tái tạo `os.link` HardLink) và khóa miễn trừ vĩnh viễn.
   - Sau $D$ ngày an toàn $\rightarrow$ **Tự động xóa vĩnh viễn** gói `.zip`.

## Cách chạy ứng dụng

```powershell
cd C:\DiskGuard
uv run python main.py
```

## Cách chạy bộ kiểm thử tự động

```powershell
cd C:\DiskGuard
uv run pytest -v
```
