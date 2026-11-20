use crate::format::interpolate;
use std::collections::HashMap;

pub const SOURCE_LANGUAGE: &str = "vi";
pub const FALLBACK_LANGUAGE: &str = "en";

pub const LANGUAGES: &[(&str, &str, &str)] = &[
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
];

pub fn is_valid_language_code(code: &str) -> bool {
    LANGUAGES.iter().any(|(c, _, _)| *c == code)
}

#[derive(Debug, Clone)]
pub struct Catalog {
    locales: HashMap<String, HashMap<String, String>>,
    source_keys: Vec<String>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::load_embedded()
    }
}

impl Catalog {
    pub fn load_embedded() -> Self {
        let mut locales = HashMap::with_capacity(50);

        macro_rules! insert_locale {
            ($code:expr, $file:expr) => {
                let json_str = include_str!($file).trim_start_matches('\u{feff}');
                let map: HashMap<String, String> = serde_json::from_str(json_str)
                    .unwrap_or_else(|e| panic!("Failed to parse locale {}: {}", $code, e));
                locales.insert($code.to_string(), map);
            };
        }

        insert_locale!("vi", "../locales/vi.json");
        insert_locale!("en", "../locales/en.json");
        insert_locale!("zh_CN", "../locales/zh_CN.json");
        insert_locale!("zh_TW", "../locales/zh_TW.json");
        insert_locale!("ja", "../locales/ja.json");
        insert_locale!("ko", "../locales/ko.json");
        insert_locale!("th", "../locales/th.json");
        insert_locale!("id", "../locales/id.json");
        insert_locale!("ms", "../locales/ms.json");
        insert_locale!("fil", "../locales/fil.json");
        insert_locale!("km", "../locales/km.json");
        insert_locale!("lo", "../locales/lo.json");
        insert_locale!("my", "../locales/my.json");
        insert_locale!("hi", "../locales/hi.json");
        insert_locale!("bn", "../locales/bn.json");
        insert_locale!("ur", "../locales/ur.json");
        insert_locale!("ta", "../locales/ta.json");
        insert_locale!("te", "../locales/te.json");
        insert_locale!("mr", "../locales/mr.json");
        insert_locale!("gu", "../locales/gu.json");
        insert_locale!("pa", "../locales/pa.json");
        insert_locale!("ne", "../locales/ne.json");
        insert_locale!("fa", "../locales/fa.json");
        insert_locale!("ar", "../locales/ar.json");
        insert_locale!("he", "../locales/he.json");
        insert_locale!("tr", "../locales/tr.json");
        insert_locale!("az", "../locales/az.json");
        insert_locale!("ru", "../locales/ru.json");
        insert_locale!("uk", "../locales/uk.json");
        insert_locale!("pl", "../locales/pl.json");
        insert_locale!("cs", "../locales/cs.json");
        insert_locale!("sk", "../locales/sk.json");
        insert_locale!("hu", "../locales/hu.json");
        insert_locale!("ro", "../locales/ro.json");
        insert_locale!("bg", "../locales/bg.json");
        insert_locale!("sr", "../locales/sr.json");
        insert_locale!("hr", "../locales/hr.json");
        insert_locale!("el", "../locales/el.json");
        insert_locale!("de", "../locales/de.json");
        insert_locale!("fr", "../locales/fr.json");
        insert_locale!("es", "../locales/es.json");
        insert_locale!("pt_BR", "../locales/pt_BR.json");
        insert_locale!("pt_PT", "../locales/pt_PT.json");
        insert_locale!("it", "../locales/it.json");
        insert_locale!("nl", "../locales/nl.json");
        insert_locale!("sv", "../locales/sv.json");
        insert_locale!("da", "../locales/da.json");
        insert_locale!("nb", "../locales/nb.json");
        insert_locale!("fi", "../locales/fi.json");
        insert_locale!("sw", "../locales/sw.json");

        let messages_str = include_str!("../locales/messages.json").trim_start_matches('\u{feff}');
        let source_keys: Vec<String> = serde_json::from_str(messages_str)
            .expect("Failed to parse master messages.json source keys");

        Self {
            locales,
            source_keys,
        }
    }

    /// Load all embedded locales into a new `Catalog` instance.
    /// Convenience alias fulfilling the `PROJECT.md` interface contract.
    pub fn load_all() -> Self {
        Self::load_embedded()
    }

    /// Parse a single locale translation mapping from a JSON string.
    /// Resilient against leading UTF-8 Byte Order Marks (`\u{feff}`).
    pub fn from_json(json_str: &str) -> Result<HashMap<String, String>, serde_json::Error> {
        let clean = json_str.trim_start_matches('\u{feff}');
        serde_json::from_str(clean)
    }

    /// Load a single locale translation mapping from an external JSON file on disk.
    /// Resilient against leading UTF-8 Byte Order Marks (`\u{feff}`).
    pub fn load_file<P: AsRef<std::path::Path>>(
        path: P,
    ) -> greenc_core::Result<HashMap<String, String>> {
        let content = std::fs::read_to_string(path)?;
        let map = Self::from_json(&content)?;
        Ok(map)
    }

    /// Register or replace a locale's translation mapping in the catalog.
    pub fn add_locale(&mut self, locale: &str, map: HashMap<String, String>) {
        self.locales.insert(locale.to_string(), map);
    }

    pub fn get_source_keys(&self) -> Vec<String> {
        self.source_keys.clone()
    }

    pub fn get_raw(&self, locale: &str, key: &str) -> Option<&str> {
        self.locales
            .get(locale)
            .and_then(|map| map.get(key))
            .map(|s| s.as_str())
    }

    pub fn tr(&self, locale: &str, key: &str, args: &[(&str, &str)]) -> String {
        // Fallback hierarchy:
        // 1. Target locale
        // 2. Fallback language ("en")
        // 3. Key itself (Vietnamese source)
        let template = self
            .get_raw(locale, key)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                self.get_raw(FALLBACK_LANGUAGE, key)
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or(key);

        interpolate(template, args)
    }

    pub fn get_translations_for_locale(&self, locale: &str) -> HashMap<String, String> {
        self.locales
            .get(locale)
            .cloned()
            .or_else(|| self.locales.get(FALLBACK_LANGUAGE).cloned())
            .unwrap_or_default()
    }
}
