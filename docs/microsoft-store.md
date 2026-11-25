# Hướng dẫn đưa GreenC lên Microsoft Store (gói MSIX)

Tài liệu này hướng dẫn từng bước để chủ dự án đóng gói và nộp **GreenC** lên **Microsoft Store** bằng định dạng **MSIX** (Store lo việc ký số và phân phối — không cần chứng thư ký số riêng như đường nộp EXE/MSI).

---

## 1. Đăng ký tài khoản Microsoft Store Developer (cá nhân, miễn phí)

1. Mở trang đăng ký chính thức: **https://storedeveloper.microsoft.com**
2. Đăng nhập bằng **tài khoản Microsoft cá nhân** (Personal Microsoft Account, ví dụ `@outlook.com` / `@hotmail.com` hoặc email cá nhân đã liên kết tài khoản Microsoft; không dùng tài khoản trường học/công ty).
3. Chọn loại tài khoản **Individual developer (Cá nhân)** — miễn phí lệ phí đăng ký cho nhà phát triển cá nhân.
4. Hoàn tất bước **xác minh danh tính**:
   - Chuẩn bị giấy tờ tuỳ thân hợp lệ (CCCD/Hộ chiếu còn hạn) và camera để chụp ảnh khuôn mặt (**selfie**) theo hướng dẫn trên trình duyệt/điện thoại.
   - Chờ duyệt (thường từ vài phút đến vài ngày làm việc).

---

## 2. Giữ tên ứng dụng và lấy 3 giá trị Identity

1. Đăng nhập **Partner Center** → mục **Apps and games** → **New product** → chọn **MSIX or PWA app** (không chọn EXE/MSI).
2. Nhập tên ứng dụng **GreenC** và bấm **Check availability** → **Reserve product name**.
3. Sau khi giữ tên thành công, vào menu bên trái: **Product management** → **Product identity**.
4. Ghi lại đúng **3 giá trị** sau (sao chép nguyên văn, kể cả dấu hoa/thường và khoảng trắng):
   - `Package/Identity/Name` (ví dụ dạng `12345Publisher.GreenC` hoặc `CungChat.GreenC`)
   - `Package/Identity/Publisher` (ví dụ dạng `CN=XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`)
   - `Package/Properties/PublisherDisplayName` (tên hiển thị nhà phát hành đã đăng ký)

---

## 3. Đặt biến kho lưu trữ (Repository Variables) trên GitHub

> **Lưu ý:** Dùng **Variables** (không cần Secrets — 3 giá trị Identity này nằm công khai trong `AppxManifest.xml` của gói MSIX).

1. Trên GitHub, mở kho `greenC` → **Settings** → **Secrets and variables** → **Actions** → thẻ **Variables**.
2. Tạo 3 Repository variables:

| Tên biến | Giá trị từ Partner Center → Product identity |
|---|---|
| `STORE_IDENTITY_NAME` | `Package/Identity/Name` |
| `STORE_IDENTITY_PUBLISHER` | `Package/Identity/Publisher` |
| `STORE_PUBLISHER_DISPLAY_NAME` | `Package/Properties/PublisherDisplayName` |

Workflow cũng nhận các tên `PACKAGEIDENTITYNAME`, `PACKAGEIDENTITYPUBLISHER`, `PACKAGEPROPERTIESPUBLISHERDISPLAYNAME`.

---

## 4. Chạy workflow tạo gói `.msix` và `.msixupload`

1. Mở thẻ **Actions** trên GitHub → chọn workflow **GreenC v2 CI & Release Pipeline**.
2. Bấm **Run workflow** (`workflow_dispatch`):
   - Có thể để trống `msix_version` (tự lấy từ `Cargo.toml` và chuyển `a.b.c` thành `a.b.c.0`).
3. Ngoài ra, mỗi khi có tag `v*` mới **và** biến `STORE_IDENTITY_NAME` đã được đặt, job `build-msix` cũng tự chạy để xuất artifact.
4. Khi job **Build Microsoft Store MSIX Package (x64)** hoàn tất, tải artifact **GreenC-MSIX-Store**:
   - `GreenC-<a.b.c.0>-x64.msixupload` — **tệp dùng để tải lên Partner Center**.
   - `GreenC-<a.b.c.0>-x64.msix` — tệp MSIX gốc (chưa ký; Store sẽ tự ký khi phát hành).
   - `wack-report.xml` (nếu Windows App Certification Kit có sẵn trên runner).

> **Không tự ký chứng thư tự tạo** lên tệp `.msixupload` khi nộp Store — Microsoft Store sẽ ký bằng chứng thư chính thức của Microsoft.

---

## 5. Tạo bản nộp (Submission) trong Partner Center

1. Vào sản phẩm **GreenC** → **Start your submission**.
2. **Pricing and availability**: chọn miễn phí (Free), chọn thị trường phân phối, ngày phát hành.
3. **Properties**:
   - Category: **Utilities & tools** (hoặc **System utilities**).
   - Privacy policy URL: đăng tải nội dung từ [`docs/privacy.md`](privacy.md) lên trang web chính thức (ví dụ `https://greenc.cungchat.com/privacy`) và dán URL vào mục **Privacy policy**.
   - System requirements: Windows 10 version 1903 (build 18362) trở lên, kiến trúc x64. Ghi chú cần **Microsoft Edge WebView2 Runtime** (có sẵn trên Windows 11 và Windows 10 đã cập nhật; máy Windows 10 1903/1909 chưa cập nhật cần cài Evergreen WebView2 Runtime).
4. **Age ratings**: điền bảng câu hỏi IARC theo gợi ý trong [`docs/store-listing/en.md`](store-listing/en.md) / [`docs/store-listing/vi.md`](store-listing/vi.md) (mọi câu hỏi về nội dung nhạy cảm đều chọn **No** → xếp hạng Everyone / 3+).
5. **Packages**: tải lên tệp `GreenC-<a.b.c.0>-x64.msixupload`.
   - Kiểm tra hệ thống xác nhận đúng kiến trúc **x64**, MinVersion **10.0.18362.0**, và không báo lỗi Identity.
6. **Store listings**: thêm ít nhất hai ngôn ngữ **English** (`docs/store-listing/en.md`) và **Vietnamese** (`docs/store-listing/vi.md`), kèm ảnh chụp màn hình từ `docs/screenshots/en/` và `docs/screenshots/`.
7. **Submission options → Notes for certification**: dán đoạn giải trình quyền `runFullTrust` bên dưới.
8. Kiểm tra lại toàn bộ mục đã xanh → bấm **Submit to the Store**.

---

## 6. Lời giải trình `runFullTrust` (Notes for certification)

Sao chép mẫu tiếng Anh sau vào mục **Notes for certification** khi nộp Partner Center:

```text
Justification for runFullTrust restricted capability:

GreenC is a desktop disk-space inspection and cleanup utility built with Rust and Tauri v2 (Win32 desktop application packaged as MSIX).

1. Why runFullTrust is required:
   - GreenC is a native Win32 desktop application (Windows.FullTrustApplication) that analyzes user-selected local drives, calculates folder sizes (Space Lens), detects open file locks via Windows Restart Manager before touching any file, and archives user-approved cache/temp files into a local Canary Safety Vault (ZIP64) with one-click restore.
   - It runs strictly at standard user privilege (Medium IL, no UAC elevation required) and does NOT request unvirtualizedResources, broadFileSystemAccess, or any other restricted capability.

2. Microsoft Store compliance & safety in the Store build (--features store):
   - Self-updating from GitHub Releases is completely disabled; updates are delivered exclusively via Microsoft Store.
   - Login autostart uses the manifest-declared desktop:StartupTask (Enabled="false" by default) and the Windows.ApplicationModel.StartupTask API; it never writes to HKCU\...\Run.
   - Store package data is stored in its own isolated virtualized folder and does not read, migrate, or delete data from any standalone EXE install.
   - System paths (C:\Windows, Program Files, WindowsApps, %LOCALAPPDATA%\Packages) are strictly protected from modification.
   - All processing is 100% local. The Microsoft Store edition makes zero outbound network requests.
```

---

## 7. Lưu ý kiểm thử trước khi nộp (trên máy Windows thật)

Trước khi bấm Submit lần đầu, nên kiểm tra nhanh trên một máy Windows 10/11 thật (sideload bản tự ký thử bằng chứng thư dev cục bộ, KHÔNG nộp bản tự ký):

- Khởi chạy app, kiểm tra màn hình Cài đặt hiển thị **"Cập nhật qua Microsoft Store"** và không có nút tải bộ cài GitHub.
- Bật/tắt **Khởi động cùng hệ điều hành** trong Cài đặt, sau đó thử tắt trong **Windows Settings → Apps → Startup** rồi mở lại GreenC để xác nhận báo trạng thái `DisabledByUser`.
- Thử chạy đồng thời với bản cài EXE (nếu có) để xác nhận thông báo mutex một phiên hoạt động.
- Thử quét Space Lens, dọn dẹp 1 mục nhỏ vào Kho dự phòng (Vault) trên ổ dữ liệu, khôi phục lại và kiểm tra khu chờ (`%USERPROFILE%\.greenc-store-staging`) không nằm trong AppData ảo hoá.
