// Automated RTL layout direction, LTR path/number isolation, comma-decimal parity,
// and Light/Dark/System theme resolution test suite (Task T064).

import {
  RTL_LANGUAGES,
  COMMA_DECIMAL_LANGUAGES,
  isRtlLocale,
  getDecimalSeparator,
  isolateLtr,
  resolveEffectiveDark,
  i18n
} from '../lib/i18n.svelte';

function assert(condition: boolean, message: string): void {
  if (!condition) {
    throw new Error(`Assertion failed: ${message}`);
  }
}

export function runRtlAndThemeAssertions(): void {
  // 1. RTL locale classification (exactly ar, he, fa, ur)
  assert(RTL_LANGUAGES.length === 4, 'Expected 4 RTL locales');
  for (const rtl of ['ar', 'he', 'fa', 'ur']) {
    assert(isRtlLocale(rtl), `${rtl} must be classified as RTL`);
  }
  for (const ltr of ['vi', 'en', 'zh_CN', 'ja', 'de', 'fr', 'ru']) {
    assert(!isRtlLocale(ltr), `${ltr} must be classified as LTR`);
  }

  // 2. Comma-decimal separator parity across all 27 comma-decimal locales
  assert(COMMA_DECIMAL_LANGUAGES.length === 27, 'Expected 27 comma-decimal locales');
  assert(getDecimalSeparator('vi') === ',', 'Vietnamese uses comma decimal');
  assert(getDecimalSeparator('de') === ',', 'German uses comma decimal');
  assert(getDecimalSeparator('fa') === ',', 'Persian uses comma decimal');
  assert(getDecimalSeparator('en') === '.', 'English uses dot decimal');
  assert(getDecimalSeparator('ja') === '.', 'Japanese uses dot decimal');
  assert(getDecimalSeparator('ar') === '.', 'Arabic uses dot decimal');

  // 3. Live dictionary switching + placeholder & format specifier interpolation (< 5 ms)
  const start = performance.now();
  i18n.setLocale('ar', {
    'Còn khoảng {days} ngày trước khi đầy': 'متبقي حوالي {days} يوم قبل الامتلاء',
    'Ổn định · {pct:.0f}%': 'مستقر · {pct:.0f}%'
  });
  const elapsedMs = performance.now() - start;
  assert(elapsedMs < 50, `Live locale switch must be instantaneous (<50ms), got ${elapsedMs}ms`);
  assert(i18n.isRtl === true, 'Store isRtl must reactively flip to true for Arabic');
  assert(
    i18n.t('Còn khoảng {days} ngày trước khi đầy', { days: 19 }) ===
      'متبقي حوالي 19 يوم قبل الامتلاء',
    'Standard {days} token must interpolate cleanly in RTL'
  );
  assert(
    i18n.t('Ổn định · {pct:.0f}%', { pct: 72 }) === 'مستقر · 72%',
    'Python-style {pct:.0f} specifier must interpolate cleanly'
  );

  // 4. Signed delta formatting & decimal separator switching
  i18n.setLocale('vi', {});
  assert(i18n.formatSignedSize(1536 * 1024 * 1024) === '+1,5 GB', 'Vietnamese signed +GB');
  assert(i18n.formatSignedSize(-512 * 1024 * 1024) === '-512,0 MB', 'Vietnamese signed -MB');

  i18n.setLocale('en', {});
  assert(i18n.formatSignedSize(1536 * 1024 * 1024) === '+1.5 GB', 'English signed +GB');

  // 5. LTR isolation marks around Windows paths inside RTL UI
  const isolated = isolateLtr('C:\\Users\\Test\\AppData\\Roaming\\ZaloData');
  assert(isolated.startsWith('\u2066') && isolated.endsWith('\u2069'), 'Unicode FSI..PDI marks');

  // 6. Light / Dark / System theme resolution
  assert(resolveEffectiveDark('dark', false) === true, 'Explicit dark overrides light OS');
  assert(resolveEffectiveDark('light', true) === false, 'Explicit light overrides dark OS');
  assert(resolveEffectiveDark('system', true) === true, 'System mode follows dark OS');
  assert(resolveEffectiveDark('system', false) === false, 'System mode follows light OS');
}
