# GreenC Linux Disk Guard (`greenc-disk-guard.sh`)

Script mỏng cho cron / `@reboot` trên Linux, gọi `greenc-cli guard run --json` (dùng chung lõi bảo vệ hạn mức với `greenc-app`).

## Tính năng chính
- **Khoá đơn thực thi (`flock`)**: tránh hai lượt cron chồng chéo nhau.
- **Xoay nhật ký tự động (`<= 1 MiB`)**: tự chuyển `guardian.log` sang `guardian.log.1` khi đạt 1 MiB.
- **Ghi dòng khởi động `@reboot`**: truyền `--reboot` trong dòng `@reboot` để luôn ghi dấu khởi động và kiểm tra ngay cả khi có lỗi.
- **Phòng thủ hai lớp công cụ build**: script giữ kiểm tra tiến trình ở shell (`ps -u $(id -u)`) truyền qua `--extra-busy-tools`, kết hợp với bộ phát hiện `/proc` theo UID trong `greenc-cli` (bao gồm cả `GradleDaemon` rảnh).
- **Thực lợi & cooldown bền vững**: ghi rõ `logical`, `vault`, `net`, `target`; khi không đạt mục tiêu hoặc thực lợi < 5% nhu cầu, `greenc-cli` tự tăng cooldown (`30 phút → 2 giờ → 12 giờ`) trong SQLite để không lặp lại mỗi 15 phút.

## Hướng dẫn cài đặt (thủ công — không tự cài)

1. Biên dịch hoặc sao chép `greenc-cli` vào `~/.local/bin/greenc-cli` và cấp quyền thực thi:
   ```bash
   install -m 755 target/release/greenc-cli ~/.local/bin/greenc-cli
   install -m 755 packaging/linux/greenc-disk-guard.sh ~/.local/bin/greenc-disk-guard.sh
   ```

2. Thiết lập hạn mức cho ổ cần bảo vệ (ví dụ `/home`, ngưỡng cảnh báo 70%, dọn ở 80%, mục tiêu 75%):
   ```bash
   greenc-cli quota set --drive /home --warn 70 --clean 80 --target 75 --auto-clean
   ```
   *(Tuỳ chọn — khi Vault nằm cùng ổ `/home`)*: chỉ bật xoá thẳng cho **đơn vị cache lập trình tái tạo được** (dữ liệu thường vẫn luôn qua Vault):
   ```bash
   greenc-cli guard set-mode --drive /home --mode direct
   ```

3. Kiểm thử khô (không sửa đĩa):
   ```bash
   ~/.local/bin/greenc-disk-guard.sh --dry-run
   greenc-cli guard suggest --json
   ```

4. Thêm vào crontab người dùng (`crontab -e`):
   ```cron
   @reboot /home/<user>/.local/bin/greenc-disk-guard.sh --reboot
   */15 * * * * /home/<user>/.local/bin/greenc-disk-guard.sh
   ```

5. Theo dõi nhật ký:
   ```bash
   tail -f ~/.local/share/greenc/guardian.log
   ```
