# Feature Specification: GreenC Landing Page (`greenc.cungchat.com`)

**Feature Branch**: `003-greenc-landing-page`  
**Created**: 2026-11-20  
**Status**: Approved  
**Target Audience**: Everyday Windows users (primarily non-technical / low-tech users who experience a full red C: drive).

## 1. Overview & Goals

Build a clean, Google-style minimalist landing page for **GreenC** (`greenc.cungchat.com`) that:
1. Explains what GreenC does in **simple, everyday language** (no technical jargon like Rust, Tauri, NTFS Junction, ZIP64, SHA-256, JWT in primary user copy).
2. Showcases **real product screenshots** captured directly from GreenC v2.0.0 and demonstrates real-world effectiveness (Before/After C: drive storage bar + plain-language real performance metrics).
3. Allows **free download** of `GreenC-Setup-2.0.0.exe` **strictly gated behind `cungchat-account`** (`https://account.cungchat.com`), enforced server-side so the installer cannot be downloaded without a valid `cungchat-account` session.
4. Supports **Vietnamese (default)** and **English** with an instant language switcher.
5. Runs locally on `localhost` for preview/verification first, then deploys to `render_server_santony` behind Cloudflare Tunnel at `https://greenc.cungchat.com`.

## 2. User Stories & Acceptance Criteria

### US1 — Discover GreenC & Understand Value Immediately (P1)
- As a non-technical Windows user whose C: drive is full, I land on `greenc.cungchat.com` and within 5 seconds understand what GreenC does, see real screenshots of the app, and see how much space it can free safely.
- **Acceptance Criteria**:
  - Hero displays plain-language headline, concise subtitle, primary "Tải miễn phí cho Windows" CTA, and a crisp real screenshot of GreenC's main screen.
  - Visual style follows Google's clean product aesthetic: pure white `#FFFFFF` + light neutral `#F8F9FA` surfaces, `#202124` / `#5F6368` ink hierarchy, `#137333` / `#1E8E3E` GreenC emerald accent, generous whitespace, rounded pill buttons, subtle `#DADCE0` hairlines.
  - Zero intimidating technical jargon in headlines, cards, or FAQs.

### US2 — View Real Product Screenshots & Effectiveness Showcase (P1)
- As a prospective user, I want to inspect real screens of GreenC and see realistic before/after results so I trust the app before installing it.
- **Acceptance Criteria**:
  - Interactive **Before / After** C: drive bar showing a red full drive turning green with clear plain-language breakdown (temporary files, Downloads/Documents moved to D:, Zalo cache).
  - 4 real-world impact cards written in plain language (4.5× faster cleanup, ultra-light < 15 MB background RAM, tiny 5.6 MB installer, Vietnamese + 49 languages).
  - Interactive **Product Tour** with tabs for real GreenC screenshots (Tổng quan / Dọn dẹp / Chuyển sang ổ D / Kho an toàn / Cài đặt) and click-to-zoom lightbox.

### US3 — Authenticate with `cungchat-account` & Download Installer (P1)
- As a user, I click "Tải miễn phí" and sign in (or register) with my `cungchat-account` (Google One-Click via `account.cungchat.com/auth/google` or Email/Password via `account.cungchat.com/auth/login` & `/auth/register`) to download `GreenC-Setup-2.0.0.exe`.
- **Acceptance Criteria**:
  - Clicking Download while logged out opens a clean Google-style sign-in modal with:
    - "Tiếp tục với Google" (redirects to `https://account.cungchat.com/auth/google?redirect_uri=<origin>/auth/callback`).
    - Email + Password login tab (`POST https://account.cungchat.com/auth/login`).
    - Quick registration tab (`POST https://account.cungchat.com/auth/register`).
  - `/auth/callback` captures `?accessToken=...`, validates via `GET https://account.cungchat.com/users/me`, cleans the URL immediately via `history.replaceState`, and returns the user to the download-ready state.
  - Clicking Download while logged in requests a short-lived single-use ticket from `POST /api/download-ticket` (with `Authorization: Bearer <accessToken>`). The backend verifies the token directly with `account.cungchat.com/users/me` before issuing a 60-second single-use ticket, then streams `GreenC-Setup-2.0.0.exe` via `GET /api/download?ticket=<ticket>`.
  - Direct access to `/api/download` without a valid unused ticket returns `401 Unauthorized`. The installer file is never placed inside the public static folder.

### US4 — Bilingual Support (VI / EN) & Responsive Layout (P2)
- Users can switch between Vietnamese (default) and English with a single click in the header; choice persists in `localStorage`.
- Layout adapts cleanly across desktop, tablet, and mobile viewports.

## 3. Non-Functional Requirements
- Fast load: Vite-bundled lightweight HTML/CSS/TS frontend, compressed WebP/PNG screenshots.
- Security: Server-side token validation against `account.cungchat.com/users/me`, single-use expiring download tickets, security headers (`X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`), no token left in URL history.
- Operational simplicity: Single Node.js server (`server.mjs`) running on `127.0.0.1:14510` via `systemd` on `render_server_santony`, exposed as `https://greenc.cungchat.com` through Cloudflare Tunnel.
