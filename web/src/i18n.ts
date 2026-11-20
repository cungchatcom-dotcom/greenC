export type Lang = 'vi' | 'en';

export interface TranslationDict {
  meta: {
    title: string;
    description: string;
  };
  nav: {
    features: string;
    results: string;
    screenshots: string;
    faq: string;
    signIn: string;
    signOut: string;
    download: string;
  };
  hero: {
    titleLine1: string;
    titleLine2: string;
    subtitle: string;
    ctaPrimary: string;
    ctaDownloading: string;
    ctaSecondary: string;
    pillars: [string, string, string];
    windowTitle: string;
  };
  impact: {
    eyebrow: string;
    title: string;
    subtitle: string;
    stats: Array<{
      value: string;
      label: string;
      desc: string;
    }>;
    beforeLabel: string;
    afterLabel: string;
    toggleBefore: string;
    toggleAfter: string;
    driveTitle: string;
    beforeStatus: string;
    afterStatus: string;
    beforeSummary: string;
    afterSummary: string;
    beforeHint: string;
    afterHint: string;
    breakdownTitle: string;
    breakdownItems: Array<{
      name: string;
      saved: string;
      detail: string;
    }>;
  };
  tour: {
    eyebrow: string;
    title: string;
    subtitle: string;
    zoomHint: string;
    tabs: Array<{
      id: string;
      label: string;
      title: string;
      desc: string;
      image: string;
      alt: string;
    }>;
  };
  safety: {
    eyebrow: string;
    title: string;
    subtitle: string;
    cards: Array<{
      icon: 'shield' | 'lock' | 'heart';
      title: string;
      desc: string;
    }>;
  };
  testimonials: {
    eyebrow: string;
    title: string;
    subtitle: string;
    items: Array<{
      quote: string;
      name: string;
      role: string;
      initials: string;
      tone: 'green' | 'blue' | 'amber';
    }>;
  };
  faq: {
    eyebrow: string;
    title: string;
    items: Array<{
      q: string;
      a: string;
    }>;
  };
  ctaBottom: {
    title: string;
    subtitle: string;
    button: string;
    meta: string;
  };
  footer: {
    madeBy: string;
    termsLink: string;
    githubLink: string;
    accountLink: string;
    version: string;
  };
  terms: {
    backHome: string;
    title: string;
    updated: string;
    intro: string;
    sections: Array<{
      heading: string;
      body: string[];
    }>;
  };
  auth: {
    modalTitle: string;
    modalSubtitle: string;
    googleBtn: string;
    orDivider: string;
    tabLogin: string;
    tabRegister: string;
    emailLabel: string;
    emailPlaceholder: string;
    passwordLabel: string;
    passwordPlaceholder: string;
    submitLogin: string;
    submitRegister: string;
    submitting: string;
    registerHint: string;
    loggedInAs: string;
    downloadNow: string;
    downloadStarted: string;
    downloadReady: string;
    errInvalidCredentials: string;
    errGeneric: string;
    errDownload: string;
    close: string;
  };
}

export const DICT: Record<Lang, TranslationDict> = {
  vi: {
    meta: {
      title: 'GreenC — Đặt hạn mức một lần, ổ C luôn xanh mỗi ngày',
      description:
        'Bạn chỉ cần đặt hạn mức tối đa cho ổ đĩa. GreenC tự động phân tích sâu, dọn sạch file rác, nén lưu vào kho an toàn và bảo vệ ổ C của bạn mỗi ngày.',
    },
    nav: {
      features: 'Cách hoạt động',
      results: 'Hiệu quả thực tế',
      screenshots: 'Giao diện thật',
      faq: 'Hỏi đáp',
      signIn: 'Đăng nhập',
      signOut: 'Đăng xuất',
      download: 'Tải miễn phí',
    },
    hero: {
      titleLine1: 'Ổ C luôn xanh.',
      titleLine2: 'Không cần canh dọn.',
      subtitle:
        'Chỉ cần đặt hạn mức tối đa cho ổ đĩa. Phần còn lại để GreenC lo mỗi ngày.',
      ctaPrimary: 'Tải miễn phí cho Windows',
      ctaDownloading: 'Đang chuẩn bị tải về...',
      ctaSecondary: 'Xem giao diện thật',
      pillars: [
        'Tự động phân tích sâu và dọn dẹp file rác',
        'Nén và lưu trữ vào kho an toàn',
        'Tự động bảo vệ ổ C của bạn mỗi ngày',
      ],
      windowTitle: 'GreenC trên Windows 11',
    },
    impact: {
      eyebrow: 'Hiệu quả thấy rõ',
      title: 'Nhẹ máy ngay sau lần đầu chạy',
      subtitle:
        'Cài một lần, máy tính của bạn tự giữ sạch mỗi ngày.',
      stats: [
        {
          value: '4,5×',
          label: 'Dọn nhanh hơn hẳn',
          desc: 'Hơn 10.000 file rác (1,4 GB) được dọn gọn chỉ trong khoảng 1 phút rưỡi, bạn vẫn làm việc bình thường.',
        },
        {
          value: '7 ngày',
          label: 'Kho an toàn giữ file',
          desc: 'File dọn xong chưa mất ngay — lỡ cần lại, bấm một cái là khôi phục nguyên vẹn.',
        },
        {
          value: '5,6 MB',
          label: 'Tải và cài siêu nhanh',
          desc: 'File cài đặt nhỏ gọn, bấm đúp là xong ngay trong 10 giây, không hỏi rắc rối.',
        },
        {
          value: '< 15 MB',
          label: 'Tốn cực ít RAM',
          desc: 'Chạy ngầm chỉ tốn bằng 1/10 một thẻ Chrome — bật cả ngày máy vẫn mượt như chưa cài gì.',
        },
      ],
      beforeLabel: 'Trước khi cài GreenC',
      afterLabel: 'Sau khi bật GreenC',
      toggleBefore: 'Trước khi dọn',
      toggleAfter: 'Sau khi dọn',
      driveTitle: 'Ổ đĩa C: (256 GB)',
      beforeStatus: 'Đầy 94% · Chỉ còn trống 15 GB',
      afterStatus: 'Chỉ dùng 63% · Trống tới 94 GB',
      beforeSummary: 'Ổ C đỏ lòm — mở file nặng là đứng hình, Windows suốt ngày báo hết dung lượng.',
      afterSummary: 'Trống thêm 79 GB ngay lập tức — máy chạy thoáng, mọi ảnh và tài liệu vẫn mở như cũ.',
      beforeHint: 'Bấm "Sau khi dọn" để xem ổ C thay đổi thế nào',
      afterHint: 'File đã dọn vẫn được nén cất trong Kho an toàn suốt 7 ngày',
      breakdownTitle: '79 GB trống thêm lấy từ đâu?',
      breakdownItems: [
        {
          name: 'Thư mục Tải về, Tài liệu & Hình ảnh',
          saved: '+ 40,2 GB',
          detail: 'Chuyển gọn sang ổ D — bấm mở trên máy vẫn y như cũ, chẳng cần đổi thói quen.',
        },
        {
          name: 'Ảnh & file cũ từ Zalo PC',
          saved: '+ 17,7 GB',
          detail: 'Dọn phần file tạm phình to theo tháng năm, giữ nguyên tin nhắn của bạn.',
        },
        {
          name: 'File rác từ trình duyệt & Windows',
          saved: '+ 21,1 GB',
          detail: 'Quét sạch bộ nhớ đệm thừa của Chrome, Edge và các file tạm cũ.',
        },
      ],
    },
    tour: {
      eyebrow: 'Nhìn tận mắt',
      title: 'Mở lên là hiểu, bấm là chạy',
      subtitle:
        'Ảnh chụp thẳng từ GreenC v2.0 — bấm từng mục để xem chi tiết.',
      zoomHint: 'Bấm vào ảnh để phóng to',
      tabs: [
        {
          id: 'dashboard',
          label: 'Tổng quan',
          title: 'Thấy ngay thứ gì đang ăn hết ổ C',
          desc: 'Một màn hình cho bạn biết ổ C còn trống bao nhiêu, mỗi ngày tăng thêm bao nhiêu và nên dọn chỗ nào trước.',
          image: '/screenshots/dashboard.png',
          alt: 'Ảnh chụp màn hình Tổng quan của ứng dụng GreenC',
        },
        {
          id: 'cleanup',
          label: 'Dọn dẹp',
          title: 'Tự động phân tích sâu và dọn sạch file rác',
          desc: 'Tự tìm file thừa từ Zalo, trình duyệt và Windows. Bạn thấy rõ từng mục và dung lượng lấy lại được trước khi dọn.',
          image: '/screenshots/cleanup.png',
          alt: 'Ảnh chụp màn hình Dọn dẹp của ứng dụng GreenC',
        },
        {
          id: 'relocate',
          label: 'Chuyển sang ổ D',
          title: 'Dời thư mục nặng sang ổ D nhẹ nhàng',
          desc: 'Đưa Tải về, Tài liệu, Hình ảnh hay file Zalo sang ổ D chỉ với một cú bấm. Đổi ý lúc nào cũng hoàn tác lại được.',
          image: '/screenshots/relocate.png',
          alt: 'Ảnh chụp màn hình Di chuyển dữ liệu sang ổ D của GreenC',
        },
        {
          id: 'vault',
          label: 'Kho an toàn',
          title: 'Nén và lưu trữ an toàn suốt 7 ngày',
          desc: 'Dọn xong chưa xóa mất ngay — GreenC nén gọn cất vào Kho an toàn. Lỡ cần lại file cũ, bấm một cái là về chỗ cũ.',
          image: '/screenshots/vault.png',
          alt: 'Ảnh chụp màn hình Kho an toàn 7 ngày của GreenC',
        },
        {
          id: 'settings',
          label: 'Cài đặt',
          title: 'Đặt hạn mức một lần, yên tâm cả năm',
          desc: 'Chọn hạn mức tối đa cho ổ C, chọn ổ D làm nơi cất kho dự phòng và để GreenC tự động canh máy mỗi ngày.',
          image: '/screenshots/settings.png',
          alt: 'Ảnh chụp màn hình Cài đặt của ứng dụng GreenC',
        },
      ],
    },
    safety: {
      eyebrow: 'Cách GreenC chăm sóc máy bạn',
      title: 'Đặt hạn mức một lần, GreenC lo hết',
      subtitle: 'Chọn ngưỡng tối đa cho ổ đĩa — từ đó về sau, GreenC tự lo trọn gói cho bạn.',
      cards: [
        {
          icon: 'shield',
          title: 'Tự động phân tích sâu và dọn dẹp file rác',
          desc: 'GreenC tự soi kỹ xem ứng dụng nào đang làm phình ổ C rồi dọn sạch phần rác thừa, không đụng tới tài liệu quan trọng của bạn.',
        },
        {
          icon: 'lock',
          title: 'Nén và lưu trữ vào kho an toàn',
          desc: 'Trước khi giải phóng ổ C, mọi file đều được nén gọn cất riêng vào Kho an toàn trong 7 ngày — lỡ cần lại, lấy ra trong một cú bấm.',
        },
        {
          icon: 'heart',
          title: 'Tự động bảo vệ ổ C của bạn mỗi ngày',
          desc: 'Chạy ngầm nhẹ nhàng ngay trên máy bạn, giữ riêng tư tuyệt đối. Hễ ổ C chạm ngưỡng đã đặt, GreenC tự ra tay trước khi máy kịp chậm.',
        },
      ],
    },
    testimonials: {
      eyebrow: 'Người dùng nói gì',
      title: 'Người dùng nói gì về GreenC',
      subtitle: 'Chia sẻ thực tế từ những người từng đau đầu vì ổ C báo đỏ.',
      items: [
        {
          quote:
            'Máy kế toán của mình ngày nào cũng nhận cả trăm file hóa đơn với ảnh Zalo, ổ C đỏ suốt mà không dám xóa bừa vì sợ mất chứng từ. Cài GreenC xong, đặt hạn mức 1 lần là máy tự dọn gọn, có kho an toàn 7 ngày nên yên tâm hẳn.',
          name: 'Chị Minh Hằng',
          role: 'Kế toán trưởng · Công ty TNHH Nam Việt',
          initials: 'MH',
          tone: 'green',
        },
        {
          quote:
            'Trước cứ vài tuần mình lại phải nhờ IT công ty qua dọn máy giúp. Giờ dùng GreenC chuyển hết thư mục Tải về sang ổ D chỉ bằng một cú bấm, máy nhẹ tênh mà mở file vẫn y như cũ.',
          name: 'Anh Quốc Bảo',
          role: 'Trưởng phòng Kinh doanh · Minh Phát Logistics',
          initials: 'QB',
          tone: 'blue',
        },
        {
          quote:
            'Mình làm nội dung nên hay tải nhiều ảnh, video về máy. Thích nhất là GreenC chạy ngầm siêu nhẹ, không làm giật máy lúc làm việc, mà ổ C lúc nào cũng trống vài chục GB.',
          name: 'Chị Thu Thảo',
          role: 'Thiết kế & Nội dung · Studio Mộc',
          initials: 'TT',
          tone: 'amber',
        },
      ],
    },
    faq: {
      eyebrow: 'Hỏi đáp',
      title: 'Mọi người hay hỏi',
      items: [
        {
          q: 'GreenC có miễn phí thật không?',
          a: 'Hoàn toàn miễn phí. Không quảng cáo, không cài kèm phần mềm lạ. Bạn chỉ cần đăng nhập tài khoản Cùng Chat (hoặc bấm Đăng nhập bằng Google) là tải về dùng ngay.',
        },
        {
          q: 'Tại sao phải đăng nhập tài khoản Cùng Chat mới tải được?',
          a: 'Để đảm bảo bạn luôn tải đúng bản cài sạch, chính chủ từ Cùng Chat và giúp tụi mình hỗ trợ bạn tốt hơn khi cần. Tạo tài khoản cũng hoàn toàn miễn phí.',
        },
        {
          q: 'Lỡ tay dọn nhầm file đang cần thì sao?',
          a: 'Đừng lo. GreenC không xóa mất ngay mà nén cất vào Kho an toàn trên ổ D trong 7 ngày. Bạn chỉ cần mở mục Kho dự phòng, tìm lại và bấm Khôi phục là xong.',
        },
        {
          q: 'Tôi không rành máy tính có tự cài và dùng được không?',
          a: 'Rất dễ. File cài chỉ 5,6 MB — bấm đúp chuột là cài xong trong khoảng 10 giây. Mở app lên bạn chỉ cần chọn hạn mức cho ổ đĩa, mọi việc còn lại GreenC tự làm hết.',
        },
      ],
    },
    ctaBottom: {
      title: 'Đặt hạn mức một lần, ổ C xanh mãi mãi',
      subtitle: 'Tải GreenC miễn phí ngay hôm nay để không bao giờ phải đau đầu vì ổ C báo đỏ nữa.',
      button: 'Tải miễn phí cho Windows',
      meta: 'Phiên bản 2.0.0 · Windows 10 / 11 · Chỉ 5,6 MB',
    },
    footer: {
      madeBy: 'GreenC — Sản phẩm của Cùng Chat',
      termsLink: 'Điều kiện & Điều khoản',
      githubLink: 'Mã nguồn mở (GitHub)',
      accountLink: 'Tài khoản Cùng Chat',
      version: 'Phiên bản 2.0.0',
    },
    terms: {
      backHome: '← Quay lại trang chủ GreenC',
      title: 'Điều kiện & Điều khoản sử dụng — GreenC',
      updated: 'Cập nhật lần cuối: 20/11/2026',
      intro:
        'Bằng việc tải xuống và sử dụng phần mềm GreenC do Cùng Chat (cungchat.com) phát triển, bạn đồng ý tuân thủ các điều kiện và điều khoản dưới đây. Vui lòng đọc kỹ để hiểu rõ quyền lợi và trách nhiệm của mình.',
      sections: [
        {
          heading: '1. Chấp nhận điều khoản',
          body: [
            'Khi tải, cài đặt hoặc sử dụng GreenC, bạn xác nhận đã đọc, hiểu và đồng ý với các điều khoản này. Nếu không đồng ý, vui lòng ngừng sử dụng phần mềm và gỡ cài đặt khỏi thiết bị của bạn.',
          ],
        },
        {
          heading: '2. Mục đích sử dụng & Phần mềm miễn phí mã nguồn mở',
          body: [
            'GreenC được cung cấp miễn phí cho người dùng Windows nhằm hỗ trợ phân tích dung lượng, dọn dẹp file rác, nén lưu dự phòng vào Kho an toàn và di chuyển dữ liệu sang ổ đĩa khác.',
            'Mã nguồn dự án được công khai tại GitHub (github.com/cungchatcom-dotcom/greenC). Người dùng cam kết không sử dụng phần mềm vào mục đích vi phạm pháp luật hoặc gây hại đến hệ thống của người khác.',
          ],
        },
        {
          heading: '3. Tài khoản Cùng Chat và bảo mật',
          body: [
            'Việc tải bộ cài đặt chính thức trên greenc.cungchat.com yêu cầu đăng nhập bằng tài khoản Cùng Chat (account.cungchat.com) miễn phí để xác thực nguồn tải chính chủ.',
            'Bạn chịu trách nhiệm bảo mật thông tin đăng nhập tài khoản Cùng Chat của mình và thông báo ngay cho chúng tôi nếu phát hiện truy cập trái phép.',
          ],
        },
        {
          heading: '4. Quyền riêng tư & Dữ liệu trên máy tính',
          body: [
            'Toàn bộ quá trình quét, nén Kho an toàn và dọn dẹp của GreenC diễn ra 100% cục bộ trên máy tính của bạn. GreenC không thu thập, đọc hay tải lên đám mây bất kỳ tài liệu, hình ảnh hay tệp tin cá nhân nào của bạn.',
          ],
        },
        {
          heading: '5. Quyền và nghĩa vụ của Cùng Chat',
          body: [
            'Cùng Chat nỗ lực duy trì phần mềm ổn định, an toàn và phát hành bản cập nhật cải tiến theo thời gian.',
            'Chúng tôi có quyền cập nhật, nâng cấp hoặc điều chỉnh tính năng của GreenC và trang tải về mà không cần thông báo trước.',
          ],
        },
        {
          heading: '6. Trách nhiệm của người dùng & Giới hạn trách nhiệm',
          body: [
            'GreenC tích hợp sẵn Kho an toàn lưu bản sao dự phòng (mặc định 7 ngày) nhằm giảm thiểu rủi ro khi dọn dẹp. Tuy nhiên, người dùng vẫn cần chủ động kiểm tra và sao lưu các dữ liệu tối quan trọng của mình.',
            'Phần mềm được cung cấp theo hiện trạng ("as-is"). Cùng Chat không chịu trách nhiệm đối với các thiệt hại gián tiếp hoặc mất mát dữ liệu phát sinh ngoài phạm vi kiểm soát hợp lý của phần mềm.',
          ],
        },
        {
          heading: '7. Thay đổi điều khoản',
          body: [
            'Cùng Chat có thể điều chỉnh Điều kiện & Điều khoản này theo thời gian. Phiên bản mới nhất luôn được đăng tải tại greenc.cungchat.com/terms và có hiệu lực kể từ ngày công bố.',
          ],
        },
        {
          heading: '8. Liên hệ',
          body: [
            'Nếu bạn có bất kỳ câu hỏi hoặc góp ý nào về GreenC và Điều khoản sử dụng, vui lòng liên hệ:',
            'Website: https://cungchat.com · Trang sản phẩm: https://greenc.cungchat.com',
            'Email hỗ trợ: cungchat.com@gmail.com · Mã nguồn mở: https://github.com/cungchatcom-dotcom/greenC',
          ],
        },
      ],
    },
    auth: {
      modalTitle: 'Đăng nhập để tải GreenC miễn phí',
      modalSubtitle: 'Dùng tài khoản Cùng Chat của bạn (hoặc bấm tiếp tục với Google cho nhanh).',
      googleBtn: 'Tiếp tục với Google',
      orDivider: 'hoặc dùng email',
      tabLogin: 'Đăng nhập',
      tabRegister: 'Tạo tài khoản mới',
      emailLabel: 'Email của bạn',
      emailPlaceholder: 'tenban@email.com',
      passwordLabel: 'Mật khẩu',
      passwordPlaceholder: 'Nhập mật khẩu (ít nhất 6 ký tự)',
      submitLogin: 'Đăng nhập & Tải về',
      submitRegister: 'Tạo tài khoản & Tải về',
      submitting: 'Đang kiểm tra...',
      registerHint: 'Tạo miễn phí trong vài giây, dùng chung cho mọi sản phẩm Cùng Chat.',
      loggedInAs: 'Đã đăng nhập:',
      downloadNow: 'Tải GreenC-Setup-2.0.0.exe (5,6 MB)',
      downloadStarted: 'Đang tải GreenC-Setup-2.0.0.exe về máy của bạn!',
      downloadReady: 'Bạn đã đăng nhập tài khoản Cùng Chat. Bấm nút bên dưới để tải ngay.',
      errInvalidCredentials: 'Email hoặc mật khẩu chưa đúng. Bạn kiểm tra lại nhé.',
      errGeneric: 'Kết nối đang hơi trục trặc. Bạn thử lại sau vài giây nhé.',
      errDownload: 'Phiên đăng nhập đã hết hạn. Bạn đăng nhập lại để tải nhé.',
      close: 'Đóng',
    },
  },
  en: {
    meta: {
      title: 'GreenC — Set your drive limit once. Stay green every day.',
      description:
        'Just set your maximum drive limit. GreenC automatically analyzes and clears junk files, compresses them into a safety vault, and protects your C: drive every day.',
    },
    nav: {
      features: 'How it works',
      results: 'Real results',
      screenshots: 'Inside the app',
      faq: 'FAQ',
      signIn: 'Sign in',
      signOut: 'Sign out',
      download: 'Download free',
    },
    hero: {
      titleLine1: 'Keep drive C: green.',
      titleLine2: 'Without lifting a finger.',
      subtitle:
        'Just set your maximum drive limit once. GreenC takes care of the rest every single day.',
      ctaPrimary: 'Download free for Windows',
      ctaDownloading: 'Preparing your download...',
      ctaSecondary: 'See it in action',
      pillars: [
        'Deep-analyzes and cleans junk automatically',
        'Compresses and stores in a Safety Vault',
        'Protects your C: drive every single day',
      ],
      windowTitle: 'GreenC on Windows 11',
    },
    impact: {
      eyebrow: 'Results you can feel',
      title: 'A lighter PC from day one',
      subtitle:
        'Set it up once and let GreenC keep your drive tidy every day.',
      stats: [
        {
          value: '4.5×',
          label: 'Faster cleanup',
          desc: 'Clears over 10,000 junk files (1.4 GB) in about 90 seconds while you keep working.',
        },
        {
          value: '7 days',
          label: 'Safety Vault backup',
          desc: 'Cleaned files aren’t gone right away — bring anything back in a single click.',
        },
        {
          value: '5.6 MB',
          label: 'Quick 10-second setup',
          desc: 'A tiny download that installs with a simple double-click — no complicated prompts.',
        },
        {
          value: '< 15 MB',
          label: 'Tiny RAM footprint',
          desc: 'Uses about 1/10th the memory of a single Chrome tab — runs all day without slowing you down.',
        },
      ],
      beforeLabel: 'Before GreenC',
      afterLabel: 'With GreenC running',
      toggleBefore: 'Before',
      toggleAfter: 'After',
      driveTitle: 'Local Disk (C:) · 256 GB',
      beforeStatus: '94% full · Only 15 GB free',
      afterStatus: '63% used · 94 GB free',
      beforeSummary: 'Drive C: in the red — apps stall and Windows keeps nagging about low storage.',
      afterSummary: '79 GB freed right away — your PC feels fresh and every file opens just as before.',
      beforeHint: 'Click "After" to see what changes',
      afterHint: 'Everything cleaned stays compressed in your Safety Vault for 7 days',
      breakdownTitle: 'Where did the extra 79 GB come from?',
      breakdownItems: [
        {
          name: 'Downloads, Documents & Pictures',
          saved: '+ 40.2 GB',
          detail: 'Moved neatly to drive D: — everything still opens from the exact same place.',
        },
        {
          name: 'Old Zalo PC photos & chat files',
          saved: '+ 17.7 GB',
          detail: 'Clears years of hidden media clutter without touching your messages.',
        },
        {
          name: 'Browser & Windows junk',
          saved: '+ 21.1 GB',
          detail: 'Sweeps away leftover cache from Chrome, Edge, and old temporary files.',
        },
      ],
    },
    tour: {
      eyebrow: 'Inside GreenC',
      title: 'Simple enough for anyone',
      subtitle:
        'Real screenshots from GreenC v2.0 — click each tab to explore.',
      zoomHint: 'Click any screenshot to zoom in',
      tabs: [
        {
          id: 'dashboard',
          label: 'Overview',
          title: 'Spot what’s filling up drive C: in seconds',
          desc: 'See how much space is left, how fast it’s growing each day, and what can be cleared safely right now.',
          image: '/screenshots/dashboard.png',
          alt: 'GreenC Overview screen',
        },
        {
          id: 'cleanup',
          label: 'Cleanup',
          title: 'Deep-scan and clear junk automatically',
          desc: 'Finds hidden clutter from Zalo, browsers, and Windows — showing you exactly how much space you’ll get back.',
          image: '/screenshots/cleanup.png',
          alt: 'GreenC Cleanup screen',
        },
        {
          id: 'relocate',
          label: 'Move to D:',
          title: 'Shift heavy folders to drive D: safely',
          desc: 'Move Downloads, Documents, Pictures, or Zalo files to drive D: in one click — and undo anytime you want.',
          image: '/screenshots/relocate.png',
          alt: 'GreenC Move to D: screen',
        },
        {
          id: 'vault',
          label: 'Safety Vault',
          title: 'Compressed & kept safe for 7 days',
          desc: 'Nothing vanishes immediately. GreenC compresses cleaned files into your Safety Vault so you can restore anything in one click.',
          image: '/screenshots/vault.png',
          alt: 'GreenC 7-day Safety Vault screen',
        },
        {
          id: 'settings',
          label: 'Settings',
          title: 'Set your limit once, relax all year',
          desc: 'Pick your maximum C: drive limit, choose where to keep your Safety Vault, and let GreenC guard your drive daily.',
          image: '/screenshots/settings.png',
          alt: 'GreenC Settings screen',
        },
      ],
    },
    safety: {
      eyebrow: 'How GreenC looks after your PC',
      title: 'Just set your maximum drive limit',
      subtitle: 'Pick your limit once — GreenC takes care of everything else automatically.',
      cards: [
        {
          icon: 'shield',
          title: 'Deep-analyzes and cleans junk automatically',
          desc: 'GreenC pinpoints what’s bloating your C: drive and sweeps away clutter without ever touching your important files.',
        },
        {
          icon: 'lock',
          title: 'Compresses and stores in a Safety Vault',
          desc: 'Before freeing up space, cleaned files are compressed into a 7-day Safety Vault so you can bring anything back in one click.',
        },
        {
          icon: 'heart',
          title: 'Protects your C: drive every single day',
          desc: 'Runs quietly and privately on your PC. Whenever drive C: nears your limit, GreenC steps in before your computer slows down.',
        },
      ],
    },
    testimonials: {
      eyebrow: 'What users say',
      title: 'What people say about GreenC',
      subtitle: 'Real stories from people who used to struggle with a full C: drive.',
      items: [
        {
          quote:
            'My work laptop receives hundreds of invoices and chat photos every day. I was always scared to delete files by hand. With GreenC, I set my limit once and the 7-day Safety Vault gives me total peace of mind.',
          name: 'Minh Hang',
          role: 'Chief Accountant · Nam Viet Co., Ltd.',
          initials: 'MH',
          tone: 'green',
        },
        {
          quote:
            'I used to call IT every few weeks just to free up space. Now GreenC moved my Downloads folder to drive D: in one click — my PC feels brand new and everything opens right where I expect it.',
          name: 'Quoc Bao',
          role: 'Sales Manager · Minh Phat Logistics',
          initials: 'QB',
          tone: 'blue',
        },
        {
          quote:
            'I download tons of photos and video clips for work. What I love most is how quietly GreenC runs in the background while keeping dozens of gigabytes free on my C: drive.',
          name: 'Thu Thao',
          role: 'Content & Design · Moc Studio',
          initials: 'TT',
          tone: 'amber',
        },
      ],
    },
    faq: {
      eyebrow: 'FAQ',
      title: 'Common questions',
      items: [
        {
          q: 'Is GreenC really free?',
          a: 'Yes, 100% free — no ads and no bundled extras. Just sign in with a free CungChat account (or Google) to download and start using it.',
        },
        {
          q: 'Why do I need a CungChat account to download?',
          a: 'It makes sure you always get the clean, official installer straight from CungChat and helps us support you better. Creating an account is completely free.',
        },
        {
          q: 'What if something I still need gets cleaned?',
          a: 'No worries. GreenC never deletes files permanently right away — it compresses them into your 7-day Safety Vault on drive D: so you can restore them in one click.',
        },
        {
          q: 'I’m not tech-savvy — is it easy to set up?',
          a: 'Super easy. The download is just 5.6 MB and installs in about 10 seconds. Pick your drive limit once, and GreenC takes care of the rest.',
        },
      ],
    },
    ctaBottom: {
      title: 'Set it once, stay green forever',
      subtitle: 'Download GreenC for free today and never worry about a red C: drive again.',
      button: 'Download free for Windows',
      meta: 'Version 2.0.0 · Windows 10 / 11 · Only 5.6 MB',
    },
    footer: {
      madeBy: 'GreenC — Made by CungChat',
      termsLink: 'Terms of Service',
      githubLink: 'Open Source (GitHub)',
      accountLink: 'CungChat Account',
      version: 'Version 2.0.0',
    },
    terms: {
      backHome: '← Back to GreenC home',
      title: 'Terms of Service — GreenC',
      updated: 'Last updated: November 20, 2026',
      intro:
        'By downloading and using GreenC developed by CungChat (cungchat.com), you agree to the terms below. Please read them carefully to understand your rights and responsibilities.',
      sections: [
        {
          heading: '1. Acceptance of Terms',
          body: [
            'By downloading, installing, or using GreenC, you confirm that you have read, understood, and agreed to these Terms. If you do not agree, please stop using and uninstall the software.',
          ],
        },
        {
          heading: '2. Intended Use & Open-Source Software',
          body: [
            'GreenC is provided free of charge for Windows users to analyze drive space, clean temporary junk, store backups in a Safety Vault, and move folders to another drive.',
            'The project source code is publicly available on GitHub (github.com/cungchatcom-dotcom/greenC). You agree not to misuse the software for unlawful or harmful activities.',
          ],
        },
        {
          heading: '3. CungChat Account & Security',
          body: [
            'Downloading the official installer from greenc.cungchat.com requires signing in with a free CungChat account (account.cungchat.com) to ensure official distribution.',
            'You are responsible for keeping your account credentials secure and notifying us promptly of any unauthorized access.',
          ],
        },
        {
          heading: '4. Privacy & Local-Only Processing',
          body: [
            'All scanning, Safety Vault compression, and cleanup run 100% locally on your computer. GreenC never collects, reads, or uploads your personal files or photos.',
          ],
        },
        {
          heading: '5. CungChat Rights & Updates',
          body: [
            'CungChat strives to keep GreenC reliable and safe, and may release updates or feature improvements without prior notice.',
          ],
        },
        {
          heading: '6. User Responsibility & Limitation of Liability',
          body: [
            'While GreenC includes a 7-day Safety Vault backup to minimize risk, users remain responsible for maintaining backups of critical personal or business data.',
            'The software is provided "as-is". CungChat is not liable for indirect damages or data loss beyond reasonable control.',
          ],
        },
        {
          heading: '7. Changes to Terms',
          body: [
            'CungChat may update these Terms over time. The latest version is always published at greenc.cungchat.com/terms and takes effect upon posting.',
          ],
        },
        {
          heading: '8. Contact',
          body: [
            'If you have questions about GreenC or these Terms, please reach out:',
            'Website: https://cungchat.com · Product: https://greenc.cungchat.com',
            'Support email: cungchat.com@gmail.com · GitHub: https://github.com/cungchatcom-dotcom/greenC',
          ],
        },
      ],
    },
    auth: {
      modalTitle: 'Sign in to download GreenC free',
      modalSubtitle: 'Use your CungChat account (or continue quickly with Google).',
      googleBtn: 'Continue with Google',
      orDivider: 'or use email',
      tabLogin: 'Sign in',
      tabRegister: 'Create account',
      emailLabel: 'Your email',
      emailPlaceholder: 'you@example.com',
      passwordLabel: 'Password',
      passwordPlaceholder: 'Enter password (at least 6 characters)',
      submitLogin: 'Sign in & Download',
      submitRegister: 'Create account & Download',
      submitting: 'Please wait...',
      registerHint: 'Free account in seconds — works across all CungChat apps.',
      loggedInAs: 'Signed in as:',
      downloadNow: 'Download GreenC-Setup-2.0.0.exe (5.6 MB)',
      downloadStarted: 'Your download of GreenC-Setup-2.0.0.exe has started!',
      downloadReady: 'You are signed in with CungChat. Click below to start downloading.',
      errInvalidCredentials: 'Incorrect email or password. Please check and try again.',
      errGeneric: 'Something went wrong while connecting. Please try again in a moment.',
      errDownload: 'Your session expired. Please sign in again to download.',
      close: 'Close',
    },
  },
};
