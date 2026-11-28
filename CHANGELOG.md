# Changelog

Mọi thay đổi đáng chú ý của Green C:\ được ghi ở đây.
Định dạng theo [Keep a Changelog](https://keepachangelog.com/vi/1.1.0/), đánh số theo [Semantic Versioning](https://semver.org/lang/vi/).

Ghi thay đổi mới vào mục **Unreleased**. Khi phát hành, `scripts/version.py bump` sẽ chuyển mục này thành phiên bản có ngày.

## [Unreleased]

## [2.4.1] - 2026-11-28

### Sửa
- Sửa lỗi biểu đồ cột trong **Hiệu quả của GreenC** (`SavingsScorecard`) bị trong suốt/tàng hình do Tailwind v3 bỏ qua các lớp độ mờ (`bg-accent/85`, `bg-accent/15`, `bg-danger/15`, ...) khi biến màu trong `tailwind.config.js` khai báo dạng hex `var(--accent)` thay vì kênh RGB `rgb(var(--accent-rgb) / <alpha-value>)`.
- Bổ sung `aria-label` và cỡ chữ dễ đọc hơn cho các cột biểu đồ theo kết quả rà soát OCR.

## [2.4.0] - 2026-11-28

### Cải thiện
- **Bộ máy dọn rác 5 tầng tính từ mục tiêu**:
  - **Tầng 0**: Tính chính xác lượng dung lượng cần giải phóng từ mục tiêu ổ đĩa và dừng ngay khi đạt mục tiêu. Khi ổ C: đã xanh, bấm **Làm xanh ổ C: ngay** sẽ hỏi người dùng chọn **Dọn nhẹ (chỉ rác thuần túy — Tầng 1)** hoặc **Dọn sâu tối đa**.
  - **Tầng 1 (Rác thuần túy)**: Dọn các tệp ít quan trọng nhất trước (Temp, Crash Dumps, Log cũ, phiên bản cũ).
  - **Tầng 2 (Cache ứng dụng ít dùng nhất)**: Xếp hạng ứng dụng theo thời điểm ít sử dụng nhất trước, dọn bộ đệm an toàn tự tin nhất trước.
  - **Tầng 3 (Dọn sâu theo ứng dụng ít dùng nhất)**: Nén gọn WOF không mất dữ liệu & di chuyển dữ liệu nặng sang ổ dự phòng theo thứ tự ít dùng nhất trước.
  - **Tầng 4 (Sao lưu USB / ổ cứng di động)**: Nếu vẫn chưa đạt mục tiêu, tự động phát hiện USB/ổ cứng di động (hoặc mời cắm vào & bấm **Kiểm tra lại USB**), nén sao lưu ZIP có kiểm chứng SHA-256 sang USB (tuyệt đối không tạo NTFS junction lên USB tháo rời) để giải phóng ổ C: và vẫn khôi phục 1 chạm.
  - **Tầng 5 (Gợi ý gỡ ứng dụng ít dùng nhất)**: Nếu vẫn chưa đạt mục tiêu, liệt kê các ứng dụng cài trên máy từ lâu không dùng & nặng nhất trước (loại trừ thành phần hệ thống/driver/runtime) kèm nút **Mở gỡ cài đặt** chính thức.

### Sửa
- Sửa lỗi biểu đồ cột trong **Hiệu quả của GreenC** (`SavingsScorecard`) bị xẹp chiều cao thành mẩu nhỏ do CSS `%` trong container `flex-1`.
- Sửa lỗi ô **Chọn nhanh ổ đĩa...** trong Cài đặt tự reset về dòng chữ mặc định ngay sau khi người dùng chọn ổ.
- Sửa lỗi đo dung lượng ổ hệ thống `/` trên Linux/macOS khi cập nhật snapshot sau khi chạy `make_green_now`.

## [2.3.0] - 2026-11-28

### Cải thiện
- Bỏ menu trái, thay bằng thanh top bar icon gọn gàng (**Trang chủ**, **Khôi phục**, **Tùy chỉnh**, **Cài đặt**, **Chạy ngầm**).
- Nâng cấp nút **Làm xanh ổ C: ngay** chạy trọn chuỗi (Phân tích → Chọn ổ dự phòng → Dọn rác tách theo từng ứng dụng → Nén gọn WOF → Tự động chuyển dữ liệu an toàn → Báo cáo trước/sau), hỗ trợ máy 1 ổ với 2 lựa chọn và hỏi xác nhận đóng/mở lại các ứng dụng đang mở.
- Thêm bảng **Hiệu quả của GreenC** ngay trên Trang chủ theo Ngày / Tuần / Tháng / Năm kèm xếp hạng top ứng dụng sinh rác nhiều nhất.
- Trang **Khôi phục** hợp nhất, lọc theo Ngày hoặc Ứng dụng, khôi phục 1 chạm không còn thuật ngữ kỹ thuật.
- Màn **Di chuyển** hỗ trợ chọn nhiều mục + **Chuyển tất cả**, bỏ popup xác nhận từng dòng, hỗ trợ **Ẩn chạy ngầm**, hiển thị gọn tên tệp đang chuyển, tự động đóng popup khi xong, và thêm **1 chạm PowerShell** cho WSL2.
- Quốc tế hoá nhóm **Chat & Mạng xã hội** (Telegram, Discord, Slack, Teams, WhatsApp, WeChat, Viber, LINE, DingTalk, Zalo PC) và ẩn hoàn toàn các mục không còn gì để dọn/chuyển trên ổ C:.

## [2.2.3] - 2026-11-26

### Cải thiện
- Các màn hình hiện ngay kết quả quét lần trước và cập nhật ngầm. Nút dọn, di chuyển, nén, khôi phục và xoá chỉ bật khi đã có kết quả vừa quét.
- Bản đồ Space Lens tự quét ngầm khi chưa có hoặc đã cũ hơn 6 giờ.

### Sửa
- Bảng "Có thể giải phóng" ở Tổng quan luôn trống.
- Bản đồ Space Lens để hở khoảng trắng.
- Hai lượt quét dọn dẹp chạy cùng lúc có thể báo nhầm Canary không đạt.
- Trong lúc đang quét, GreenC không còn bỏ qua việc khôi phục tác vụ di chuyển dở và việc canh gác nền.
- Đang thao tác trên tệp thì các nút thao tác khác bị khoá, kể cả khi chuyển qua lại giữa các màn hình.
- Chỉ cho phép một thao tác chuyển, hoàn tác, sửa liên kết hoặc phục hồi chạy cùng lúc, kể cả giữa ứng dụng và dòng lệnh.
- Sửa liên kết: thử tạo liên kết mới trước khi bỏ liên kết cũ; không thay thư mục đang có dữ liệu.
- Phục hồi sau sự cố: không gỡ liên kết khi ổ đích chưa sẵn sàng hoặc liên kết trỏ nơi khác; chỉ xoá tệp đã có bản tương đương ở nơi còn lại; chỉ dùng thư mục tạm của đúng lần chuyển.
- Chuyển và hoàn tác: khi không trả được thư mục về chỗ cũ thì giữ cả hai bản thay vì xoá bản ở ổ đích.
- Không chuyển thư mục có liên kết bên trong (trước đây bản chuyển thiếu các liên kết này).
- Hoàn tác: yêu cầu đóng ứng dụng; không hoàn tác thư mục Windows khi ổ đích chưa sẵn sàng.
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
