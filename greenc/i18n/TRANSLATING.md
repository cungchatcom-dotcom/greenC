# Translating Green C:\

Source language: Vietnamese. Every string in `messages.json` is a Vietnamese UI string from a
Windows desktop app that keeps the system drive C: free (cleans old app files into a compressed
"Vault" and relocates app data to another drive). Each locale file is
`locales/<code>.json` = `{ "<Vietnamese source>": "<translation>", ... }` with **every** key
from `messages.json`, in the same order.

## Rules
- Keep every `{placeholder}` exactly (same names, same format spec such as `{pct:.0f}`), no new ones. Reorder them freely to fit grammar.
- Keep HTML tags/entities (`<b>`, `<br>`, `<a href="{url}">`, `&nbsp;`), `\n`, `→`, `·`, `—`, `…`, `%` as in the source.
- Keep a leading/trailing space if the source has one (e.g. `" ngày"` is a spin-box suffix: `" days"`).
- Never translate: `Green C:\`, `C:`, `Zalo`, `DingTalk`, `Telegram`, `npm`, `pip`, `OneDrive`, `Windows`, `Explorer`, `SHA-256`, file extensions (`.db`, `.log`, …), `NTFS`, `HardLink` inside technical parentheses, `Rule`, `Rollback` inside technical text may be translated naturally.
- Concise, neutral, professional desktop-UI tone, like Microsoft Windows 11 in that language. Buttons are short imperative verbs. No exclamation marks, no emoji.
- Use the platform's established terms for: drive (ổ / ổ đĩa), folder (thư mục), file (file / tệp), settings (Cài đặt), restart, undo (Hoàn tác), cache (bộ đệm), link / junction (liên kết), hard link (liên kết cứng).

## Glossary (Vietnamese → meaning)
- Kho dự phòng → the Vault: folder holding compressed backup packages of cleaned files (keep it a noun like "Vault"/"backup store"; be consistent).
- Gói dự phòng / gói nén → a backup package (one .zip in the Vault).
- Giữ bản dự phòng … ngày → keep backups for … days before permanent deletion.
- Dọn dẹp → Clean up. Phân tích → Analyze. Làm mới → Refresh. Chuyển → Move. Chuyển dữ liệu → Move data / Relocate data.
- Giải phóng → Free up (space). Đang dùng → In use (size used). Giới hạn → Limit (size cap).
- Quy tắc → rule (background quota rule). Chạy nền → background.
- Đạt / Không đạt → Passed / Failed (safety check). Kiểm tra → Check.
- Ổ lưu dữ liệu / ổ dữ liệu → the data drive chosen by the user.
- Vị trí → Location (path column). Đích → Destination.
- Hoàn tác → Undo. Khôi phục → Restore. Tạm dừng / Tiếp tục / Dừng → Pause / Resume / Stop.
- "{letter}" in `<b>{letter}</b>&nbsp;&nbsp;{label}` is a drive letter; usually keep the template as is.
- "Tác giả" → Author.
