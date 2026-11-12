"""Tiny gettext-style translation layer.

Vietnamese is the source language: every user-visible string in the code is written in
Vietnamese and wrapped in ``tr()``. Other languages live in ``locales/<code>.json`` as
``{source: translation}``. The language is chosen once at startup (``set_language``);
changing it in Settings takes effect after a restart.

    tr("Dọn dẹp {size}", size="4,0 GB")   # translate, then str.format(**kwargs)
    N_("Đang chạy")                        # mark a module-level constant; call tr() where it is shown
"""

from __future__ import annotations

import json
from pathlib import Path

SOURCE_LANGUAGE = "vi"
FALLBACK_LANGUAGE = "en"
LOCALES_DIR = Path(__file__).resolve().parent / "locales"

# (code, native name, English name). Codes follow BCP 47 with "_" instead of "-".
LANGUAGES: list[tuple[str, str, str]] = [
    ("vi", "Tiếng Việt", "Vietnamese"),
    ("en", "English", "English"),
    ("zh_CN", "简体中文", "Chinese (Simplified)"),
    ("zh_TW", "繁體中文", "Chinese (Traditional)"),
    ("ja", "日本語", "Japanese"),
    ("ko", "한국어", "Korean"),
    ("th", "ไทย", "Thai"),
    ("id", "Bahasa Indonesia", "Indonesian"),
    ("ms", "Bahasa Melayu", "Malay"),
    ("fil", "Filipino", "Filipino"),
    ("km", "ខ្មែរ", "Khmer"),
    ("lo", "ລາວ", "Lao"),
    ("my", "မြန်မာ", "Burmese"),
    ("hi", "हिन्दी", "Hindi"),
    ("bn", "বাংলা", "Bengali"),
    ("ur", "اردو", "Urdu"),
    ("ta", "தமிழ்", "Tamil"),
    ("te", "తెలుగు", "Telugu"),
    ("mr", "मराठी", "Marathi"),
    ("gu", "ગુજરાતી", "Gujarati"),
    ("pa", "ਪੰਜਾਬੀ", "Punjabi"),
    ("ne", "नेपाली", "Nepali"),
    ("fa", "فارسی", "Persian"),
    ("ar", "العربية", "Arabic"),
    ("he", "עברית", "Hebrew"),
    ("tr", "Türkçe", "Turkish"),
    ("az", "Azərbaycanca", "Azerbaijani"),
    ("ru", "Русский", "Russian"),
    ("uk", "Українська", "Ukrainian"),
    ("pl", "Polski", "Polish"),
    ("cs", "Čeština", "Czech"),
    ("sk", "Slovenčina", "Slovak"),
    ("hu", "Magyar", "Hungarian"),
    ("ro", "Română", "Romanian"),
    ("bg", "Български", "Bulgarian"),
    ("sr", "Српски", "Serbian"),
    ("hr", "Hrvatski", "Croatian"),
    ("el", "Ελληνικά", "Greek"),
    ("de", "Deutsch", "German"),
    ("fr", "Français", "French"),
    ("es", "Español", "Spanish"),
    ("pt_BR", "Português (Brasil)", "Portuguese (Brazil)"),
    ("pt_PT", "Português (Portugal)", "Portuguese (Portugal)"),
    ("it", "Italiano", "Italian"),
    ("nl", "Nederlands", "Dutch"),
    ("sv", "Svenska", "Swedish"),
    ("da", "Dansk", "Danish"),
    ("nb", "Norsk bokmål", "Norwegian Bokmål"),
    ("fi", "Suomi", "Finnish"),
    ("sw", "Kiswahili", "Swahili"),
]
LANGUAGE_CODES = [code for code, _, _ in LANGUAGES]
RTL_LANGUAGES = {"ar", "he", "fa", "ur"}

_current = SOURCE_LANGUAGE
_catalog: dict[str, str] = {}


def load_catalog(code: str) -> dict[str, str]:
    if code == SOURCE_LANGUAGE:
        return {}
    path = LOCALES_DIR / f"{code}.json"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return {k: v for k, v in data.items() if isinstance(v, str) and v}


def set_language(code: str | None) -> str:
    """Activate a language; unknown codes fall back to the source language."""
    global _current, _catalog
    code = code if code in LANGUAGE_CODES else SOURCE_LANGUAGE
    _current = code
    _catalog = load_catalog(code)
    return code


def current_language() -> str:
    return _current


def is_rtl(code: str | None = None) -> bool:
    return (code or _current) in RTL_LANGUAGES


def tr(text: str, /, **kwargs) -> str:
    translated = _catalog.get(text, text)
    if not kwargs:
        return translated
    try:
        return translated.format(**kwargs)
    except (KeyError, IndexError, ValueError):
        return text.format(**kwargs)


def N_(text: str) -> str:
    """Mark a string for extraction without translating it yet."""
    return text


def match_system_language(qt_locale_name: str) -> str:
    """Map a Qt locale name such as 'pt_BR', 'zh_Hant_TW' or 'vi_VN' to a supported code."""
    name = qt_locale_name.replace("-", "_")
    parts = name.split("_")
    lang = parts[0].lower()
    if lang == "zh":
        return "zh_TW" if any(p in ("Hant", "TW", "HK", "MO") for p in parts[1:]) else "zh_CN"
    if lang == "pt":
        return "pt_PT" if "PT" in parts[1:] else "pt_BR"
    if lang in ("no", "nn"):
        return "nb"
    if lang == "tl":
        return "fil"
    return lang if lang in LANGUAGE_CODES else FALLBACK_LANGUAGE


def decimal_point() -> str:
    return "," if _current in _COMMA_DECIMAL else "."


_COMMA_DECIMAL = {
    "vi", "id", "tr", "az", "ru", "uk", "pl", "cs", "sk", "hu", "ro", "bg", "sr", "hr", "el",
    "de", "fr", "es", "pt_BR", "pt_PT", "it", "nl", "sv", "da", "nb", "fi", "fa",
}
