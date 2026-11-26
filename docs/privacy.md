# Chính sách quyền riêng tư / Privacy Policy — GreenC

*Cập nhật lần cuối / Last updated: 2026-11-26*

---

## Tiếng Việt

### 1. Nguyên tắc chung
**GreenC** là phần mềm quản lý dung lượng ổ đĩa mã nguồn mở do **Cùng Chat** phát triển. Mọi tính năng quét ổ đĩa, phân tích dung lượng, dọn dẹp tệp tạm, nén lưu trữ (Kho dự phòng / Safety Vault) và di chuyển thư mục đều hoạt động **100% cục bộ trên thiết bị của bạn**.

### 2. Dữ liệu chúng tôi KHÔNG thu thập
GreenC **không** thu thập, lưu trữ trên máy chủ hay chia sẻ với bên thứ ba bất kỳ dữ liệu nào sau đây:
- Nội dung tệp tin, tên tệp, đường dẫn thư mục hoặc cấu trúc ổ đĩa của bạn.
- Thông tin cá nhân (tên, email, tài khoản, vị trí địa lý, nội dung thông báo lỗi chi tiết).
- Mã định danh thiết bị, mã định danh người dùng, cookie hay mã theo dõi xuyên phiên.
- Quảng cáo hoặc SDK theo dõi của bên thứ ba.
- Bất kỳ dữ liệu chẩn đoán nào khi bạn **chưa đồng ý**.

### 3. Chẩn đoán ẩn danh khi bạn đồng ý (chỉ trên bản cài đặt độc lập)
Để phát hiện lỗi và cải thiện tốc độ quét/dọn dẹp, bản cài đặt độc lập có tuỳ chọn **gửi số đếm chẩn đoán ẩn danh** (mặc định **tắt**, chỉ đếm và gửi khi bạn chủ động chọn **Đồng ý gửi** hoặc bật trong **Cài đặt**):
- **Các trường dữ liệu được gửi** (`POST https://greenc-log.cungchat.com/v1/counters`, tối đa khoảng 1 lần mỗi 20 giờ hoặc sau khi ứng dụng gặp sự cố sập):
  - `day`: ngày theo UTC (`YYYY-MM-DD`).
  - `version`: phiên bản GreenC (ví dụ `2.2.3`).
  - `os`: nhóm hệ điều hành (`windows10`, `windows11`, `macos`, `linux`).
  - `build`: số build Windows hoặc phiên bản chính macOS (chỉ gửi kèm sự kiện `active`, Linux là `0`).
  - `disk`: nhóm dung lượng ổ hệ thống (`lt128`, `lt256`, `lt512`, `lt1024`, `ge1024` GiB, chỉ gửi kèm sự kiện `active`).
  - `event`: loại sự kiện (`active`, `start`, `scan`, `screen`, `action`, `crash`).
  - `name`: tên cố định của màn hình/lệnh quét/thao tác; với sự kiện `crash` chỉ là vị trí tệp mã nguồn thuộc GreenC (dạng `crates/greenc-.../src/....rs:dòng`) hoặc `dependency`.
  - `outcome`: kết quả (`ok`, `partial`, `failed`, `cancelled`, `cached`, `fresh`).
  - `code`: mã nhóm lỗi chuẩn hoá (ví dụ `Io`, `FileLocked`, `Win32:5`, không kèm đường dẫn hay nội dung lỗi).
  - `duration`: khoảng thời gian thực hiện (`lt500ms`, `lt2s`, `lt5s`, `lt15s`, `lt60s`, `lt5m`, `ge5m`).
  - `count`: số lần xảy ra trong ngày.
  - `batch`: UUID ngẫu nhiên tạo mới cho mỗi lô gửi, chỉ dùng để máy chủ chống đếm trùng khi thử gửi lại.
- **Thời gian lưu giữ trên máy chủ:**
  - Dữ liệu số đếm chi tiết theo ngày được giữ tối đa **90 ngày**.
  - Số liệu tổng hợp theo tháng được giữ tối đa **24 tháng**.
- **Địa chỉ IP & hạ tầng truyền tải:** Kết nối HTTPS đi qua **Cloudflare** để bảo vệ và định tuyến; Cloudflare thấy địa chỉ IP của bạn theo chính sách quyền riêng tư của Cloudflare. Máy chủ nhận chẩn đoán của GreenC **không lưu địa chỉ IP** của bạn.
- **Nhật ký cục bộ trên máy:** Bản cài đặt độc lập ghi nhật ký sự kiện đã làm sạch (không tên tệp, không đường dẫn) trong thư mục ứng dụng (`diagnostics/log/`, tối đa 5 tệp × 1 MiB, tự xoá tệp cũ hơn 7 ngày). Bạn có thể **Xem dữ liệu sẽ gửi**, **Xuất nhật ký** hoặc **Xoá nhật ký** bất kỳ lúc nào trong **Cài đặt**.
- **Quyền tắt bất kỳ lúc nào:** Khi bạn tắt tuỳ chọn gửi chẩn đoán trong **Cài đặt**, GreenC **xoá ngay lập tức** toàn bộ dữ liệu đang chờ gửi, lô đang dở và dấu sập cục bộ, đồng thời ngừng đếm và gửi.
- **Phiên bản Microsoft Store (MSIX):** Tính năng chẩn đoán bị **tắt hoàn toàn ngay từ khi biên dịch** — không đếm, không ghi nhật ký chẩn đoán, không hiện tuỳ chọn và **không bao giờ gửi** dữ liệu chẩn đoán.

### 4. Dữ liệu lưu cục bộ & Kết nối mạng thực tế của ứng dụng
Để vận hành, GreenC lưu cơ sở dữ liệu cấu hình và lịch sử cục bộ (SQLite `greenc.db`), cùng các gói sao lưu ZIP64 trong Kho dự phòng tại thư mục ứng dụng cục bộ hoặc ổ dữ liệu bạn chọn:
- **Bản cài đặt độc lập (EXE / winget / macOS / Linux):** lưu trong `%LOCALAPPDATA%\GreenC` (Windows) hoặc thư mục dữ liệu người dùng chuẩn của hệ điều hành, và ổ dữ liệu bạn chọn.
  - *Kiểm tra cập nhật:* Gửi yêu cầu HTTPS `GET` tới GitHub Releases API (`https://api.github.com/repos/cungchatcom-dotcom/greenC/releases/latest`) kèm tiêu đề `User-Agent: GreenC/<version>` để kiểm tra bản mới. Khi bạn bấm **Cập nhật ngay**, ứng dụng tải bộ cài từ `https://github.com/cungchatcom-dotcom/greenC/releases/download/...`.
  - *Chẩn đoán ẩn danh (chỉ khi bạn đồng ý):* Gửi yêu cầu HTTPS `POST` tới `https://greenc-log.cungchat.com/v1/counters` như mô tả ở mục 3.
- **Bản Microsoft Store (MSIX):** lưu trong vùng dữ liệu cô lập của gói ứng dụng (`%LOCALAPPDATA%\GreenC-Store` ảo hoá theo chuẩn MSIX) và ổ dữ liệu bạn chọn. **Không** tự thực hiện bất kỳ kết nối mạng nền nào (không kiểm tra cập nhật GitHub, không gửi chẩn đoán). Mọi bản cập nhật đều do Microsoft Store quản lý. Nếu bạn chủ động bấm vào liên kết trang chủ (`https://cungchat.com`) hoặc liên kết mở Microsoft Store (`ms-windows-store://`), hệ điều hành sẽ mở bằng trình duyệt hoặc ứng dụng Microsoft Store mặc định.

### 5. Quyền kiểm soát của bạn
Bạn có thể bật/tắt chẩn đoán, xem trước JSON sẽ gửi, xuất hoặc xoá nhật ký cục bộ ngay trong **Cài đặt**, và xoá toàn bộ dữ liệu cục bộ của GreenC bất kỳ lúc nào bằng cách gỡ cài đặt ứng dụng (đối với bản Store, Windows tự động dọn dữ liệu trong vùng ảo hoá của gói) và xoá thư mục dữ liệu/Kho dự phòng bạn đã chọn trên ổ đĩa.

### 6. Liên hệ
Nếu có câu hỏi về quyền riêng tư hoặc bảo mật, vui lòng liên hệ: **cungchat.com@gmail.com** hoặc mở issue tại kho mã nguồn công khai **https://github.com/cungchatcom-dotcom/greenC**.

---

## English

### 1. Overview
**GreenC** is an open-source local disk space management utility developed by **CungChat**. All disk scanning, Space Lens analysis, temporary file cleanup, Canary Safety Vault archiving, compaction, and folder relocation operate **100% locally on your device**.

### 2. Data We Do NOT Collect
GreenC does **not** collect, transmit, store on external servers, or sell any of the following:
- Your files, filenames, folder paths, drive contents, or raw error message text.
- Personal information (name, email address, account credentials, or geographic location).
- Device identifiers, user identifiers, cookies, or cross-session tracking IDs.
- Third-party ad networks or tracking SDKs.
- Any diagnostic data unless you **explicitly opt in**.

### 3. Opt-In Anonymous Diagnostics (Standalone Edition Only)
To measure performance and fix bugs, the standalone edition offers optional **anonymous diagnostic counters** (**off by default**; nothing is counted or sent until you explicitly choose **Agree to send** or turn it on in **Settings**):
- **Fields sent** (`POST https://greenc-log.cungchat.com/v1/counters`, at most once every ~20 hours or after a crash):
  - `day`: UTC date (`YYYY-MM-DD`).
  - `version`: GreenC app version (e.g. `2.2.3`).
  - `os`: OS family (`windows10`, `windows11`, `macos`, `linux`).
  - `build`: Windows build number or macOS major version (`active` event only; `0` on Linux).
  - `disk`: system drive capacity bucket (`lt128`, `lt256`, `lt512`, `lt1024`, `ge1024` GiB; `active` event only).
  - `event`: event type (`active`, `start`, `scan`, `screen`, `action`, `crash`).
  - `name`: fixed screen/scan/action identifier; for `crash`, only a GreenC source location (`crates/greenc-.../src/....rs:line`) or `dependency`.
  - `outcome`: result status (`ok`, `partial`, `failed`, `cancelled`, `cached`, `fresh`).
  - `code`: normalized error variant code (e.g. `Io`, `FileLocked`, `Win32:5`, never paths or error messages).
  - `duration`: coarse duration bucket (`lt500ms`, `lt2s`, `lt5s`, `lt15s`, `lt60s`, `lt5m`, `ge5m`).
  - `count`: daily occurrence count.
  - `batch`: random per-batch UUID used solely by the server to deduplicate retries.
- **Server retention:**
  - Daily counter rows are retained for **90 days**.
  - Monthly aggregated summaries are retained for **24 months**.
- **IP address & transport:** HTTPS traffic passes through **Cloudflare** for routing and abuse protection; Cloudflare sees your IP address under Cloudflare's privacy policy. The GreenC diagnostics server does **not** log or store your IP address.
- **Local event log:** The standalone edition keeps a sanitized local event log (no filenames or paths) in the app directory (`diagnostics/log/`, max 5 files × 1 MiB, deleted after 7 days). You can inspect the exact outgoing JSON (**View data to be sent**), export the log, or clear it anytime in **Settings**.
- **Opt out anytime:** Turning diagnostics off in **Settings** immediately deletes all queued counters, any pending batch, and local crash markers, and stops counting and sending.
- **Microsoft Store edition (MSIX):** Diagnostics are **completely disabled at compile time** — no counting, no diagnostic logging, no UI controls, and **zero** diagnostic network requests.

### 4. Local Storage & Actual Network Activity
GreenC stores its settings, scan history, and Vault index in a local SQLite database (`greenc.db`), and stores user-requested Canary Safety Vault backups (ZIP64) on your chosen local drive:
- **Standalone edition (EXE / winget / macOS / Linux):** stored in `%LOCALAPPDATA%\GreenC` (Windows) or the standard OS user data directory, plus your selected data drive.
  - *Update checks:* Makes an HTTPS `GET` request to the GitHub Releases API (`https://api.github.com/repos/cungchatcom-dotcom/greenC/releases/latest`) with `User-Agent: GreenC/<version>` to check whether a newer release is available. If you click **Update now**, it downloads the official release installer from `https://github.com/cungchatcom-dotcom/greenC/releases/download/...`.
  - *Anonymous diagnostics (only if you opt in):* Sends HTTPS `POST` requests to `https://greenc-log.cungchat.com/v1/counters` as described in Section 3.
- **Microsoft Store edition (MSIX):** stored in the package's isolated MSIX AppData location (`%LOCALAPPDATA%\GreenC-Store`, virtualized by Windows) plus your selected data drive. Makes **zero** background or automatic outbound network connections (no GitHub update checks, no diagnostics). Application updates are handled exclusively by Microsoft Store. Clicking external links (such as `https://cungchat.com` or `ms-windows-store://`) delegates to your system web browser or the Microsoft Store app.

### 5. Your Control
You can toggle diagnostics, preview the exact outgoing JSON, export or clear the local log in **Settings**, and remove all local GreenC data at any time by uninstalling the app (for the Microsoft Store MSIX edition, Windows removes the package's isolated virtualized data automatically) and deleting any custom Vault/data folder you configured on your secondary drive.

### 6. Contact
For privacy or security questions, contact **cungchat.com@gmail.com** or open an issue at **https://github.com/cungchatcom-dotcom/greenC**.
