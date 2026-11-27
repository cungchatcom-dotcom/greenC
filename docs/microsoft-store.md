# Hướng dẫn đưa GreenC lên Microsoft Store (gói MSIX bundle x64 + Arm64)

Tài liệu này hướng dẫn từng bước để chủ dự án đóng gói và nộp **GreenC** lên **Microsoft Store** bằng định dạng **MSIX bundle** (`.msixbundle` đóng gói trong `.msixupload`, gồm cả bản **x64** và **Arm64 gốc**) — Store lo việc ký số và phân phối nên không cần chứng thư ký số riêng như đường nộp EXE/MSI.

> **Vì sao cần bản Arm64 gốc (AArch64):**
> - Trên **Windows 11 trên Arm**, gói Arm64 gốc chạy trực tiếp không qua lớp giả lập x64.
> - Trên **Windows 10 trên Arm**, hệ điều hành chỉ giả lập x86 nên gói x64 đơn lẻ không thể chạy được.
> - **Partner Center** khuyến nghị nộp kèm gói Arm64. GreenC không chứa mã AArch32 nên việc loại bỏ AArch32 không ảnh hưởng đến gói phát hành.

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

## 4. Chạy workflow tạo gói `.msixbundle` và `.msixupload` (x64 + Arm64)

1. Mở thẻ **Actions** trên GitHub → chọn workflow **GreenC v2 CI & Release Pipeline**.
2. Bấm **Run workflow** (`workflow_dispatch`):
   - **Quy tắc phiên bản:** Mọi bản nộp mới lên Partner Center phải có **số phiên bản mới lớn hơn** bản đã nộp trước đó (ví dụ **`2.2.2.0` đã được dùng**, nên lần nộp kế tiếp phải tăng số phiên bản, chẳng hạn `2.2.3.0`).
   - **Chữ số thứ 4 luôn giữ là `0`** (`a.b.c.0` — Microsoft Store dành chữ số thứ 4 cho hệ thống Store quản lý).
   - Có thể để trống `msix_version` để lấy từ `Cargo.toml` (tự chuyển `a.b.c` thành `a.b.c.0`), hoặc nhập trực tiếp `msix_version` dạng `a.b.c` / `a.b.c.0` khi cần đóng gói bản thử/bản nộp mới.
3. Ngoài ra, workflow Store cũng tự chạy trên các nhánh `feat/microsoft-store`, `feat/store-arm64`, và mỗi khi có tag `v*` mới **và** biến `STORE_IDENTITY_NAME` (hoặc `PACKAGEIDENTITYNAME`) đã được đặt.
4. Quy trình đóng gói gồm 3 job nối tiếp nhau (bất kỳ kiến trúc nào lỗi sẽ làm hỏng toàn bộ workflow và **không** xuất tệp nộp Store):
   - **Build Microsoft Store MSIX Package (`x64` trên `windows-latest`, `arm64` trên `windows-11-arm`)**: biên dịch bản `--features store` gốc cho từng kiến trúc, chạy kiểm thử `--features store` trên Arm64, kiểm tra PE header (`0x8664` cho x64, `0xAA64` cho Arm64), kiểm thử cài đặt thực tế (smoke test), và tải lên 2 artifact riêng biệt theo kiến trúc **`GreenC-MSIX-x64`** và **`GreenC-MSIX-arm64`** (chỉ chứa tệp `.msix` đơn lẻ để chẩn đoán — **không dùng trực tiếp để nộp Partner Center**).
   - **Bundle Microsoft Store MSIX (`x64 + arm64`)**: kiểm tra đúng 1 gói `x64` và 1 gói `arm64` có cùng `Identity` (`Name`, `Publisher`, `Version`), gộp thành `.msixbundle`, giải nén kiểm tra lại `AppxBundleManifest.xml`, rồi nén thành `.msixupload` và tải lên artifact **`GreenC-MSIX-Store`**.
   - **Smoke-Test Store Bundle (`x64` và `arm64`)**: cài thử chính tệp `.msixbundle` trên cả hai runner `windows-latest` và `windows-11-arm`, xác nhận Windows tự chọn đúng kiến trúc (`x64` / `arm64`) và vượt qua bài kiểm thử dọn dẹp + khôi phục Kho dự phòng (Vault).
5. Khi toàn bộ các job trên đã xanh, tải artifact **`GreenC-MSIX-Store`**:
   - `GreenC-<a.b.c.0>.msixupload` — **tệp duy nhất dùng để tải lên Partner Center** (file ZIP bên trong chỉ chứa đúng `GreenC-<a.b.c.0>.msixbundle`).
   - `GreenC-<a.b.c.0>.msixbundle` — gói bundle gốc chưa ký chứa đồng thời `x64` và `arm64` (Store sẽ tự ký khi phát hành).

> **Không tự ký chứng thư tự tạo** lên tệp `.msixbundle` / `.msixupload` khi nộp Store — Microsoft Store sẽ ký bằng chứng thư chính thức của Microsoft.
>
> **Cảnh báo không thể hoàn tác (bundle → gói đơn):** Một khi đã phát hành bản dạng **bundle** (`.msixbundle` trong `.msixupload`) lên Microsoft Store, **mọi bản nộp kế tiếp về sau bắt buộc phải tiếp tục là bundle** — Partner Center **không cho phép** quay lại nộp tệp `.msix` đơn kiến trúc.

---

## 5. Tạo bản nộp (Submission) trong Partner Center

1. Vào sản phẩm **GreenC** → **Start your submission** (hoặc **Update** nếu đã có bản phát hành trước đó).
2. **Pricing and availability**: chọn miễn phí (Free), chọn thị trường phân phối, ngày phát hành.
3. **Properties**:
   - Category: **Utilities & tools** (hoặc **System utilities**).
   - Privacy policy URL: đăng tải nội dung từ [`docs/privacy.md`](privacy.md) lên trang web chính thức (ví dụ `https://greenc.cungchat.com/privacy`) và dán URL vào mục **Privacy policy**.
   - System requirements: Windows 10 version 1903 (build 18362) trở lên hoặc Windows 11, hỗ trợ cả **x64** và **Arm64** gốc. Ghi chú cần **Microsoft Edge WebView2 Runtime** (có sẵn trên Windows 11 và Windows 10 đã cập nhật; máy Windows 10 1903/1909 chưa cập nhật cần cài Evergreen WebView2 Runtime).
4. **Age ratings**: điền bảng câu hỏi IARC theo gợi ý trong [`docs/store-listing/en.md`](store-listing/en.md) / [`docs/store-listing/vi.md`](store-listing/vi.md) (mọi câu hỏi về nội dung nhạy cảm đều chọn **No** → xếp hạng Everyone / 3+).
5. **Packages**: tải lên tệp `GreenC-<a.b.c.0>.msixupload` từ artifact **`GreenC-MSIX-Store`** (với phiên bản mới lớn hơn bản đã nộp trước đó, ví dụ `> 2.2.2.0`, và chữ số thứ 4 là `0`).
   - Kiểm tra Partner Center nhận diện đủ **cả hai gói kiến trúc x64 và Arm64** bên trong bundle, MinVersion **10.0.18362.0**, và không báo lỗi Identity hay trùng phiên bản.
   - Gỡ bỏ gói `.msix` x64 đơn cũ (nếu còn nằm trong danh sách Packages của bản nộp trước) khi chuyển sang bundle; lưu ý sau khi bundle được phát hành thì mọi bản nộp sau đều phải là bundle.
6. **Store listings**: thêm ít nhất hai ngôn ngữ **English** (`docs/store-listing/en.md`) và **Vietnamese** (`docs/store-listing/vi.md`), kèm ảnh chụp màn hình từ `docs/store-listing/screenshots/en/` và `docs/store-listing/screenshots/vi/`.
7. **Submission options → Notes for certification**: dán đoạn giải trình quyền `runFullTrust` bên dưới.
8. Kiểm tra lại toàn bộ mục đã xanh → bấm **Submit to the Store**.

---

## 6. Lời giải trình `runFullTrust` (Notes for certification)

Sao chép mẫu tiếng Anh sau vào mục **Notes for certification** khi nộp Partner Center:

```text
GreenC is a classic Win32 desktop application (Rust + Tauri/WebView2) packaged as MSIX. It helps users see what fills their drives and safely clean temporary and cache files.

Why runFullTrust is needed:
1. The app's entry point is a Win32 executable (EntryPoint="Windows.FullTrustApplication"). It cannot run inside an AppContainer.
2. Scanning drives and measuring folder sizes requires standard Win32 file access to locations the user chooses, including cache folders of other desktop apps.
3. Before cleaning, the app uses the Windows Restart Manager API to detect files in use, so it does not remove files that are open in other programs.
4. Cleaned cache files can be archived first into a local "Vault" (ZIP) so the user can restore them. Optional NTFS compression (WOF/CompactOS) also needs Win32 APIs.
5. The app has a system tray icon and an optional startup task. The startup task is declared in the manifest and disabled by default.

How it is used:
- The app runs with standard user rights. It never requests administrator elevation, and it installs no drivers or services.
- runFullTrust is the only restricted capability declared. broadFileSystemAccess and unvirtualizedResources are not used.
- Cleaning is started by the user, or by automatic cleaning rules that the user configures. Windows system folders, Program Files, WindowsApps and AppData\Local\Packages are protected.
- Updates are delivered only through Microsoft Store. The self-updater is compiled out of this edition, and the app does not download or run installers.
- No account, no ads, no telemetry. The app's own code does not send user data over the network.

Helper process: MSIX redirects newly created AppData files into the package's private storage. To restore a file that belongs to another app to its original AppData location, GreenC starts its own command-line helper (greenc-cli.exe from this package, copied to a private temporary folder in the user profile) outside the package context. The helper only performs the restore or undo requested by the user, verifies the result by SHA-256, and exits. The temporary folder is then deleted.

How to test: open the app > Clean up > select a browser cache > Clean up with "Archive to safe Vault" enabled > open Vault > click Restore.
```

---

## 7. Lưu ý kiểm thử trước khi nộp (trên máy Windows thật)

Trước khi bấm Submit lần đầu, nên kiểm tra nhanh trên một máy Windows 10/11 thật (sideload bản tự ký thử bằng chứng thư dev cục bộ, KHÔNG nộp bản tự ký):

- Khởi chạy app, kiểm tra màn hình Cài đặt hiển thị **"Cập nhật qua Microsoft Store"** và không có nút tải bộ cài GitHub.
- Bật/tắt **Khởi động cùng hệ điều hành** trong Cài đặt, sau đó thử tắt trong **Windows Settings → Apps → Startup** rồi mở lại GreenC để xác nhận báo trạng thái `DisabledByUser`.
- Thử chạy đồng thời với bản cài EXE (nếu có) để xác nhận thông báo mutex một phiên hoạt động.
- Thử quét Space Lens, dọn dẹp 1 mục nhỏ vào Kho dự phòng (Vault) trên ổ dữ liệu, khôi phục lại và kiểm tra khu chờ (`%USERPROFILE%\.greenc-store-staging`) không nằm trong AppData ảo hoá.
