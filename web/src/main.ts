import './styles.css';
import { DICT, type Lang } from './i18n';
import {
  ACCOUNT_API_BASE,
  buildGoogleLoginUrl,
  consumeOAuthCallbackFromUrl,
  fetchCurrentUser,
  getStoredToken,
  loginWithEmail,
  registerWithEmail,
  requestVerifiedDownload,
  setStoredToken,
  type CungChatUser,
} from './auth';

const LANG_KEY = 'greenc_landing_lang';

interface AppState {
  lang: Lang;
  route: 'home' | 'terms';
  user: CungChatUser | null;
  token: string | null;
  baMode: 'before' | 'after';
  activeTabId: string;
  modalOpen: boolean;
  authMode: 'login' | 'register';
  authEmail: string;
  authBusy: boolean;
  authError: string;
  downloading: boolean;
  lightboxSrc: string | null;
  lightboxAlt: string;
  toast: string;
}

function detectInitialLang(): Lang {
  try {
    const saved = localStorage.getItem(LANG_KEY);
    if (saved === 'vi' || saved === 'en') return saved;
  } catch {
    // ignore
  }
  return 'vi';
}

const state: AppState = {
  lang: detectInitialLang(),
  route: window.location.pathname === '/terms' ? 'terms' : 'home',
  user: null,
  token: null,
  baMode: 'after',
  activeTabId: 'dashboard',
  modalOpen: false,
  authMode: 'login',
  authEmail: '',
  authBusy: false,
  authError: '',
  downloading: false,
  lightboxSrc: null,
  lightboxAlt: '',
  toast: '',
};

let toastTimer: number | null = null;

function esc(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function showToast(msg: string): void {
  state.toast = msg;
  renderToast();
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    state.toast = '';
    renderToast();
  }, 4200);
}

function renderToast(): void {
  const el = document.getElementById('gc-toast');
  if (!el) return;
  if (state.toast) {
    el.textContent = state.toast;
    el.hidden = false;
  } else {
    el.hidden = true;
  }
}

const CHECK_ICON = `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="20 6 9 17 4 12"/></svg>`;
const DOWNLOAD_ICON = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>`;
const GOOGLE_G_ICON = `<svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true"><path fill="#4285F4" d="M23.49 12.27c0-.79-.07-1.54-.2-2.27H12v4.3h6.45a5.52 5.52 0 0 1-2.39 3.62v3h3.87c2.26-2.08 3.56-5.15 3.56-8.65z"/><path fill="#34A853" d="M12 24c3.24 0 5.95-1.07 7.93-2.91l-3.87-3c-1.07.72-2.44 1.15-4.06 1.15-3.12 0-5.77-2.11-6.72-4.95H1.28v3.1A12 12 0 0 0 12 24z"/><path fill="#FBBC05" d="M5.28 14.29A7.2 7.2 0 0 1 4.9 12c0-.79.14-1.56.38-2.29v-3.1H1.28A12 12 0 0 0 0 12c0 1.94.46 3.77 1.28 5.39l4-3.1z"/><path fill="#EA4335" d="M12 4.76c1.76 0 3.34.61 4.59 1.8l3.44-3.44C17.95 1.19 15.24 0 12 0A12 12 0 0 0 1.28 6.61l4 3.1C6.23 6.87 8.88 4.76 12 4.76z"/></svg>`;

function safetyIcon(kind: 'shield' | 'lock' | 'heart'): string {
  if (kind === 'shield') {
    return `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><polyline points="9 12 11 14 15 10"/></svg>`;
  }
  if (kind === 'lock') {
    return `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>`;
  }
  return `<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"/></svg>`;
}

async function triggerDownload(): Promise<void> {
  const t = DICT[state.lang];
  if (!state.token || !state.user) {
    state.authError = '';
    state.modalOpen = true;
    render();
    return;
  }

  state.downloading = true;
  render();
  try {
    const { downloadUrl, filename } = await requestVerifiedDownload(state.token);
    const a = document.createElement('a');
    a.href = downloadUrl;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
    state.modalOpen = false;
    showToast(t.auth.downloadStarted);
  } catch (err) {
    if (err instanceof Error && err.message === 'UNAUTHORIZED') {
      state.user = null;
      state.token = null;
      state.authError = t.auth.errDownload;
      state.modalOpen = true;
    } else {
      showToast(t.auth.errGeneric);
    }
  } finally {
    state.downloading = false;
    render();
  }
}

function navigateTo(route: 'home' | 'terms'): void {
  state.route = route;
  const targetPath = route === 'terms' ? '/terms' : '/';
  if (window.location.pathname !== targetPath) {
    window.history.pushState({}, '', targetPath);
  }
  window.scrollTo({ top: 0, behavior: 'smooth' });
  render();
}

function render(): void {
  const t = DICT[state.lang];
  document.documentElement.lang = state.lang;
  document.title = state.route === 'terms' ? t.terms.title : t.meta.title;
  const metaDesc = document.querySelector('meta[name="description"]');
  if (metaDesc) metaDesc.setAttribute('content', t.meta.description);

  const activeTab = t.tour.tabs.find((x) => x.id === state.activeTabId) || t.tour.tabs[0];
  const isBefore = state.baMode === 'before';
  let authSubmitLabel = t.auth.submitRegister;
  if (state.authBusy) {
    authSubmitLabel = t.auth.submitting;
  } else if (state.authMode === 'login') {
    authSubmitLabel = t.auth.submitLogin;
  }

  const app = document.getElementById('app');
  if (!app) return;

  const mainContent =
    state.route === 'terms'
      ? `
      <section class="gc-terms">
        <div class="gc-wrap gc-terms-wrap">
          <a href="/" class="gc-back-link js-nav-home">${esc(t.terms.backHome)}</a>
          <h1>${esc(t.terms.title)}</h1>
          <p class="gc-terms-date">${esc(t.terms.updated)}</p>
          <p class="gc-terms-intro">${esc(t.terms.intro)}</p>
          <div class="gc-terms-body">
            ${t.terms.sections
              .map(
                (sec) => `
              <section class="gc-terms-sec">
                <h2>${esc(sec.heading)}</h2>
                ${sec.body.map((p) => `<p>${esc(p)}</p>`).join('')}
              </section>
            `
              )
              .join('')}
          </div>
        </div>
      </section>
    `
      : `
      <section class="gc-hero">
        <div class="gc-wrap">
          <h1>
            ${esc(t.hero.titleLine1)}<br />
            <span class="gc-accent">${esc(t.hero.titleLine2)}</span>
          </h1>

          <p class="gc-hero-sub">${esc(t.hero.subtitle)}</p>

          <div class="gc-hero-cta">
            <button type="button" class="gc-btn gc-btn-primary js-download" ${state.downloading ? 'disabled' : ''}>
              ${DOWNLOAD_ICON}
              <span>${esc(state.downloading ? t.hero.ctaDownloading : t.hero.ctaPrimary)}</span>
            </button>
            <a href="#screenshots" class="gc-btn gc-btn-outline">${esc(t.hero.ctaSecondary)}</a>
          </div>

          <div class="gc-hero-notes">
            ${t.hero.pillars
              .map((pillar) => `<span class="gc-hero-note">${CHECK_ICON} ${esc(pillar)}</span>`)
              .join('')}
          </div>

          <div class="gc-window">
            <div class="gc-window-bar">
              <div class="gc-dots"><span></span><span></span><span></span></div>
              <span>${esc(t.hero.windowTitle)}</span>
              <span>v2.0</span>
            </div>
            <img
              src="/screenshots/dashboard.png"
              alt="${esc(t.tour.tabs[0].alt)}"
              class="gc-shot js-zoom"
              data-src="/screenshots/dashboard.png"
              data-alt="${esc(t.tour.tabs[0].alt)}"
            />
          </div>
        </div>
      </section>

      <section id="results" class="gc-section gc-section-tint">
        <div class="gc-wrap">
          <div class="gc-sec-head">
            <h2>${esc(t.impact.title)}</h2>
            <p>${esc(t.impact.subtitle)}</p>
          </div>

          <div class="gc-stats">
            ${t.impact.stats
              .map(
                (s) => `
              <article class="gc-stat">
                <div class="gc-stat-val">${esc(s.value)}</div>
                <div class="gc-stat-lbl">${esc(s.label)}</div>
                <p>${esc(s.desc)}</p>
              </article>
            `
              )
              .join('')}
          </div>

          <div class="gc-ba">
            <div class="gc-ba-top">
              <div>
                <h3>${esc(isBefore ? t.impact.beforeLabel : t.impact.afterLabel)}</h3>
                <p>${esc(isBefore ? t.impact.beforeHint : t.impact.afterHint)}</p>
              </div>
              <div class="gc-seg" role="group">
                <button type="button" data-ba="before" class="${isBefore ? 'is-before' : ''}">${esc(t.impact.toggleBefore)}</button>
                <button type="button" data-ba="after" class="${!isBefore ? 'is-after' : ''}">${esc(t.impact.toggleAfter)}</button>
              </div>
            </div>

            <div class="gc-meter">
              <div class="gc-meter-head">
                <span class="gc-drive-lbl">${esc(t.impact.driveTitle)}</span>
                <span class="gc-drive-stat ${isBefore ? 'is-before' : 'is-after'}">
                  ${esc(isBefore ? t.impact.beforeStatus : t.impact.afterStatus)}
                </span>
              </div>
              <div class="gc-bar-bg">
                <div class="gc-bar-fg ${isBefore ? 'is-before' : 'is-after'}"></div>
              </div>
              <p class="gc-meter-msg">${esc(isBefore ? t.impact.beforeSummary : t.impact.afterSummary)}</p>
            </div>

            <p class="gc-bk-title">${esc(t.impact.breakdownTitle)}</p>
            <div class="gc-bk-grid">
              ${t.impact.breakdownItems
                .map(
                  (item) => `
                <div class="gc-bk-item">
                  <div class="gc-bk-row">
                    <span>${esc(item.name)}</span>
                    <span class="gc-bk-val">${esc(item.saved)}</span>
                  </div>
                  <p>${esc(item.detail)}</p>
                </div>
              `
                )
                .join('')}
            </div>
          </div>
        </div>
      </section>

      <section id="screenshots" class="gc-section">
        <div class="gc-wrap">
          <div class="gc-sec-head">
            <h2>${esc(t.tour.title)}</h2>
            <p>${esc(t.tour.subtitle)}</p>
          </div>

          <div class="gc-tabs" role="tablist">
            ${t.tour.tabs
              .map(
                (tab) => `
              <button
                type="button"
                role="tab"
                id="gc-tab-${esc(tab.id)}"
                aria-selected="${tab.id === activeTab.id}"
                aria-controls="gc-tabpanel-tour"
                data-tab="${esc(tab.id)}"
                class="gc-tab ${tab.id === activeTab.id ? 'is-active' : ''}"
              >${esc(tab.label)}</button>
            `
              )
              .join('')}
          </div>

          <div id="gc-tabpanel-tour" role="tabpanel" aria-labelledby="gc-tab-${esc(activeTab.id)}">
            <div class="gc-tour-cap">
              <h3>${esc(activeTab.title)}</h3>
              <p>${esc(activeTab.desc)}</p>
            </div>

            <div class="gc-window">
              <div class="gc-window-bar">
                <div class="gc-dots"><span></span><span></span><span></span></div>
                <span>GreenC — ${esc(activeTab.label)}</span>
                <span>v2.0</span>
              </div>
              <img
                src="${esc(activeTab.image)}"
                alt="${esc(activeTab.alt)}"
                class="gc-shot js-zoom"
                data-src="${esc(activeTab.image)}"
                data-alt="${esc(activeTab.alt)}"
              />
            </div>
          </div>
          <p class="gc-zoom-note">${esc(t.tour.zoomHint)}</p>
        </div>
      </section>

      <section id="safety" class="gc-section gc-section-tint">
        <div class="gc-wrap">
          <div class="gc-sec-head">
            <h2>${esc(t.safety.title)}</h2>
            <p>${esc(t.safety.subtitle)}</p>
          </div>

          <div class="gc-safety">
            ${t.safety.cards
              .map(
                (c) => `
              <article class="gc-safe-card">
                <div class="gc-safe-ico">${safetyIcon(c.icon)}</div>
                <h3>${esc(c.title)}</h3>
                <p>${esc(c.desc)}</p>
              </article>
            `
              )
              .join('')}
          </div>
        </div>
      </section>

      <section id="testimonials" class="gc-section">
        <div class="gc-wrap">
          <div class="gc-sec-head">
            <h2>${esc(t.testimonials.title)}</h2>
            <p>${esc(t.testimonials.subtitle)}</p>
          </div>

          <div class="gc-quotes">
            ${t.testimonials.items
              .map(
                (item) => `
              <figure class="gc-quote-card">
                <blockquote>“${esc(item.quote)}”</blockquote>
                <figcaption class="gc-quote-author">
                  <span class="gc-avatar gc-tone-${esc(item.tone)}" aria-hidden="true">${esc(item.initials)}</span>
                  <div>
                    <div class="gc-author-name">${esc(item.name)}</div>
                    <div class="gc-author-role">${esc(item.role)}</div>
                  </div>
                </figcaption>
              </figure>
            `
              )
              .join('')}
          </div>
        </div>
      </section>

      <section id="faq" class="gc-section gc-section-tint">
        <div class="gc-wrap">
          <div class="gc-sec-head">
            <h2>${esc(t.faq.title)}</h2>
          </div>

          <div class="gc-faq">
            ${t.faq.items
              .map(
                (item, idx) => `
              <details ${idx === 0 ? 'open' : ''}>
                <summary>${esc(item.q)}</summary>
                <div class="gc-faq-body">${esc(item.a)}</div>
              </details>
            `
              )
              .join('')}
          </div>
        </div>
      </section>

      <section class="gc-section" style="padding-top: 56px;">
        <div class="gc-wrap">
          <div class="gc-cta-card">
            <h2>${esc(t.ctaBottom.title)}</h2>
            <p>${esc(t.ctaBottom.subtitle)}</p>
            <button type="button" class="gc-btn gc-btn-primary js-download" ${state.downloading ? 'disabled' : ''}>
              ${DOWNLOAD_ICON}
              <span>${esc(state.downloading ? t.hero.ctaDownloading : t.ctaBottom.button)}</span>
            </button>
            <div class="gc-cta-meta">${esc(t.ctaBottom.meta)}</div>
          </div>
        </div>
      </section>
    `;

  app.innerHTML = `
    <header class="gc-header">
      <div class="gc-wrap gc-header-row">
        <a href="/" class="gc-brand js-nav-home">
          <span class="gc-logo">G</span>
          <span>GreenC</span>
        </a>

        <nav class="gc-nav" aria-label="Primary">
          <a href="/#results" class="js-nav-anchor" data-hash="#results">${esc(t.nav.results)}</a>
          <a href="/#screenshots" class="js-nav-anchor" data-hash="#screenshots">${esc(t.nav.screenshots)}</a>
          <a href="/#safety" class="js-nav-anchor" data-hash="#safety">${esc(t.nav.features)}</a>
          <a href="/#faq" class="js-nav-anchor" data-hash="#faq">${esc(t.nav.faq)}</a>
        </nav>

        <div class="gc-actions">
          <div class="gc-lang" role="group" aria-label="Language">
            <button type="button" data-lang="vi" class="${state.lang === 'vi' ? 'is-active' : ''}">VI</button>
            <button type="button" data-lang="en" class="${state.lang === 'en' ? 'is-active' : ''}">EN</button>
          </div>

          ${
            state.user
              ? `<div class="gc-user-chip">
                  <span class="gc-user-email" title="${esc(state.user.email)}">${esc(state.user.email)}</span>
                  <button type="button" id="btn-signout" class="gc-user-out">${esc(t.nav.signOut)}</button>
                </div>`
              : `<button type="button" id="btn-open-auth" class="gc-btn gc-btn-ghost">${esc(t.nav.signIn)}</button>`
          }

          <button type="button" class="gc-btn gc-btn-primary gc-btn-sm js-download" ${state.downloading ? 'disabled' : ''}>
            ${esc(t.nav.download)}
          </button>
        </div>
      </div>
    </header>

    <main>${mainContent}</main>

    <footer class="gc-footer">
      <div class="gc-wrap gc-footer-row">
        <div>${esc(t.footer.madeBy)} · ${esc(t.footer.version)}</div>
        <div class="gc-footer-links">
          <a href="/terms" class="js-nav-terms">${esc(t.footer.termsLink)}</a>
          <a href="https://github.com/cungchatcom-dotcom/greenC" target="_blank" rel="noopener noreferrer">${esc(t.footer.githubLink)}</a>
          <a href="mailto:cungchat.com@gmail.com">cungchat.com@gmail.com</a>
          <a href="https://cungchat.com" target="_blank" rel="noopener noreferrer">cungchat.com</a>
          <a href="${esc(ACCOUNT_API_BASE)}/app/" target="_blank" rel="noopener noreferrer">${esc(t.footer.accountLink)}</a>
        </div>
      </div>
    </footer>

    <!-- Auth / Download Modal -->
    <div id="gc-auth-modal" class="gc-overlay" ${state.modalOpen ? '' : 'hidden'} role="dialog" aria-modal="true">
      <div class="gc-modal">
        <button type="button" id="btn-close-modal" class="gc-close" aria-label="${esc(t.auth.close)}">×</button>
        <h3>${esc(t.auth.modalTitle)}</h3>
        <p class="gc-modal-sub">${esc(state.user ? t.auth.downloadReady : t.auth.modalSubtitle)}</p>

        ${
          state.user
            ? `
              <div class="gc-user-chip" style="margin-bottom: 18px; max-width: 100%;">
                <span>${esc(t.auth.loggedInAs)} <strong>${esc(state.user.email)}</strong></span>
              </div>
              <button type="button" class="gc-btn gc-btn-primary gc-full js-download" ${state.downloading ? 'disabled' : ''}>
                ${DOWNLOAD_ICON}
                <span>${esc(state.downloading ? t.hero.ctaDownloading : t.auth.downloadNow)}</span>
              </button>
            `
            : `
              <a href="${esc(buildGoogleLoginUrl())}" class="gc-google-btn">
                ${GOOGLE_G_ICON}
                <span>${esc(t.auth.googleBtn)}</span>
              </a>

              <div class="gc-or">${esc(t.auth.orDivider)}</div>

              <div class="gc-auth-tabs">
                <button type="button" data-auth-mode="login" class="${state.authMode === 'login' ? 'is-active' : ''}">${esc(t.auth.tabLogin)}</button>
                <button type="button" data-auth-mode="register" class="${state.authMode === 'register' ? 'is-active' : ''}">${esc(t.auth.tabRegister)}</button>
              </div>

              ${state.authError ? `<div class="gc-err">${esc(state.authError)}</div>` : ''}

              <form id="gc-auth-form">
                <div class="gc-field">
                  <label for="gc-email">${esc(t.auth.emailLabel)}</label>
                  <input id="gc-email" name="email" type="email" required autocomplete="email" value="${esc(state.authEmail)}" placeholder="${esc(t.auth.emailPlaceholder)}" />
                </div>
                <div class="gc-field">
                  <label for="gc-password">${esc(t.auth.passwordLabel)}</label>
                  <input id="gc-password" name="password" type="password" required minlength="6" autocomplete="${state.authMode === 'login' ? 'current-password' : 'new-password'}" placeholder="${esc(t.auth.passwordPlaceholder)}" />
                </div>
                ${state.authMode === 'register' ? `<p class="gc-hint">${esc(t.auth.registerHint)}</p>` : ''}
                <button type="submit" class="gc-btn gc-btn-primary gc-full" ${state.authBusy ? 'disabled' : ''}>
                  ${esc(authSubmitLabel)}
                </button>
              </form>
            `
        }
      </div>
    </div>

    <!-- Screenshot Lightbox -->
    <div id="gc-lightbox" class="gc-overlay" ${state.lightboxSrc ? '' : 'hidden'} role="dialog" aria-modal="true">
      <div class="gc-lb-wrap">
        <button type="button" id="btn-close-lb" class="gc-lb-close" aria-label="${esc(t.auth.close)}">×</button>
        ${state.lightboxSrc ? `<img src="${esc(state.lightboxSrc)}" alt="${esc(state.lightboxAlt)}" />` : ''}
      </div>
    </div>

    <div id="gc-toast" class="gc-toast" ${state.toast ? '' : 'hidden'} role="status" aria-live="polite">${esc(state.toast)}</div>
  `;

  bindEvents();
}

function bindEvents(): void {
  document.querySelectorAll<HTMLAnchorElement>('.js-nav-terms').forEach((link) => {
    link.addEventListener('click', (e) => {
      e.preventDefault();
      navigateTo('terms');
    });
  });

  document.querySelectorAll<HTMLAnchorElement>('.js-nav-home').forEach((link) => {
    link.addEventListener('click', (e) => {
      e.preventDefault();
      navigateTo('home');
    });
  });

  document.querySelectorAll<HTMLAnchorElement>('.js-nav-anchor').forEach((link) => {
    link.addEventListener('click', (e) => {
      const hash = link.dataset.hash;
      if (!hash) return;
      if (state.route !== 'home') {
        e.preventDefault();
        state.route = 'home';
        window.history.pushState({}, '', '/' + hash);
        render();
        const target = document.querySelector(hash);
        target?.scrollIntoView({ behavior: 'smooth' });
      }
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-lang]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const next = btn.dataset.lang as Lang;
      if (next === 'vi' || next === 'en') {
        state.lang = next;
        try {
          localStorage.setItem(LANG_KEY, next);
        } catch {
          // ignore
        }
        render();
      }
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-ba]').forEach((btn) => {
    btn.addEventListener('click', () => {
      state.baMode = btn.dataset.ba === 'before' ? 'before' : 'after';
      render();
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-tab]').forEach((btn) => {
    btn.addEventListener('click', () => {
      if (btn.dataset.tab) {
        state.activeTabId = btn.dataset.tab;
        render();
      }
    });
  });

  document.querySelectorAll<HTMLElement>('.js-zoom').forEach((img) => {
    img.addEventListener('click', () => {
      state.lightboxSrc = img.dataset.src || null;
      state.lightboxAlt = img.dataset.alt || '';
      render();
    });
  });

  document.querySelectorAll<HTMLButtonElement>('.js-download').forEach((btn) => {
    btn.addEventListener('click', () => {
      void triggerDownload();
    });
  });

  document.getElementById('btn-open-auth')?.addEventListener('click', () => {
    state.authError = '';
    state.modalOpen = true;
    render();
  });

  document.getElementById('btn-signout')?.addEventListener('click', () => {
    setStoredToken(null);
    state.token = null;
    state.user = null;
    render();
  });

  document.getElementById('btn-close-modal')?.addEventListener('click', () => {
    state.modalOpen = false;
    render();
  });

  document.getElementById('gc-auth-modal')?.addEventListener('click', (e) => {
    if (e.target === e.currentTarget) {
      state.modalOpen = false;
      render();
    }
  });

  document.getElementById('btn-close-lb')?.addEventListener('click', () => {
    state.lightboxSrc = null;
    render();
  });

  document.getElementById('gc-lightbox')?.addEventListener('click', (e) => {
    if (e.target === e.currentTarget) {
      state.lightboxSrc = null;
      render();
    }
  });

  document.querySelectorAll<HTMLButtonElement>('[data-auth-mode]').forEach((btn) => {
    btn.addEventListener('click', () => {
      state.authMode = btn.dataset.authMode === 'register' ? 'register' : 'login';
      state.authError = '';
      render();
    });
  });

  const form = document.getElementById('gc-auth-form') as HTMLFormElement | null;
  form?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const t = DICT[state.lang];
    const formData = new FormData(form);
    const email = String(formData.get('email') || '');
    const password = String(formData.get('password') || '');

    state.authEmail = email;
    state.authBusy = true;
    state.authError = '';
    render();

    try {
      const token =
        state.authMode === 'login'
          ? await loginWithEmail(email, password)
          : await registerWithEmail(email, password);
      const user = await fetchCurrentUser(token);
      if (!user) throw new Error('INVALID_CREDENTIALS');
      state.token = token;
      state.user = user;
      state.authBusy = false;
      await triggerDownload();
    } catch (err) {
      state.authBusy = false;
      const msg = err instanceof Error ? err.message : '';
      if (msg === 'INVALID_CREDENTIALS') {
        state.authError = t.auth.errInvalidCredentials;
      } else {
        state.authError = msg || t.auth.errGeneric;
      }
      render();
    }
  });
}

window.addEventListener('popstate', () => {
  state.route = window.location.pathname === '/terms' ? 'terms' : 'home';
  render();
});

window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    if (state.lightboxSrc) {
      state.lightboxSrc = null;
      render();
    } else if (state.modalOpen) {
      state.modalOpen = false;
      render();
    }
  }
});

async function bootstrap(): Promise<void> {
  render();
  const fromCallback = consumeOAuthCallbackFromUrl();
  const token = fromCallback || getStoredToken();
  if (!token) return;

  const user = await fetchCurrentUser(token);
  if (user) {
    state.token = token;
    state.user = user;
    if (fromCallback) {
      state.modalOpen = true;
    }
    render();
  } else {
    setStoredToken(null);
  }
}

void bootstrap();
