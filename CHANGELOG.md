# Changelog

Mọi thay đổi đáng chú ý của Green C:\ được ghi ở đây.
Định dạng theo [Keep a Changelog](https://keepachangelog.com/vi/1.1.0/), đánh số theo [Semantic Versioning](https://semver.org/lang/vi/).

Ghi thay đổi mới vào mục **Unreleased**. Khi phát hành, `scripts/version.py bump` sẽ chuyển mục này thành phiên bản có ngày.

## [Unreleased]

### Thay đổi
- Nén vào Kho dự phòng nhanh hơn khoảng 4,5 lần trên bộ đệm npm (11.900 file, 1,4 GB: từ 448 giây xuống khoảng 100 giây): file nhỏ được đọc, hash và nén song song; file lớn được nén song song theo từng khối; dữ liệu đã nén sẵn (gzip, zip, ảnh…) được lưu thẳng, không nén lại; bỏ các lần stat và kiểm tra trùng lặp.
- Thanh trạng thái khi dọn ứng dụng hiện từng giai đoạn, số file, dung lượng, tốc độ và thời gian còn lại.

## [2.0.0] - 2026-11-12

### Thêm
- Bộ cài `GreenC-Setup-<version>.exe` (Inno Setup), cài cho người dùng hiện tại, không cần quyền Admin.
- Quản lý version: `scripts/version.py`, CHANGELOG, tag `vX.Y.Z` và GitHub Actions tự build bộ cài khi đẩy tag.
- Chọn ổ dữ liệu ở lần chạy đầu và trong Cài đặt; Vault và dữ liệu ứng dụng được chuyển sang ổ đó.
- Trang Chuyển dữ liệu: thư mục Windows (Downloads, Documents…), bộ đệm npm/pip, hướng dẫn Zalo, chuyển thư mục kèm junction, hoàn tác.
- Số ngày giữ bản lưu mặc định 7 ngày, chỉnh được trong Cài đặt.
- Chỉ cho chạy một cửa sổ ứng dụng cùng lúc.
- Giao diện 50 ngôn ngữ, chọn trong Cài đặt → Ngôn ngữ; mặc định theo ngôn ngữ Windows; hỗ trợ ngôn ngữ viết từ phải sang trái.

### Thay đổi
- Đổi tên thành **Green C:\\** (trước đây là DiskGuard; package Python `greenc`), tác giả Cùng Chat (cungchat.com).
- Giao diện mới kiểu Fluent: Tổng quan, Dọn dẹp, Chuyển dữ liệu, Vault, Cài đặt; bảng chi tiết bật bằng icon con mắt; ô đường dẫn có nút sao chép và mở thư mục.
- Dữ liệu ứng dụng chuyển về `%LOCALAPPDATA%\Green C` (tự di chuyển từ `C:\DiskGuard`).

### Sửa
- Lỗi nén file lớn hơn 2 GiB (ZIP64), kiểm tra dung lượng trống của Vault, nén nhanh hơn.
- Quét an toàn với junction; không xóa file bị sửa trong lúc lưu trữ; chu trình nền chạy ngoài luồng giao diện.

## [0.1.0] - 2026-11-09

### Thêm
- Bản DiskGuard đầu tiên: biểu đồ dung lượng theo ngày, truy vết nguyên nhân, hồ sơ Zalo/DingTalk, quota cuộn với Vault và tự khôi phục.
