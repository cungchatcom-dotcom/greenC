# Chính sách quyền riêng tư / Privacy Policy — GreenC

*Cập nhật lần cuối / Last updated: 2026-11-25*

---

## Tiếng Việt

### 1. Nguyên tắc chung
**GreenC** là phần mềm quản lý dung lượng ổ đĩa mã nguồn mở do **Cùng Chat** phát triển. Mọi tính năng quét ổ đĩa, phân tích dung lượng, dọn dẹp tệp tạm, nén lưu trữ (Kho dự phòng / Safety Vault) và di chuyển thư mục đều hoạt động **100% cục bộ trên thiết bị của bạn**.

### 2. Dữ liệu chúng tôi KHÔNG thu thập
GreenC **không** thu thập, lưu trữ trên máy chủ hay chia sẻ với bên thứ ba bất kỳ dữ liệu nào sau đây:
- Nội dung tệp tin, tên tệp, đường dẫn thư mục hoặc cấu trúc ổ đĩa của bạn.
- Thông tin cá nhân (tên, email, tài khoản, vị trí địa lý).
- Dữ liệu đo đạc hành vi người dùng (telemetry, analytics), báo cáo sự cố tự động hay mã định danh thiết bị.
- Quảng cáo hoặc SDK theo dõi của bên thứ ba.

### 3. Dữ liệu lưu cục bộ trên máy của bạn
Để vận hành, GreenC chỉ lưu cơ sở dữ liệu cấu hình và lịch sử cục bộ (SQLite `greenc.db`), cùng các gói sao lưu ZIP64 trong Kho dự phòng tại thư mục ứng dụng cục bộ hoặc ổ dữ liệu bạn chọn:
- **Bản cài đặt độc lập (EXE / winget / macOS / Linux):** lưu trong `%LOCALAPPDATA%\GreenC` (Windows) hoặc thư mục dữ liệu người dùng chuẩn của hệ điều hành, và ổ dữ liệu bạn chọn.
- **Bản Microsoft Store (MSIX):** lưu trong vùng dữ liệu cô lập của gói ứng dụng (`%LOCALAPPDATA%\GreenC-Store` ảo hoá theo chuẩn MSIX) và ổ dữ liệu bạn chọn.

### 4. Kết nối mạng thực tế của ứng dụng
- **Phiên bản Microsoft Store (MSIX):** **Không** tự thực hiện bất kỳ kết nối mạng nền nào. Mọi bản cập nhật đều do Microsoft Store quản lý. Nếu bạn chủ động bấm vào liên kết trang chủ (`https://cungchat.com`) hoặc liên kết mở Microsoft Store (`ms-windows-store://`), hệ điều hành sẽ mở bằng trình duyệt hoặc ứng dụng Microsoft Store mặc định.
- **Phiên bản cài đặt độc lập (GitHub / EXE / winget / macOS / Linux):** Ứng dụng gửi yêu cầu HTTPS `GET` tới GitHub Releases API (`https://api.github.com/repos/cungchatcom-dotcom/greenC/releases/latest`) kèm tiêu đề `User-Agent: GreenC/<version>` để kiểm tra xem có phiên bản mới hay không. Khi bạn chủ động bấm **Cập nhật ngay**, ứng dụng tải bộ cài từ `https://github.com/cungchatcom-dotcom/greenC/releases/download/...`. Yêu cầu này không gửi dữ liệu cá nhân, không gửi danh sách tệp, không gửi cookie hay mã định danh người dùng; địa chỉ IP của bạn chỉ được GitHub xử lý theo chính sách quyền riêng tư của GitHub khi phục vụ kết nối HTTPS thông thường.

### 5. Quyền kiểm soát của bạn
Bạn có thể xoá toàn bộ dữ liệu cục bộ của GreenC bất kỳ lúc nào bằng cách gỡ cài đặt ứng dụng (đối với bản Store, Windows tự động dọn dữ liệu trong vùng ảo hoá của gói) và xoá thư mục dữ liệu/Kho dự phòng bạn đã chọn trên ổ đĩa.

### 6. Liên hệ
Nếu có câu hỏi về quyền riêng tư hoặc bảo mật, vui lòng liên hệ: **cungchat.com@gmail.com** hoặc mở issue tại kho mã nguồn công khai **https://github.com/cungchatcom-dotcom/greenC**.

---

## English

### 1. Overview
**GreenC** is an open-source local disk space management utility developed by **CungChat**. All disk scanning, Space Lens analysis, temporary file cleanup, Canary Safety Vault archiving, compaction, and folder relocation operate **100% locally on your device**.

### 2. Data We Do NOT Collect
GreenC does **not** collect, transmit, store on external servers, or sell any of the following:
- Your files, filenames, folder paths, or drive contents.
- Personal information (name, email address, account credentials, or location).
- Usage telemetry, analytics, automated crash uploads, or device/advertising identifiers.
- Third-party ad networks or tracking SDKs.

### 3. Local-Only Storage
GreenC stores its settings, scan history, and Vault index in a local SQLite database (`greenc.db`), and stores user-requested Canary Safety Vault backups (ZIP64) on your chosen local drive:
- **Standalone edition (EXE / winget / macOS / Linux):** stored in `%LOCALAPPDATA%\GreenC` (Windows) or the standard OS user data directory, plus your selected data drive.
- **Microsoft Store edition (MSIX):** stored in the package's isolated MSIX AppData location (`%LOCALAPPDATA%\GreenC-Store`, virtualized by Windows) plus your selected data drive.

### 4. Actual Network Activity
- **Microsoft Store edition (MSIX):** Makes **zero** background or automatic outbound network connections. Application updates are handled exclusively by Microsoft Store. Clicking external links (such as `https://cungchat.com` or `ms-windows-store://`) delegates to your system web browser or the Microsoft Store app.
- **Standalone edition (GitHub / EXE / winget / macOS / Linux):** Makes an HTTPS `GET` request to the GitHub Releases API (`https://api.github.com/repos/cungchatcom-dotcom/greenC/releases/latest`) with `User-Agent: GreenC/<version>` to check whether a newer release is available. If you click **Update now**, it downloads the official release installer from `https://github.com/cungchatcom-dotcom/greenC/releases/download/...`. No personal information, disk metadata, cookies, or user identifiers are sent; your IP address is handled only by GitHub as part of standard HTTPS transport under GitHub's privacy policy.

### 5. Your Control
You can remove all local GreenC data at any time by uninstalling the app (for the Microsoft Store MSIX edition, Windows removes the package's isolated virtualized data automatically) and deleting any custom Vault/data folder you configured on your secondary drive.

### 6. Contact
For privacy or security questions, contact **cungchat.com@gmail.com** or open an issue at **https://github.com/cungchatcom-dotcom/greenC**.
