# Implementation Plan: GreenC Landing Page (`greenc.cungchat.com`)

## Architecture Overview

```
Browser (greenc.cungchat.com / localhost:3010)
  ├── Static Landing Page (Vite build: HTML + CSS + TS + Real PNG/WebP screenshots)
  ├── Auth calls → https://account.cungchat.com
  │     ├── GET  /auth/google?redirect_uri=<origin>/auth/callback
  │     ├── POST /auth/login
  │     ├── POST /auth/register
  │     └── GET  /users/me
  └── Download gate → Node.js Server (web/server.mjs)
        ├── GET  /api/health        → { ok, version, fileSize }
        ├── POST /api/download-ticket (Bearer JWT)
        │     └── Server verifies JWT via ACCOUNT_API_BASE/users/me
        │     └── Returns { ticket, filename, size } (single-use, 60s TTL)
        └── GET  /api/download?ticket=<ticket>
              └── Consumes ticket & streams releases/GreenC-Setup-2.0.0.exe
```

## Directory Structure (`C:\DiskGuard\web\`)

```
web/
├── package.json
├── vite.config.ts
├── tsconfig.json
├── index.html
├── server.mjs               # Production & local auth-gated download + static server
├── public/
│   ├── favicon.svg
│   └── screenshots/         # Real captured screenshots of GreenC v2.0.0
│       ├── dashboard.png
│       ├── cleanup.png
│       ├── relocate.png
│       ├── vault.png
│       └── settings.png
├── src/
│   ├── main.ts              # Entry, event wiring, lightbox, before/after toggle
│   ├── styles.css           # Google-style minimalist design system
│   ├── i18n.ts              # Low-tech friendly VI (default) & EN dictionary
│   └── auth.ts              # cungchat-account client (Google OAuth, Email, /users/me, download ticket)
└── releases/                # (gitignored) holds GreenC-Setup-2.0.0.exe outside public root
```

## Real Screenshot Capture Plan
1. Inspect `ui/` (Svelte 5 app) and/or run `ui/` with realistic mock/demo data in headless Playwright/Chromium (or capture directly from `dist/GreenC/GreenC.exe`) at `1280×800` @ 2x DPR so all 5 tabs (`Tổng quan`, `Dọn dẹp`, `Chuyển dữ liệu`, `Vault`, `Cài đặt`) are crystal-clear, authentic GreenC v2.0.0 screens in Vietnamese with realistic C: drive & cleanup data.

## Verification & Deployment Plan
1. Build frontend (`npm run build`) and run `node server.mjs` on `http://localhost:3010`.
2. Verify in browser (via `browser` subagent): visual polish, VI/EN toggle, Before/After showcase, screenshot tabs & lightbox, login modal, unauthenticated download rejection (`401`).
3. Run **Open Code Review (`ocr`)** on changes and fix any findings.
4. Deploy `web/dist`, `web/server.mjs`, `web/package.json`, and `dist/GreenC-Setup-2.0.0.exe` to `/home/pc/projects/greenc.cungchat.com` on `render_server_santony`.
5. Configure `systemd` service (`greenc-web.service`) on `127.0.0.1:14510` and configure Cloudflare Tunnel route for `greenc.cungchat.com` → `http://127.0.0.1:14510`.
