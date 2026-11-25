# Changelog

Mọi thay đổi đáng chú ý của Green C:\ được ghi ở đây.
Định dạng theo [Keep a Changelog](https://keepachangelog.com/vi/1.1.0/), đánh số theo [Semantic Versioning](https://semver.org/lang/vi/).

Ghi thay đổi mới vào mục **Unreleased**. Khi phát hành, `scripts/version.py bump` sẽ chuyển mục này thành phiên bản có ngày.

## [Unreleased]

### Sửa
- Chỉ cho phép một thao tác chuyển, hoàn tác, sửa liên kết hoặc phục hồi chạy cùng lúc, kể cả giữa ứng dụng và dòng lệnh.
- Sửa liên kết: thử tạo liên kết mới trước khi bỏ liên kết cũ; không thay thư mục đang có dữ liệu.
- Phục hồi sau sự cố: không gỡ liên kết khi ổ đích chưa sẵn sàng hoặc liên kết trỏ nơi khác; chỉ xoá tệp đã có bản tương đương ở nơi còn lại; chỉ dùng thư mục tạm của đúng lần chuyển.
- Chuyển và hoàn tác: khi không trả được thư mục về chỗ cũ thì giữ cả hai bản thay vì xoá bản ở ổ đích.
- Thông báo phục hồi nêu rõ trường hợp cần người dùng xử lý.

## [2.2.2] - 2026-11-25

### Sửa (quan trọng: có thể mất tệp ở các bản trước)
- Chuyển Documents, Downloads…: tệp lưu trong lúc đang chuyển từng bị xoá cùng thư mục cũ. Nay chỉ xoá đúng những tệp đã chép và không thay đổi; phần còn lại được giữ.
- Hoàn tác: khi gặp lỗi hoặc bấm huỷ, GreenC từng xoá cả thư mục đích, kể cả tệp có sẵn. Nay không ghi đè tệp có sẵn và chỉ dọn phần do chính nó tạo ra.
- Chuyển thư mục ứng dụng: nếu dữ liệu thay đổi trong lúc chép thì dừng và giữ nguyên bản gốc; hoàn tác cũng kiểm tra tương tự.
- Không chuyển thư mục Windows có chứa dữ liệu chat Zalo; hoàn tác thư mục Zalo yêu cầu đóng Zalo.

## [2.2.1] - 2026-11-25

### Sửa
- Thư mục Zalo không còn xuất hiện trong "thư mục nâng cao" và không chuyển được bằng cơ chế chung, vốn không kiểm tra Zalo đang chạy.
- Windows: cài đè im lặng (`winget upgrade`) không còn thất bại khi GreenC đang chạy; bộ cài tự đóng rồi mở lại GreenC, và từ chối khi GreenC đang dọn hoặc chuyển dữ liệu.

## [2.2.0] - 2026-11-25

### Thêm
- **Dọn Zalo.** Dọn bộ đệm và các bản cài cũ của Zalo; không đụng tin nhắn, ảnh, tệp đã nhận, đăng nhập. Tự dọn là tuỳ chọn, mặc định tắt. Khi Zalo đang chạy, giữ bản đang chạy và bản mới nhất; khi không chạy, giữ hai bản mới nhất.
- `greenc-cli zalo`: xem tình trạng và dọn Zalo từ dòng lệnh. Tự động chuyển dữ liệu chat sang ổ khác chưa mở trong bản này; vẫn có hướng dẫn chuyển bằng tuỳ chọn của Zalo.
- Mã nguồn và quy trình đóng gói MSIX cho Microsoft Store (chưa phát hành trên Store).

### Thay đổi
- Dọn tự động theo hạn mức được viết lại: bỏ qua bộ đệm của công cụ đang chạy (Gradle, Maven, npm…), dọn theo từng đơn vị trọn vẹn, đo dung lượng thực sự giải phóng, và nghỉ giữa các lần khi không còn gì để dọn.
- Đọc chỉ mục bộ đệm không còn làm đổi thời điểm truy cập của tệp.

### Sửa
- Quy tắc hạn mức cũ (từ bản 2.0) không còn đưa ảnh và tệp Zalo vào Kho dự phòng.
- Mở lại trang Chuyển dữ liệu khi đang chuyển không còn kích hoạt khôi phục sự cố.
- Windows: liên kết qua đường dẫn tên ngắn 8.3 hoặc ổ SUBST bị báo nhầm là hỏng.
- Windows: bật khởi động cùng máy thất bại khi khoá Registry Run chưa tồn tại.
- Windows: các mục đã chờ xoá không bao giờ được xoá thật.
- Windows: đường dẫn dạng `\\?\` bị coi là ổ khác.

## [2.1.2] - 2026-11-21

### Thay đổi (từ 2.0.0 bản Python tới 2.1.2)
- **Viết lại toàn bộ bằng Rust + Tauri v2 + Svelte 5** (bản v2.0.0), thay thế hoàn toàn bản Python. Bản Python cũ nằm ở nhánh `python-legacy` (chỉ có trên máy, chưa đẩy lên remote).
- Hạn mức dung lượng theo từng ổ đĩa làm căn cứ cảnh báo, sao lưu Vault và dọn dẹp tự động.
- Tự sửa đường dẫn gói Vault khi thư mục Vault đã đổi chỗ; gói mất tệp ZIP được đánh dấu MISSING.
- Hộp tiến trình dọn dẹp hiện "Đang chuẩn bị dọn dẹp…" 0/N ngay khi bấm, thay vì hiện trạng thái hoàn tất cũ của lần quét trước.
- Sửa lỗi ứng dụng thoát ngay khi mở do thiếu Tokio reactor.
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
