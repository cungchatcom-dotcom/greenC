# Feature Specification: GreenC — Nền Tảng Giữ Ổ C: Luôn Thoáng Toàn Diện & Chuyên Sâu

**Feature Branch**: `001-greenc-complete-platform`

**Created**: 2026-11-16

**Status**: Approved for Planning

**Input**: User description: "Đổi tên ứng dụng thành GreenC; dùng Spec Kit viết toàn bộ mô tả tính năng app hiện tại và bổ sung thêm chiều sâu để phần mềm trở nên hoàn thiện, xuất sắc hơn; lập kế hoạch viết lại toàn bộ bằng Rust trong C:\DiskGuard\greenc-v2."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Thiết lập Ổ dữ liệu, Tổng quan Sức khỏe Ổ C: & Dự báo Đầy Đĩa (Priority: P1)

Người dùng mở **GreenC** lần đầu trên máy tính Windows có ổ hệ thống `C:` sắp đầy và một hoặc nhiều ổ đĩa phụ (`D:`, `E:`…). Ứng dụng tự động nhận diện các ổ đĩa đủ điều kiện làm **Ổ lưu dữ liệu**, đề xuất ổ phù hợp nhất, thiết lập số ngày giữ bản dự phòng (mặc định 7 ngày) và đưa người dùng vào màn hình **Tổng quan**. Tại đây, người dùng nắm bắt ngay dung lượng hiện tại, dự báo số ngày còn lại trước khi ổ `C:` đầy theo tốc độ tăng trưởng gần nhất, bản đồ phân bổ không gian trực quan và danh sách các mục có thể giải phóng ngay (cả chế độ tóm tắt đơn giản lẫn bảng chi tiết kèm thao tác sao chép/mở đường dẫn 1 chạm).

**Why this priority**: Đây là điểm chạm đầu tiên quyết định giá trị cốt lõi của sản phẩm: giúp người dùng hiểu ngay tình trạng ổ `C:`, chọn nơi cất dữ liệu an toàn và thấy rõ cơ hội giải phóng dung lượng trong vài giây đầu tiên.

**Independent Test**: Có thể kiểm thử độc lập bằng cách khởi chạy ứng dụng trên cấu hình trắng với danh sách ổ đĩa mẫu (ổ hệ thống, ổ cố định đủ chuẩn, ổ USB, ổ định dạng không hỗ trợ, phân vùng khôi phục), xác nhận hộp thoại lần đầu chọn đúng ổ đề xuất, lưu cấu hình bền vững và hiển thị chính xác thẻ dung lượng, dự báo tốc độ đầy đĩa cũng như bảng đề xuất giải phóng.

**Acceptance Scenarios**:

1. **Given** người dùng mở GreenC lần đầu chưa có cấu hình ổ dữ liệu và máy có ổ `C:` (còn 18 GB) cùng ổ `D:` cố định hỗ trợ liên kết thư mục (còn 600 GB), **When** hộp thoại "Chọn nơi lưu dữ liệu" xuất hiện, **Then** ổ `D:` được gắn nhãn "Đề xuất" và chọn sẵn với thư mục đích `D:\GreenC\`, ổ USB hoặc phân vùng khôi phục bị vô hiệu hóa kèm lý do rõ ràng, và bấm "Để sau" vẫn tự lưu ổ đề xuất làm mặc định.
2. **Given** người dùng đang ở màn hình **Tổng quan**, **When** dung lượng ổ `C:` vượt ngưỡng 90% hoặc tốc độ tăng ròng 7 ngày gần nhất có nguy cơ làm đầy ổ trong dưới 14 ngày, **Then** thanh dung lượng chuyển sang trạng thái cảnh báo đỏ và hiển thị ước tính số ngày còn lại trước khi chạm ngưỡng nguy hiểm.
3. **Given** thẻ "Có thể giải phóng" đang ở chế độ thu gọn 3 cột, **When** người dùng bấm biểu tượng con mắt (Xem chi tiết) hoặc bấm nút sao chép / mở thư mục trên bất kỳ ô đường dẫn nào, **Then** bảng chuyển sang chế độ chi tiết 5 cột (ghi nhớ trạng thái trong phiên), đường dẫn được chép vào bộ nhớ tạm kèm thông báo ngắn 1,2 giây hoặc mở đúng vị trí trong trình quản lý tệp.
4. **Given** máy chỉ có duy nhất ổ hệ thống `C:` đủ điều kiện, **When** người dùng hoàn tất bước khởi đầu, **Then** GreenC vẫn cho phép sử dụng các tính năng theo dõi, dọn dẹp vào Kho dự phòng và nén lạnh tại chỗ, đồng thời ẩn/khóa tác vụ di chuyển sang ổ khác kèm lời giải thích rõ ràng.

---

### User Story 2 - Dọn dẹp Kiểm chứng An toàn, Kho Dự phòng (Vault) & Tự động Khôi phục (Priority: P1)

Người dùng có hàng chục gigabyte bộ đệm, tệp đính kèm trò chuyện cũ (Zalo, DingTalk…), bản cập nhật cũ bị bỏ quên, tệp đổ lỗi bộ nhớ và dữ liệu mồ côi của ứng dụng đã gỡ trên ổ `C:`. Khi bấm **Phân tích** và **Dọn dẹp**, GreenC tự kiểm chứng an toàn trước trên mẫu thử (Canary Probe), bỏ qua mọi tệp đang mở hoặc tệp cốt lõi (cơ sở dữ liệu, cấu hình, thực thi), gom đúng các nhóm liên kết cứng (`HardLink`) và đóng gói tệp cũ vào **Kho dự phòng (Vault)** trên ổ dữ liệu kèm mã kiểm tra toàn vẹn. Trong suốt thời gian giữ `D` ngày, nếu ứng dụng phát sinh lỗi thiếu tệp hoặc dịch vụ dừng bất thường, GreenC **tự động khôi phục nguyên trạng** ngay lập tức và miễn trừ vĩnh viễn tệp đó; người dùng cũng có thể duyệt tìm, khôi phục lẻ từng tệp hoặc hoàn tác cả gói bất cứ lúc nào trước khi gói tự xóa sau `D` ngày.

**Why this priority**: Cơ chế "Dọn nhưng không bao giờ sợ hỏng ứng dụng hay mất nhầm dữ liệu" nhờ kiểm chứng trước + Kho dự phòng + Tự động khôi phục là lợi thế khác biệt lớn nhất của GreenC so với mọi công cụ dọn rác truyền thống.

**Independent Test**: Có thể kiểm thử độc lập bằng cách tạo thư mục ứng dụng mẫu chứa tệp đang mở khóa độc quyền, tệp cơ sở dữ liệu bảo vệ, nhóm tệp liên kết cứng và tệp cũ rảnh rỗi; chạy Phân tích → Dọn dẹp → xác nhận chỉ tệp cũ rảnh được đóng gói vào Kho dự phòng, dung lượng vật lý giảm đúng mức mục tiêu, duyệt khôi phục lẻ 1 tệp hoạt động đúng, giả lập lỗi thiếu tệp kích hoạt Tự động Khôi phục khớp mã kiểm tra toàn vẹn 100%, và gói quá hạn `D` ngày tự động xóa sạch.

**Acceptance Scenarios**:

1. **Given** ứng dụng trò chuyện (như Zalo hoặc DingTalk) đang chạy và giữ khóa đọc/ghi trên một số tệp hiện hành cũng như dùng liên kết cứng cho cùng một tệp phương tiện ở nhiều thư mục, **When** người dùng chạy Phân tích và bấm "Dọn dẹp", **Then** GreenC tính dung lượng vật lý không trùng lặp, chạy thử nghiệm đóng gói–khôi phục mẫu thành công, bỏ qua 100% tệp đang mở hoặc tệp cơ sở dữ liệu/cấu hình, chỉ lưu 1 bản vật lý cho mỗi nhóm liên kết cứng vào Kho dự phòng và xóa đồng bộ các nhánh liên kết để giải phóng dung lượng thật trên đĩa.
2. **Given** quá trình đóng gói vào Kho dự phòng đang diễn ra trên hàng chục nghìn tệp, **When** người dùng quan sát thanh trạng thái phía dưới, **Then** giao diện vẫn mượt mà không đơ, thanh trạng thái hiển thị rõ giai đoạn hiện tại, số tệp đã xử lý, dung lượng, tốc độ xử lý, thời gian dự kiến còn lại (ETA) và phản hồi tức thì khi bấm Tạm dừng, Tiếp tục hoặc Dừng (hủy gói dở dang, giữ nguyên 100% tệp gốc).
3. **Given** một tệp bị ứng dụng khác mở khóa hoặc ghi sửa đổi ngay trong lúc đang đóng gói, hoặc ổ chứa Kho dự phòng còn trống dưới mức dự phòng an toàn (`512 MB`), **When** bước hoàn tất kiểm tra trước khi xóa tệp gốc chạy, **Then** tệp bị sửa/khóa được giữ nguyên trên ổ gốc và nếu ổ Kho dự phòng đầy thì gói dở dang bị hủy sạch mà không xóa bất kỳ tệp gốc nào.
4. **Given** một gói dự phòng đang trong thời hạn theo dõi `D` ngày, **When** hệ thống giám sát phát hiện dịch vụ liên kết bị dừng bất thường hoặc nhật ký ứng dụng xuất hiện lỗi báo thiếu đúng tên tệp đã cất trong gói, **Then** GreenC tự động giải nén trả tệp về vị trí cũ (hoặc lưu bản `.restored_<thời_gian>` nếu vị trí cũ đã có tệp mới khác rỗng), tái tạo toàn bộ nhánh liên kết cứng, phục hồi dấu thời gian sửa đổi, đối chiếu mã toàn vẹn, thêm tệp vào danh sách miễn trừ vĩnh viễn và thông báo rõ lý do cho người dùng.
5. **Given** người dùng chỉ cần lấy lại một tài liệu hoặc bức ảnh duy nhất nằm trong gói dự phòng lớn, **When** người dùng mở chi tiết gói trong trang **Kho dự phòng**, tìm kiếm tên tệp và bấm "Khôi phục tệp đã chọn", **Then** chỉ tệp đó (cùng các liên kết cứng của nó nếu có) được khôi phục về vị trí gốc trong khi phần còn lại của gói vẫn được giữ nguyên.
6. **Given** trên ổ `C:` tồn tại các thư mục dữ liệu mồ côi của ứng dụng đã gỡ cài đặt từ lâu, **When** người dùng chạy Phân tích dọn dẹp, **Then** các thư mục mồ côi này được nhận diện riêng kèm nhãn rõ ràng và khi dọn vẫn đi qua Kho dự phòng `D` ngày để đảm bảo an toàn tuyệt đối.

---

### User Story 3 - Di chuyển Dữ liệu Ứng dụng & Hệ thống Đa Chiến lược sang Ổ Khác (Priority: P1)

Người dùng muốn dời vĩnh viễn các thư mục ngốn nhiều gigabyte trên ổ `C:` sang ổ dữ liệu (`D:\GreenC\…`) mà ứng dụng và hệ điều hành vẫn hoạt động bình thường như chưa hề thay đổi. GreenC tự động quét các mục lớn hơn `200 MB` trên ổ hệ thống và áp dụng đúng cơ chế ưu tiên chuẩn nhất cho từng loại: (1) tính năng Vị trí chuẩn của hệ điều hành cho thư mục cá nhân (`Downloads`, `Documents`, `Pictures`, `Videos`, `Music`, `Desktop`), (2) đổi cấu hình gốc cho bộ đệm công cụ phát triển & AI (`npm`, `pip`, `uv`, `Cargo`, `pnpm`, `Yarn`, `NuGet`, `Gradle`, `HuggingFace`, `Ollama`…), (3) hướng dẫn từng bước cho ứng dụng tự quản lý cơ sở dữ liệu riêng (`Zalo`, kho đĩa ảo `WSL2`/`Docker` kèm thu gọn dung lượng rỗng), và (4) sao chép kiểm tra toàn vẹn + tạo liên kết thư mục trong suốt cho ứng dụng máy tính (`Telegram`, `Discord`, `CapCut`… và mục `Nâng cao`). Mọi mục đã chuyển đều có thể kiểm tra sức khỏe liên kết và bấm **Hoàn tác** trả về ổ `C:` bất cứ lúc nào.

**Why this priority**: Dọn dẹp giải quyết dữ liệu cũ, còn Di chuyển dữ liệu giải phóng triệt để hàng chục đến hàng trăm gigabyte dữ liệu đang hoạt động khỏi ổ `C:` lâu dài.

**Independent Test**: Có thể kiểm thử độc lập từng chiến lược trên môi trường giả lập: xác nhận kiểm tra điều kiện tiên quyết (ứng dụng phải đóng, không trùng OneDrive, ổ đích đủ chỗ trống cộng thêm `512 MB` dự phòng), quy trình sao chép đối chiếu mã toàn vẹn → đổi tên tạm → tạo liên kết → xác minh đích → dọn bản tạm, tự phục hồi nguyên vẹn nếu mất điện/sập giữa chừng khi khởi động lại, và hoàn tác trả dữ liệu về đúng trạng thái ban đầu.

**Acceptance Scenarios**:

1. **Given** thư mục `Downloads` nằm trên ổ `C:` (không nằm trong thư mục đồng bộ OneDrive) và lớn hơn `200 MB`, **When** người dùng bấm "Chuyển" trên trang **Di chuyển dữ liệu**, **Then** toàn bộ tệp được sao chép sang `D:\GreenC\Downloads` có đối chiếu mã toàn vẹn từng tệp, hệ điều hành được cập nhật vị trí thư mục chuẩn sang đường dẫn mới, dữ liệu cũ trên `C:` được dọn sạch (ghi chú rõ nếu còn tệp đang mở chưa xóa), và bấm "Hoàn tác" trả vị trí chuẩn lẫn dữ liệu về lại ổ `C:`.
2. **Given** người dùng chọn chuyển một ứng dụng theo cơ chế liên kết thư mục (như `Telegram Desktop`) trong khi ứng dụng đó đang chạy, **When** hệ thống kiểm tra điều kiện trước khi chuyển, **Then** thao tác bị chặn ngay từ đầu kèm thông báo rõ "Cần đóng Telegram Desktop" thay vì làm hỏng dữ liệu đang mở.
3. **Given** quá trình chuyển thư mục kèm liên kết bị dừng đột ngột (người dùng bấm Dừng, sai lệch mã toàn vẹn khi sao chép, hoặc máy mất nguồn ở bước trung gian `.greenc-moving`), **When** tác vụ hủy hoặc khi GreenC khởi động lại lần kế tiếp, **Then** cơ chế tự phục hồi hoàn tất liên kết nếu đích đã hợp lệ hoặc khôi phục nguyên vẹn thư mục gốc trên ổ `C:` và xóa sạch phần chép dở ở ổ đích — không bao giờ để mất thư mục nguồn.
4. **Given** máy có công cụ phát triển/AI (`npm`, `pip`, `uv`, `Cargo`, `Ollama`, `HuggingFace`…) có bộ đệm trên ổ `C:` vượt ngưỡng hiển thị, **When** người dùng bấm "Chuyển", **Then** công cụ được cập nhật cấu hình trỏ sang `D:\GreenC\Apps\<công-cụ>`, xác minh công cụ đã nhận cấu hình mới thành công rồi mới xóa bộ đệm cũ trên ổ `C:`.
5. **Given** một thư mục đã chuyển bằng liên kết nhưng ổ dữ liệu đích tạm thời bị ngắt kết nối hoặc đường dẫn đích bị đổi tên ngoài ý muốn, **When** người dùng mở GreenC, **Then** mục đó trong danh sách "Đã chuyển" hiển thị cảnh báo mất kết nối đích kèm hướng dẫn/nút trỏ lại đường dẫn đích an toàn.

---

### User Story 4 - Thu gọn Tại chỗ ("Nén Lạnh") & Thu gọn Đĩa ảo Không Đổi Đường dẫn (Priority: P2)

Nhiều thư mục lớn trên ổ `C:` (như bộ công cụ lập trình, thư viện phụ thuộc dự án cũ, dữ liệu bộ nhớ đệm đồ họa hoặc tệp ảnh đĩa ảo máy ảo/container) không thể xóa và cũng không tiện dời sang ổ khác (hoặc máy chỉ có 1 ổ SSD duy nhất là `C:`). GreenC cung cấp chế độ **Thu gọn tại chỗ**: áp dụng nén trong suốt của hệ thống tệp cho các thư mục ít ghi sửa và thu gọn khoảng trống thừa bên trong các tệp đĩa ảo mà không làm thay đổi bất kỳ đường dẫn hay tệp nào đối với ứng dụng.

**Why this priority**: Mở rộng chiều sâu sản phẩm cho nhóm người dùng laptop chỉ có duy nhất một ổ `C:` cũng như các thư mục SDK/công cụ bắt buộc phải nằm trên ổ hệ thống.

**Independent Test**: Có thể kiểm thử độc lập bằng cách chọn một thư mục dữ liệu tĩnh trên ổ `C:`, thực thi Thu gọn tại chỗ, xác nhận dung lượng chiếm dụng thực trên đĩa giảm rõ rệt trong khi đường dẫn, nội dung tệp và mã kiểm tra toàn vẹn khi ứng dụng đọc vẫn giữ nguyên 100%, đồng thời có thể giải nén hoàn tác về trạng thái thường.

**Acceptance Scenarios**:

1. **Given** người dùng có laptop chỉ gồm 1 ổ `C:` hoặc có thư mục SDK/dự án ít thay đổi, **When** người dùng chọn mục đó trong phần **Thu gọn tại chỗ**, **Then** GreenC bỏ qua các tệp đang mở hoặc tệp đã nén sẵn, áp dụng nén trong suốt cấp hệ thống tệp, hiển thị chính xác số gigabyte vật lý tiết kiệm được và cho phép "Giải nén lại" bất cứ lúc nào.
2. **Given** máy có tệp đĩa ảo của môi trường container/Linux con bị phình dung lượng vật lý dù bên trong đã xóa bớt dữ liệu, **When** người dùng chọn tác vụ thu gọn đĩa ảo theo hướng dẫn an toàn (sau khi môi trường ảo đã dừng), **Then** phần dung lượng rỗng chưa trả lại hệ điều hành được thu hồi về cho ổ `C:`.

---

### User Story 5 - Dòng Thời gian Biến động Theo Ngày & Truy vết Tác nhân Ghi Đĩa Thực (Priority: P2)

Khi ổ `C:` đột ngột mất vài gigabyte chỉ sau một buổi làm việc, người dùng xem thẻ **Biến động theo ngày** (biểu đồ cột tương tác 7 / 14 / 30 ngày) và bấm vào cột của ngày bất kỳ để trả lời chính xác 4 câu hỏi: **Ở đâu đầy lên? Tăng hay giảm bao nhiêu? Vì nguyên nhân gì? Do ứng dụng hay dịch vụ hệ thống nào ghi ra?** Từ dòng thủ phạm, người dùng có thể bấm tạo ngay **Quy tắc giới hạn dung lượng** chỉ với 1 thao tác.

**Why this priority**: Giúp người dùng trị tận gốc nguyên nhân làm đầy ổ cứng thay vì phải dọn đi dọn lại trong mù mờ.

**Independent Test**: Có thể kiểm thử độc lập bằng cách phát sinh các sự kiện tạo mới, ghi thêm và xóa tệp trên nhiều thư mục thuộc các nhóm nguyên nhân khác nhau, xác nhận biểu đồ cập nhật đúng mức biến động ròng (`+` / `-`) theo ngày và khi bấm chọn từng cột ngày thì hai bảng chi tiết hiển thị đúng thư mục, nhãn nguyên nhân, ứng dụng/dịch vụ liên quan và cho phép mở hộp thoại tạo quy tắc điền sẵn thông tin.

**Acceptance Scenarios**:

1. **Given** biểu đồ cột đang hiển thị lịch sử 14 ngày gần nhất của ổ `C:`, **When** người dùng bấm vào cột của một ngày cụ thể, **Then** bảng bên trái liệt kê các thư mục biến động lớn nhất trong ngày đó kèm nhãn nguyên nhân phân loại rõ ràng, còn bảng bên phải liệt kê ứng dụng/tiến trình hoặc dịch vụ hệ thống chịu trách nhiệm kèm tổng dung lượng đã ghi.
2. **Given** một ứng dụng hoặc dịch vụ ghi ồ ạt dữ liệu nhật ký hoặc bộ đệm làm tăng mạnh dung lượng trong ngày, **When** người dùng bấm nút "Tạo quy tắc" ngay tại dòng ứng dụng đó, **Then** hộp thoại tạo quy tắc mở ra với tên ứng dụng, tiến trình, dịch vụ và đường dẫn thư mục được điền sẵn chính xác.

---

### User Story 6 - Quy tắc Hạn mức Xoay vòng Tự động & Chạy ngầm Thông minh ở Khay Hệ thống (Priority: P2)

Sau khi người dùng dọn dẹp hoặc thiết lập hạn mức tối đa cho các ứng dụng hay phình dữ liệu (ví dụ giới hạn bộ nhớ phương tiện Zalo ở mức `2,5 GB`, DingTalk ở mức `3,0 GB`), GreenC âm thầm canh gác ở khay hệ thống. Định kỳ kiểm tra nhẹ nhàng, mỗi khi ứng dụng vượt trần cho phép, GreenC tự động đưa các tệp cũ nhất không bị khóa vào Kho dự phòng để kéo dung lượng về ngưỡng an toàn (`85%` hạn mức), đồng thời tự khôi phục nếu phát hiện sự cố và tự xóa gói cũ quá hạn `D` ngày — tất cả không làm gián đoạn công việc đang chạy toàn màn hình hay khi laptop đang tiết kiệm pin.

**Why this priority**: Biến việc giữ ổ `C:` thông thoáng từ thao tác thủ công định kỳ thành cơ chế tự hành bền vững ("Green your C drive forever").

**Independent Test**: Có thể kiểm thử độc lập bằng cách cấu hình một quy tắc hạn mức thấp cho thư mục thử nghiệm, sinh dữ liệu vượt trần, kích hoạt chu trình chạy ngầm và xác nhận dung lượng được kéo về đúng tỷ lệ `85%` hạn mức, tác vụ ngầm tự nhường đường ngay khi người dùng bấm tác vụ thủ công trên giao diện, và chế độ tiết kiệm pin / khẩn cấp hoạt động đúng ngưỡng.

**Acceptance Scenarios**:

1. **Given** một quy tắc đang bật với giới hạn `10 GB` và tỷ lệ mục tiêu `85%`, **When** dung lượng vật lý của thư mục mục tiêu tăng lên `11,2 GB` trong chu trình kiểm tra ngầm, **Then** GreenC tự động đóng gói tối thiểu `2,7 GB` các tệp cũ nhất (đủ tuổi tối thiểu, không bị mở khóa, không thuộc danh sách bảo vệ) vào Kho dự phòng mà không làm gián đoạn ứng dụng đang chạy.
2. **Given** chu trình kiểm tra ngầm đang chạy, **When** người dùng bấm bắt đầu Phân tích, Dọn dẹp hoặc Di chuyển dữ liệu trên giao diện chính, **Then** tác vụ ngầm dừng hợp tác ngay lập tức để nhường toàn bộ quyền ưu tiên cho thao tác của người dùng.
3. **Given** người dùng đóng cửa sổ chính khi đang bật chế độ chạy ngầm hoặc bật tùy chọn "Khởi động cùng Windows", **When** cửa sổ đóng lại hoặc máy khởi động vào phiên người dùng, **Then** GreenC thu gọn xuống biểu tượng khay hệ thống (với thực đơn Mở / Thoát nhanh) và chỉ gửi thông báo khi tự khôi phục gói dữ liệu do sự cố hoặc khi ổ `C:` tụt xuống mức trống khẩn cấp.

---

### User Story 7 - Trải nghiệm Đa ngôn ngữ Tức thì (50 Ngôn ngữ + RTL), Giao diện Sáng/Tối & Dòng lệnh Quản trị (Priority: P3)

Người dùng toàn cầu sử dụng GreenC bằng ngôn ngữ mẹ đẻ trong số 50 ngôn ngữ hỗ trợ (mặc định khớp ngôn ngữ hệ điều hành, hỗ trợ đầy đủ bố cục phải-sang-trái cho tiếng Ả Rập, Do Thái, Ba Tư, Urdu và dấu thập phân theo vùng). Khi đổi ngôn ngữ hoặc chế độ giao diện Sáng / Tối / Theo hệ thống trong **Cài đặt**, toàn bộ giao diện cập nhật tức thì mà không cần khởi động lại ứng dụng. Quản trị viên hoặc người dùng chuyên sâu cũng có thể chạy lệnh kiểm tra trạng thái, xuất/nhập cấu hình quy tắc hoặc chạy thử nghiệm mô phỏng (Sandbox Demo) để kiểm chứng toàn bộ chu trình.

**Why this priority**: Hoàn thiện chất lượng trải nghiệm đẳng cấp quốc tế và khả năng tự động hóa, chẩn đoán cho người dùng nâng cao.

**Independent Test**: Có thể kiểm thử độc lập bằng cách chuyển đổi qua lại giữa tiếng Việt, tiếng Anh và một ngôn ngữ viết từ phải sang trái ngay khi cửa sổ đang mở, xác nhận 100% nhãn văn bản cập nhật tức thì, hướng bố cục đảo chiều đúng chuẩn và các ô số/đường dẫn vẫn giữ định dạng dễ đọc; đồng thời chạy lệnh kiểm tra trạng thái chế độ dòng lệnh xuất kết quả hợp lệ.

**Acceptance Scenarios**:

1. **Given** ứng dụng đang mở bằng tiếng Việt, **When** người dùng chọn một ngôn ngữ khác (bao gồm ngôn ngữ RTL như tiếng Ả Rập) hoặc đổi chế độ giao diện Sáng/Tối trong **Cài đặt**, **Then** toàn bộ thanh điều hướng, tiêu đề, bảng dữ liệu và hướng bố cục cập nhật ngay lập tức mà không cần tắt mở lại ứng dụng.
2. **Given** người dùng muốn dùng thử cơ chế an toàn của GreenC trước khi áp dụng lên dữ liệu thật, **When** người dùng kích hoạt kịch bản mô phỏng trong hộp cát (Sandbox Demo) và thử giả lập lỗi ứng dụng, **Then** toàn bộ vòng đời Tạo dữ liệu mẫu → Vượt hạn mức → Đóng gói vào Kho dự phòng → Tự động Khôi phục diễn ra trực quan trong thư mục hộp cát riêng biệt, không chạm tới dữ liệu thật.
3. **Given** người dùng nâng cấp từ phiên bản cũ (`DiskGuard`, `Green C` hoặc `GreenC` bản trước) hoặc chạy bộ cài đặt mới khi ứng dụng đang mở, **When** bộ cài đặt hoặc ứng dụng mới khởi chạy, **Then** bộ cài nhắc đóng phiên đang chạy qua cơ chế khóa đơn phiên bản, cài đặt không đòi quyền Quản trị viên, và toàn bộ lịch sử, quy tắc, bản ghi di chuyển cùng Kho dự phòng cũ được kế thừa trọn vẹn 100%.

---

### Edge Cases

- **Tệp đang mở hoặc bị sửa đúng lúc đóng gói**: Nếu bất kỳ tiến trình nào giữ khóa đọc/ghi/xóa hoặc dấu thời gian/kích thước tệp thay đổi giữa lúc bắt đầu đọc và lúc hoàn tất gói dự phòng, tệp đó bắt buộc được bỏ qua và giữ nguyên trên ổ gốc.
- **Liên kết thư mục lồng nhau & tệp đám mây giữ chỗ (OneDrive/Cloud Placeholders)**: Bộ quét và bộ đóng gói không bao giờ đi xuyên qua các điểm liên kết lồng bên trong thư mục ứng dụng hay tải ngược các tệp đám mây chỉ lưu trực tuyến về máy, tránh xóa nhầm thư mục tài liệu khác hoặc làm tăng ngược dung lượng ổ `C:`.
- **Xung đột tên tệp khi khôi phục từ Kho dự phòng**: Nếu trong thời gian tệp nằm ở Kho dự phòng mà ứng dụng đã sinh ra một tệp mới khác rỗng tại đúng đường dẫn cũ, hệ thống khôi phục bản cũ thành `<tên_gốc>.restored_<dấu_thời_gian>` để không ghi đè mất dữ liệu mới (trừ khi tệp mới có kích thước 0 byte do lỗi mở tệp thì khôi phục thẳng vào đường dẫn gốc).
- **Ổ chứa Kho dự phòng hoặc ổ đích bị đầy giữa chừng / rút nóng**: Luôn kiểm tra trước và duy trì khoảng trống dự phòng tối thiểu `512 MB`; nếu ổ đích hết chỗ hoặc mất kết nối giữa lúc nén/chép, toàn bộ tệp tạm đích bị dọn sạch và dữ liệu nguồn trên ổ `C:` được bảo toàn nguyên vẹn.
- **Mất điện hoặc tắt cưỡng bức giữa bước chuyển liên kết thư mục**: Khi khởi động lại, GreenC tự rà soát mọi bản ghi đang ở trạng thái trung gian (`moving`) cùng thư mục tạm `.greenc-moving` để tự động hoàn tất nốt hoặc hoàn trả nguyên trạng về ổ `C:`.
- **Tệp siêu lớn (> 4 GB) và tệp đã nén sẵn (video, ảnh, gói nén)**: Hỗ trợ lưu trữ tệp dung lượng lớn không giới hạn kích thước khung 32-bit; tự động nhận diện định dạng đã nén sẵn qua phần mở rộng hoặc chữ ký đầu tệp để lưu thẳng không tốn tài nguyên nén vô ích.
- **Đổi ổ lưu dữ liệu trong Cài đặt sau khi đã dùng lâu ngày**: Tự động di chuyển an toàn toàn bộ các gói đang giữ trong Kho dự phòng sang ổ mới (sao chép, kiểm tra kích thước, cập nhật đường dẫn rồi mới xóa bản cũ), trong khi các ứng dụng đã chuyển bằng liên kết trước đó vẫn giữ nguyên vị trí hoạt động ổn định trên ổ cũ và hiển thị đúng đường dẫn thực tế.

## Requirements *(mandatory)*

### Functional Requirements

#### Quản lý Ổ đĩa, Khởi tạo & Tổng quan

- **FR-001**: Hệ thống PHẢI tự động liệt kê mọi phân vùng ổ đĩa trên máy và đánh giá tính hợp lệ làm **Ổ lưu dữ liệu** (chỉ chấp nhận ổ đĩa cố định cục bộ, hệ thống tệp hỗ trợ liên kết thư mục và tệp lớn, có quyền ghi, không phải phân vùng khôi phục hệ thống), kèm lý do cụ thể cho từng ổ không hợp lệ.
- **FR-002**: Hệ thống PHẢI đề xuất ổ lưu dữ liệu tối ưu là ổ hợp lệ không phải ổ hệ thống có dung lượng trống lớn nhất (tối thiểu `10 GB` trống); nếu không có ổ phụ đạt chuẩn thì mới cho phép chọn ổ có nhiều dung lượng trống nhất hiện có.
- **FR-003**: Ở lần khởi chạy đầu tiên khi chưa cấu hình nơi lưu dữ liệu, hệ thống PHẢI hiển thị hộp thoại chọn ổ lưu dữ liệu và số ngày giữ bản dự phòng (`1–90` ngày, mặc định `7` ngày); nếu người dùng chọn "Để sau", hệ thống vẫn tự động áp dụng ổ đề xuất.
- **FR-004**: Màn hình **Tổng quan** PHẢI hiển thị dung lượng tổng, đã dùng, còn trống của ổ đang chọn (cảnh báo trực quan khi mức dùng `>= 90%`), dự báo số ngày còn lại trước khi đầy ổ dựa trên tốc độ tăng ròng gần nhất, bản đồ phân bổ không gian trực quan và danh sách tổng hợp các mục có thể giải phóng (hỗ trợ chuyển đổi giữa chế độ thu gọn và bảng chi tiết).
- **FR-005**: Mọi đường dẫn hiển thị trong bảng dữ liệu PHẢI được rút gọn gọn gàng khi hẹp chỗ, hiện đường dẫn đầy đủ khi rê chuột, có nút sao chép nhanh vào bộ nhớ tạm (kèm thông báo xác nhận ngắn) và nút mở đúng tệp/thư mục trong trình quản lý tệp của hệ điều hành.

#### Theo dõi Biến động Theo Ngày & Truy vết Nguyên nhân

- **FR-006**: Hệ thống PHẢI ghi nhận ảnh chụp dung lượng ổ đĩa hàng ngày (tổng, đã dùng, còn trống, biến động ròng so với ngày liền trước) và cho phép xem biểu đồ cột tương tác theo khung thời gian `7`, `14` hoặc `30` ngày.
- **FR-007**: Hệ thống PHẢI ghi nhận và phân loại biến động dung lượng theo thư mục và tác nhân ghi đĩa vào tối thiểu 10 nhóm nguyên nhân chuẩn: Phương tiện trò chuyện (`MEDIA_CHAT`), Gói ứng dụng con (`EAPP_BUNDLE`), Bộ nhớ đệm & tạm (`CACHE_TEMP`), Bản cập nhật cũ bị bỏ quên (`OLD_APP_VERSION`), Tệp đổ lỗi bộ nhớ (`CRASH_DUMP`), Nhật ký phình to (`LOG_GROWTH`), Cơ sở dữ liệu (`DB_STORAGE`), Ảnh đĩa ảo Container/Máy ảo (`CONTAINER_VM`), Tệp tải về/Bộ cài lớn (`MEDIA_DOWNLOAD`) và Dữ liệu ứng dụng khác (`APP_DATA`).
- **FR-008**: Hệ thống PHẢI đối chiếu đường dẫn biến động với tiến trình đang mở tệp, tốc độ ghi của tiến trình, bảng dịch vụ hệ thống và chữ ký thư mục ứng dụng để xác định ứng dụng hoặc dịch vụ chịu trách nhiệm, đồng thời hạch toán được cả mức tăng lẫn mức giải phóng dung lượng trong ngày.
- **FR-009**: Khi người dùng bấm chọn bất kỳ cột ngày nào trên biểu đồ, hệ thống PHẢI cập nhật tức thì bảng chi tiết thư mục/nguyên nhân và bảng ứng dụng/dịch vụ của đúng ngày đó, kèm nút tạo nhanh quy tắc hạn mức từ bất kỳ dòng tác nhân nào.

#### Phân tích Thông minh, Dọn dẹp Kiểm chứng & Kho Dự phòng (Vault)

- **FR-010**: Hệ thống PHẢI tính toán dung lượng vật lý thực tế của mọi thư mục mục tiêu bằng cách khử trùng lặp theo định danh tệp vật lý để các tệp dùng liên kết cứng (`HardLink`) không bao giờ bị tính gấp đôi dung lượng.
- **FR-011**: Hệ thống PHẢI có sẵn hồ sơ nhận diện chuyên sâu cho các ứng dụng trò chuyện, làm việc và công cụ phổ biến (bao gồm đọc cấu hình đa ổ đĩa của Zalo, nhận diện thư mục phiên bản Zalo cũ không còn chạy, kho phương tiện liên kết cứng, các gói `EAppFiles`/`resource_cache`/`dumps` của DingTalk, bộ đệm trình duyệt, công cụ lập trình và các thư mục dữ liệu mồ côi của ứng dụng đã gỡ cài đặt).
- **FR-012**: Trước khi đề xuất dọn dẹp tự động cho một ứng dụng, hệ thống PHẢI thực thi kiểm chứng mẫu an toàn 1 chu trình (Canary Probe) trên bản sao thử nghiệm: đóng gói → giải nén khôi phục → kiểm tra khớp mã toàn vẹn và cấu trúc liên kết cứng; nếu thư mục gốc chứa tệp nhạy cảm không đạt kiểm chứng thì tự thu hẹp phạm vi về các thư mục con chỉ chứa bộ đệm/nhật ký an toàn.
- **FR-013**: Hệ thống PHẢI bảo vệ mặc định tất cả các tệp thực thi, thư viện hệ thống, cấu hình, cơ sở dữ liệu đang hoạt động, tệp nhật ký giao dịch cơ sở dữ liệu và tệp khóa, cũng như mọi tệp có tuổi đời nhỏ hơn ngưỡng tối thiểu (mặc định `60` giây).
- **FR-014**: Ngay trước khi đưa vào gói dự phòng và một lần nữa ngay trước khi xóa tệp gốc, hệ thống PHẢI kiểm tra trạng thái mở độc quyền cấp hệ điều hành để đảm bảo không có bất kỳ tiến trình nào đang giữ tệp và tệp không bị thay đổi kích thước hoặc dấu thời gian trong suốt quá trình nén.
- **FR-015**: Khi đóng gói vào **Kho dự phòng**, hệ thống PHẢI tự động lưu thẳng (không nén lại gây tốn CPU) đối với các tệp đã có định dạng nén sẵn, xử lý song song các tệp nhỏ và luồng tệp lớn (> `4 GB`), lưu kèm tệp kê khai (`manifest`) chứa đường dẫn gốc, danh sách toàn bộ nhánh liên kết cứng, kích thước, dấu thời gian và mã băm toàn vẹn của từng tệp.
- **FR-016**: Hệ thống PHẢI kiểm tra tính toàn vẹn của gói dự phòng sau khi tạo xong rồi mới xóa đồng bộ tệp gốc (bao gồm tất cả các đường dẫn nhánh trong cùng nhóm liên kết cứng để thực sự giải phóng dung lượng đĩa), đồng thời luôn duy trì tối thiểu `512 MB` dung lượng trống dự phòng trên ổ chứa Kho dự phòng.
- **FR-017**: Trong trang **Kho dự phòng**, người dùng PHẢI xem được danh sách gói dự phòng (ứng dụng, ngày tạo, thời hạn còn lại, kích thước gốc → sau đóng gói, trạng thái), tìm kiếm và khôi phục chọn lọc từng tệp/thư mục con riêng lẻ, khôi phục toàn bộ gói 1 chạm hoặc xóa vĩnh viễn ngay lập tức.

#### Giám sát An toàn (Safety Watchdog), Tự động Khôi phục & Tự động Xóa

- **FR-018**: Trong suốt thời hạn giữ `D` ngày của mỗi gói ở trạng thái đang theo dõi (`QUARANTINED`), bộ giám sát an toàn PHẢI kiểm tra sức khỏe của dịch vụ hệ thống liên kết và quét các tệp nhật ký lỗi mới cập nhật trong thư mục ứng dụng để phát hiện dấu hiệu báo thiếu tệp thuộc gói dự phòng.
- **FR-019**: Khi phát hiện dấu hiệu mất an toàn (hoặc khi người dùng bấm Khôi phục), hệ thống PHẢI giải nén trả tệp về vị trí gốc (xử lý chống ghi đè nếu đã có tệp mới khác rỗng), phục hồi dấu thời gian truy cập/sửa đổi, tái tạo đầy đủ các nhánh liên kết cứng, xác minh khớp mã băm toàn vẹn 100%, đưa vĩnh viễn đường dẫn đó vào danh sách miễn trừ của quy tắc và xóa gói dự phòng sau khi hoàn tất.
- **FR-020**: Khi gói dự phòng vượt qua thời hạn `D` ngày mà không phát sinh bất kỳ cảnh báo mất an toàn nào, hệ thống PHẢI tự động xóa vĩnh viễn tệp gói nén và tệp kê khai khỏi ổ đĩa, đồng thời lưu lại lịch sử trạng thái đã xóa an toàn.

#### Di chuyển Dữ liệu Đa Chiến lược & Phục hồi Sau Sự cố

- **FR-021**: Hệ thống PHẢI phát hiện các ứng dụng, công cụ và thư mục người dùng trên ổ hệ thống có dung lượng từ `200 MB` trở lên (chưa từng chuyển và không phải liên kết thư mục có sẵn) và sắp xếp theo thứ tự ưu tiên an toàn: (1) Thư mục chuẩn của hệ điều hành, (2) Cấu hình bộ đệm công cụ phát triển/AI, (3) Hướng dẫn di chuyển bằng tính năng nội tại của ứng dụng, (4) Di chuyển kèm liên kết thư mục trong suốt.
- **FR-022**: Với **Thư mục chuẩn của hệ điều hành** (`Downloads`, `Documents`, `Pictures`, `Videos`, `Music`, `Desktop`), hệ thống PHẢI tự động bỏ qua thư mục đang nằm trong vùng đồng bộ đám mây OneDrive, sao chép có kiểm tra toàn vẹn sang `<Ổ_dữ_liệu>\GreenC\<Tên_thư_mục>`, cập nhật vị trí chuẩn với hệ điều hành rồi mới dọn dữ liệu ở vị trí cũ, và hỗ trợ hoàn tác trọn vẹn.
- **FR-023**: Với **Bộ đệm công cụ phát triển & AI** (tối thiểu gồm `npm`, `pip`, `uv`, `Cargo`, `pnpm`, `Yarn`, `NuGet`, `Gradle` và kho mô hình `Ollama` / `HuggingFace` khi công cụ có mặt trên máy), hệ thống PHẢI cập nhật cấu hình công cụ sang `<Ổ_dữ_liệu>\GreenC\Apps\<mã_công_cụ>`, xác nhận công cụ đọc lại đúng đường dẫn mới rồi mới xóa bộ đệm cũ trên ổ `C:`, và hỗ trợ hoàn tác cấu hình.
- **FR-024**: Với **Ứng dụng tự quản lý dữ liệu riêng** (`Zalo`, môi trường đĩa ảo `WSL2` / `Docker Desktop`), hệ thống PHẢI cung cấp hướng dẫn từng bước rõ ràng với đường dẫn đích gợi ý, nút mở nhanh ứng dụng và nút "Kiểm tra lại" để xác nhận dữ liệu thực tế đã nằm ngoài ổ hệ thống.
- **FR-025**: Với **Di chuyển kèm liên kết thư mục** (các ứng dụng tuyển chọn như `Telegram Desktop`, `Discord`, `CapCut`… và nhóm `Nâng cao` cho các thư mục lớn khác trong hồ sơ người dùng, tuyệt đối loại trừ thư mục hệ thống), hệ thống PHẢI chặn thao tác nếu tiến trình ứng dụng đang chạy hoặc ổ đích không đủ `dung_lượng + 512 MB`, thực hiện tuần tự: sao chép đối chiếu mã toàn vẹn từng tệp → đổi tên thư mục gốc sang `.greenc-moving` → tạo liên kết thư mục → xác minh liên kết trỏ đúng đích → xóa bản `.greenc-moving`.
- **FR-026**: Ngay khi khởi động ứng dụng, hệ thống PHẢI tự động kiểm tra mọi bản ghi di chuyển bị gián đoạn giữa chừng (`moving`) để hoàn tất bước dọn tạm nếu liên kết đã hợp lệ hoặc khôi phục lại thư mục gốc trên ổ `C:` nếu quá trình tạo liên kết chưa hoàn chỉnh; đồng thời cảnh báo nếu bất kỳ liên kết đã chuyển nào bị mất kết nối tới ổ đích.
- **FR-027**: Mọi mục đã chuyển theo cơ chế thư mục chuẩn, cấu hình công cụ hoặc liên kết thư mục PHẢI có nút **Hoàn tác** để trả dữ liệu và cấu hình về lại ổ `C:` an toàn sau khi kiểm tra điều kiện ứng dụng đã đóng.

#### Thu gọn Tại chỗ ("Nén Lạnh") & Đĩa ảo

- **FR-028**: Hệ thống PHẢI cung cấp tính năng **Thu gọn tại chỗ** cho phép người dùng chọn các thư mục ứng dụng/dự án ít thay đổi trên ổ `C:` để nén trong suốt cấp hệ thống tệp (bỏ qua tệp đang khóa và tệp đã nén sẵn), báo cáo chính xác dung lượng vật lý thu hồi được và hỗ trợ giải nén hoàn trả bất cứ lúc nào.
- **FR-029**: Hệ thống PHẢI phát hiện các tệp ảnh đĩa ảo cục bộ (`.vhdx`) có dung lượng chiếm dụng lớn trên ổ `C:` và cung cấp quy trình thu gọn khoảng trống rỗng an toàn sau khi xác nhận môi trường ảo liên quan đã dừng hoàn toàn.

#### Quy tắc Chạy ngầm, Điều phối Tác vụ & Vận hành Hệ thống

- **FR-030**: Mọi tác vụ kéo dài (quét ổ đĩa, phân tích ứng dụng, dọn dẹp, di chuyển dữ liệu, đổi vị trí Kho dự phòng, thu gọn tại chỗ) PHẢI chạy ngoài luồng giao diện chính, cung cấp tiến độ trực tiếp và hỗ trợ Tạm dừng, Tiếp tục, Dừng hợp tác an toàn.
- **FR-031**: Chu trình chạy ngầm định kỳ (kiểm tra hạn mức quy tắc và giám sát an toàn Kho dự phòng) PHẢI tự động nhường quyền ngay lập tức cho tác vụ do người dùng khởi phát, tự tạm hoãn phần nén nặng khi máy đang ở chế độ tiết kiệm pin hoặc chạy ứng dụng toàn màn hình, và hỗ trợ chế độ thu hồi an toàn khẩn cấp khi ổ `C:` xuống dưới ngưỡng trống cảnh báo.
- **FR-032**: Hệ thống PHẢI đảm bảo chỉ một phiên bản ứng dụng chạy tại một thời điểm (dùng chung khóa định danh với bộ cài đặt), hỗ trợ thu nhỏ xuống khay hệ thống và tùy chọn tự khởi động cùng phiên đăng nhập của người dùng.
- **FR-033**: Khi người dùng đổi **Ổ lưu dữ liệu** trong **Cài đặt**, hệ thống PHẢI di chuyển toàn bộ các gói dự phòng đang hoạt động sang ổ mới (sao chép, kiểm tra kích thước, cập nhật cơ sở dữ liệu rồi mới xóa bản cũ) mà không làm xáo trộn các ứng dụng đã chuyển bằng liên kết trước đó.
- **FR-034**: Hệ thống PHẢI tự động nhận diện và kế thừa trọn vẹn cơ sở dữ liệu cài đặt, quy tắc hạn mức, lịch sử di chuyển và các gói Kho dự phòng từ các phiên bản trước (`DiskGuard`, `Green C` và `GreenC` bản Python 2.0) ngay lần khởi chạy đầu tiên.
- **FR-035**: Giao diện người dùng PHẢI hỗ trợ đầy đủ 50 ngôn ngữ (bao gồm đảo chiều bố cục phải-sang-trái chuẩn xác cho các ngôn ngữ RTL), áp dụng đổi ngôn ngữ và chế độ màu Sáng / Tối / Theo hệ thống ngay lập tức không cần khởi động lại, và cung cấp chế độ dòng lệnh (CLI) cho phép truy vấn trạng thái dung lượng cũng như xuất/nhập cấu hình quy tắc.

### Key Entities

- **DriveProfile (Hồ sơ Ổ đĩa)**: Đại diện cho một phân vùng trên máy (ký tự ổ, nhãn, định dạng hệ thống tệp, tổng dung lượng, dung lượng trống, cờ ổ hệ thống, trạng thái hợp lệ làm ổ dữ liệu và lý do nếu không hợp lệ).
- **DailySnapshot (Ảnh chụp Dung lượng Ngày)**: Bản ghi dung lượng tổng, đã dùng, còn trống, mức biến động ròng theo từng ngày của mỗi ổ đĩa và cơ sở để tính tốc độ đầy đĩa dự kiến.
- **WriteEvent (Sự kiện Biến động Ghi đĩa)**: Sổ cái ghi nhận đợt tăng/giảm dung lượng theo ngày, đường dẫn thư mục/tệp tiêu biểu, tiến trình, dịch vụ hệ thống và mã phân loại nguyên nhân.
- **AppCleanupProposal (Đề xuất Phân tích Ứng dụng)**: Kết quả phân tích một ứng dụng hoặc thư mục mồ côi gồm trạng thái đang chạy, danh sách đường dẫn mục tiêu, dung lượng vật lý đã khử trùng lặp liên kết cứng, dung lượng có thể thu hồi, hạn mức đề xuất và kết quả kiểm chứng mẫu an toàn (Canary Probe).
- **QuotaRule (Quy tắc Hạn mức Xoay vòng)**: Cấu hình canh gác cho một ứng dụng/dịch vụ gồm danh sách đường dẫn, trần dung lượng tối đa, tỷ lệ thu hồi mục tiêu (`85%`), số ngày giữ dự phòng (`1–90`), tuổi tệp tối thiểu, mẫu loại trừ và danh sách tệp được miễn trừ vĩnh viễn sau khôi phục.
- **VaultArchive & VaultFileItem (Gói Dự phòng & Mục Tệp trong Kho)**: Đại diện cho một gói dữ liệu đang cách ly an toàn (trạng thái `QUARANTINED`, `ROLLED_BACK`, `PURGED`, ngày tạo, ngày hết hạn, lý do trạng thái) và danh mục chi tiết từng tệp bên trong (đường dẫn gốc, nhóm nhánh liên kết cứng, kích thước, mã toàn vẹn, dấu thời gian).
- **RelocationRecord (Bản ghi Di chuyển Dữ liệu)**: Lưu vết lịch sử và trạng thái (`moving`, `done`, `failed`, `undone`) của từng thư mục/công cụ đã chuyển khỏi ổ `C:`, chiến lược sử dụng, đường dẫn nguồn, đường dẫn đích, danh sách tiến trình liên quan và trạng thái sức khỏe liên kết.
- **InPlaceCompactionRecord (Bản ghi Thu gọn Tại chỗ)**: Theo dõi các thư mục hoặc tệp đĩa ảo đã được nén lạnh trong suốt trên ổ `C:`, dung lượng trước/sau thu gọn và thời điểm thực hiện.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Người dùng hoàn tất bước chọn ổ lưu dữ liệu lần đầu và nhìn thấy danh sách đề xuất dung lượng có thể giải phóng trên ổ `C:` trong dưới `15 giây` trên ổ SSD tiêu chuẩn (quét phân tích 500.000 tệp trên ổ hệ thống hoàn tất trong dưới `5 giây`).
- **SC-002**: `100%` chiến dịch dọn dẹp, tự động khôi phục, di chuyển dữ liệu và hoàn tác đạt mức **không mất mát dữ liệu** (mọi tệp khôi phục hoặc di chuyển khớp `100%` mã kiểm tra toàn vẹn và giữ nguyên đầy đủ liên kết cứng trên toàn bộ bộ kiểm thử tự động và kịch bản ngắt giữa chừng).
- **SC-003**: `0%` tệp đang được tiến trình khác mở đọc/ghi hoặc tệp cơ sở dữ liệu/cấu hình cốt lõi của ứng dụng bị xóa hoặc làm hỏng khi ứng dụng mục tiêu đang chạy trực tiếp.
- **SC-004**: Tốc độ đóng gói vào Kho dự phòng trên tập dữ liệu hỗn hợp (ví dụ `12.000` tệp bộ đệm ứng dụng khoảng `1,5 GB`) đạt tối thiểu `150 MB/giây` trên ổ NVMe (nhanh gấp tối thiểu `5–10 lần` so với bản Python hiện hành).
- **SC-005**: Khi chạy nền ở khay hệ thống ở trạng thái rảnh, ứng dụng tiêu thụ dưới `25 MB` bộ nhớ RAM, dưới `0,2%` CPU trung bình và kích thước bộ cài đặt độc lập gọn nhẹ dưới `15 MB`.
- **SC-006**: Mọi thao tác trên giao diện duy trì độ phản hồi tức thì (khung hình mượt `60 FPS`, không treo giao diện khi chạy tác vụ nền), và thao tác bấm **Dừng** hủy tác vụ an toàn trong dưới `200 mili-giây`.
- **SC-007**: `100%` chuỗi giao diện trên cả `50` ngôn ngữ (bao gồm 4 ngôn ngữ RTL) hiển thị đầy đủ bản dịch, khớp biến định dạng và đổi ngôn ngữ trực tiếp trong dưới `100 mili-giây` mà không cần khởi động lại ứng dụng.
- **SC-008**: `100%` người dùng nâng cấp từ phiên bản `Green C` / `GreenC` hiện hành giữ nguyên toàn bộ quy tắc hạn mức, lịch sử biến động, các gói Kho dự phòng đang giữ và các liên kết thư mục đã chuyển mà không cần cấu hình lại thủ công.

## Assumptions

- Môi trường mục tiêu là máy tính để bàn và máy tính xách tay chạy hệ điều hành Windows 10 (phiên bản 1903 trở lên) hoặc Windows 11 64-bit, cài đặt theo phạm vi người dùng hiện tại không bắt buộc quyền Quản trị viên.
- Các tính năng nâng cao đòi hỏi quyền cấp hệ thống (như đọc trực tiếp bảng chỉ mục phân vùng tốc độ siêu cao hoặc thu gọn đĩa ảo hệ thống) sẽ tự động dùng cơ chế chuẩn cấp người dùng làm mặc định và chỉ xin nâng quyền khi người dùng chủ động bật/chọn tác vụ đó.
- Đồng bộ hóa đám mây trực tuyến và quản lý tập trung qua máy chủ từ xa nằm ngoài phạm vi của phiên bản này; toàn bộ dữ liệu và nhật ký của GreenC được lưu cục bộ 100% trên máy người dùng (không thu thập dữ liệu ngầm ra bên ngoài).
- Đối với các bộ đệm công cụ phát triển thuần túy (`npm`, `pip`, `uv`, `Cargo`…), do công cụ có thể tự tải lại gói khi cần, việc chuyển vị trí sẽ đổi cấu hình sang ổ mới rồi xóa bộ đệm cũ thay vì chép toàn bộ tệp bộ đệm cũ sang ổ mới.
