# Tasks: GreenC Landing Page (`greenc.cungchat.com`)

- [x] **T1**: Inspect `ui/` Svelte app and capture 5 crisp real screenshots of GreenC v2.0.0 (`dashboard`, `cleanup`, `relocate`, `vault`, `settings`) into `web/public/screenshots/`.
- [x] **T2**: Scaffold `web/` (`package.json`, `vite.config.ts`, `tsconfig.json`, `favicon.svg`).
- [x] **T3**: Implement `web/src/i18n.ts` with simple, friendly, low-tech Vietnamese (default) and English copy.
- [x] **T4**: Implement `web/src/auth.ts` integrating `https://account.cungchat.com` (Google OAuth redirect + `/auth/callback`, Email login/register, `/users/me` verification, download ticket flow).
- [x] **T5**: Implement `web/index.html`, `web/src/styles.css`, and `web/src/main.ts` with Google-style minimalist design, Before/After storage showcase, interactive screenshot gallery + lightbox, FAQ, and auth modal.
- [x] **T6**: Implement `web/server.mjs` with static serving, SPA `/auth/callback` fallback, `/api/health`, `/api/download-ticket` (server-side JWT check against `account.cungchat.com/users/me`), and `/api/download` (single-use 60s ticket streaming `GreenC-Setup-2.0.0.exe`).
- [x] **T7**: Build and launch on `http://localhost:3010`; verify UI, screenshots, and auth-gate behavior.
- [x] **T8**: Run Open Code Review (`ocr`) on all changes and address all 7 findings (high/medium/low).
- [x] **T9**: Deploy to `render_server_santony` (`/home/pc/projects/greenc.cungchat.com`), enable `greenc-web.service`, configure Cloudflare Tunnel & DNS for `greenc.cungchat.com`, and verify `https://greenc.cungchat.com` live.
