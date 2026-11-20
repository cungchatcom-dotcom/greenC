// Reactive Svelte 5 Localization & Theme Store with Runes, RTL, and Comma-Decimal Support (Tasks T024-T026, T065)

export const RTL_LANGUAGES = ['ar', 'he', 'fa', 'ur'] as const;

export const COMMA_DECIMAL_LANGUAGES = [
  'vi', 'id', 'tr', 'az', 'ru', 'uk', 'pl', 'cs', 'sk', 'hu', 'ro', 'bg', 'sr', 'hr', 'el',
  'de', 'fr', 'es', 'pt_BR', 'pt_PT', 'it', 'nl', 'sv', 'da', 'nb', 'fi', 'fa'
] as const;

export type ThemeModeOption = 'system' | 'light' | 'dark';

export function isRtlLocale(locale: string): boolean {
  return (RTL_LANGUAGES as readonly string[]).includes(locale);
}

export function getDecimalSeparator(locale: string): ',' | '.' {
  return (COMMA_DECIMAL_LANGUAGES as readonly string[]).includes(locale) ? ',' : '.';
}

/**
 * Wraps Windows paths, sizes, or version numbers in Unicode First-Strong / LTR isolation marks
 * so they never scramble when rendered inside an RTL (`ar`, `he`, `fa`, `ur`) sentence.
 */
const LTR_ISOLATE_START = String.fromCharCode(0x2066);
const LTR_ISOLATE_END = String.fromCharCode(0x2069);

export function isolateLtr(value: string): string {
  return LTR_ISOLATE_START + value + LTR_ISOLATE_END;
}

export function resolveEffectiveDark(mode: ThemeModeOption, systemPrefersDark: boolean): boolean {
  if (mode === 'dark') return true;
  if (mode === 'light') return false;
  return systemPrefersDark;
}

class I18nStore {
  locale = $state('vi');
  translations = $state<Record<string, string>>({});
  themeMode = $state<ThemeModeOption>('system');
  private systemPrefersDark = $state(false);
  private mediaBound = false;

  isRtl = $derived(isRtlLocale(this.locale));
  decimalSep = $derived(getDecimalSeparator(this.locale));
  isDark = $derived(resolveEffectiveDark(this.themeMode, this.systemPrefersDark));

  constructor() {
    this.bindSystemThemeListener();
  }

  private bindSystemThemeListener() {
    if (this.mediaBound || typeof window === 'undefined' || !window.matchMedia) {
      return;
    }
    this.mediaBound = true;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    this.systemPrefersDark = mq.matches;
    mq.addEventListener('change', (ev) => {
      this.systemPrefersDark = ev.matches;
      this.syncDocumentClasses();
    });
  }

  init(locale: string, translations?: Record<string, string>, themeMode?: string) {
    this.bindSystemThemeListener();
    this.locale = locale;
    if (translations) {
      this.translations = translations;
    }
    if (themeMode === 'light' || themeMode === 'dark' || themeMode === 'system') {
      this.themeMode = themeMode;
    }
    this.updateDocumentDirection();
    this.syncDocumentClasses();
  }

  setLocale(locale: string, translations?: Record<string, string>) {
    this.locale = locale;
    if (translations) {
      this.translations = translations;
    }
    this.updateDocumentDirection();
  }

  setThemeMode(mode: string) {
    this.bindSystemThemeListener();
    if (mode === 'light' || mode === 'dark' || mode === 'system') {
      this.themeMode = mode;
    } else {
      this.themeMode = 'system';
    }
    this.syncDocumentClasses();
  }

  private updateDocumentDirection() {
    if (typeof document !== 'undefined') {
      document.documentElement.setAttribute('dir', this.isRtl ? 'rtl' : 'ltr');
      document.documentElement.setAttribute('lang', this.locale);
    }
  }

  private syncDocumentClasses() {
    if (typeof document !== 'undefined') {
      document.documentElement.classList.toggle('dark', this.isDark);
    }
  }

  t(key: string, params?: Record<string, string | number>): string {
    const raw = this.translations[key] || key;
    if (!params) return raw;

    let result = raw;
    for (const [k, v] of Object.entries(params)) {
      // Matches both standard tokens ({k}) and Python format specifiers ({k:.0f}, {k:d})
      result = result.replace(new RegExp(`\\{${k}(?::[^}]+)?\\}`, 'g'), String(v));
    }
    return result;
  }

  formatSize(bytes: number): string {
    if (bytes === 0) return '0 B';
    const sign = bytes < 0 ? '-' : '';
    let val = Math.abs(bytes);
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    for (const unit of units) {
      if (val < 1024 || unit === 'TB') {
        const formatted = unit === 'B' || unit === 'KB'
          ? Math.round(val).toString()
          : val.toFixed(1).replace('.', this.decimalSep);
        return `${sign}${formatted} ${unit}`;
      }
      val /= 1024;
    }
    return `${sign}${val.toFixed(1).replace('.', this.decimalSep)} TB`;
  }

  formatSignedSize(deltaBytes: number): string {
    if (deltaBytes === 0) return '0 B';
    const absFormatted = this.formatSize(Math.abs(deltaBytes));
    return deltaBytes > 0 ? `+${absFormatted}` : `-${absFormatted}`;
  }

  formatEta(seconds: number): string {
    if (seconds >= 3600) {
      const h = Math.floor(seconds / 3600);
      const m = Math.floor((seconds % 3600) / 60);
      const s = seconds % 60;
      return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
    }
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return `${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
  }
}

export const i18n = new I18nStore();
export const t = (key: string, params?: Record<string, string | number>) => i18n.t(key, params);
export const formatSize = (bytes: number) => i18n.formatSize(bytes);
export const formatSignedSize = (deltaBytes: number) => i18n.formatSignedSize(deltaBytes);
export const formatEta = (seconds: number) => i18n.formatEta(seconds);
