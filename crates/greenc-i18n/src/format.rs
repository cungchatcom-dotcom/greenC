use crate::catalog::{FALLBACK_LANGUAGE, LANGUAGES};

pub const RTL_LANGUAGES: &[&str] = &["ar", "he", "fa", "ur"];

pub fn is_rtl(locale: &str) -> bool {
    RTL_LANGUAGES.contains(&locale)
}

pub const COMMA_DECIMAL_LANGUAGES: &[&str] = &[
    "vi", "id", "tr", "az", "ru", "uk", "pl", "cs", "sk", "hu", "ro", "bg", "sr", "hr", "el", "de",
    "fr", "es", "pt_BR", "pt_PT", "it", "nl", "sv", "da", "nb", "fi", "fa",
];

pub fn decimal_separator(locale: &str) -> char {
    if COMMA_DECIMAL_LANGUAGES.contains(&locale) {
        ','
    } else {
        '.'
    }
}

pub fn interpolate(template: &str, args: &[(&str, &str)]) -> String {
    if args.is_empty() || !template.contains('{') {
        return template.to_string();
    }

    let mut result = String::with_capacity(template.len() + 32);
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut token = String::new();
            let mut closed = false;
            while let Some(&next_ch) = chars.peek() {
                chars.next();
                if next_ch == '}' {
                    closed = true;
                    break;
                }
                token.push(next_ch);
            }

            if closed && !token.is_empty() {
                // Strip optional format specifiers like :.0f
                let key = match token.find(':') {
                    Some(idx) => &token[..idx],
                    None => &token,
                };

                if let Some((_, val)) = args.iter().find(|(k, _)| *k == key) {
                    result.push_str(val);
                } else {
                    // Unknown key: preserve original token
                    result.push('{');
                    result.push_str(&token);
                    result.push('}');
                }
            } else {
                result.push('{');
                result.push_str(&token);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

pub fn extract_tokens(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut token = String::new();
            let mut closed = false;
            while let Some(&next_ch) = chars.peek() {
                chars.next();
                if next_ch == '}' {
                    closed = true;
                    break;
                }
                token.push(next_ch);
            }
            if closed && !token.is_empty() {
                let key = match token.find(':') {
                    Some(idx) => &token[..idx],
                    None => &token,
                };
                tokens.push(key.to_string());
            }
        }
    }
    tokens
}

pub fn match_system_language(sys_locale: &str) -> &'static str {
    let normalized = sys_locale.replace('-', "_");
    let parts: Vec<&str> = normalized.split('_').collect();
    let lang = parts[0].to_ascii_lowercase();

    if lang == "zh" {
        if parts
            .iter()
            .any(|p| matches!(*p, "Hant" | "TW" | "HK" | "MO"))
        {
            return "zh_TW";
        }
        return "zh_CN";
    }
    if lang == "pt" {
        if parts.contains(&"PT") {
            return "pt_PT";
        }
        return "pt_BR";
    }
    if matches!(lang.as_str(), "no" | "nn") {
        return "nb";
    }
    if lang == "tl" {
        return "fil";
    }

    for (code, _, _) in LANGUAGES {
        if *code == lang {
            return code;
        }
    }

    FALLBACK_LANGUAGE
}

pub fn format_size(num_bytes: i64, locale: &str) -> String {
    let sign = if num_bytes < 0 { "-" } else { "" };
    let mut val = (num_bytes.abs()) as f64;
    let units = ["B", "KB", "MB", "GB", "TB"];
    let dec_sep = decimal_separator(locale);

    for unit in units {
        if val < 1024.0 || unit == "TB" {
            return if matches!(unit, "B" | "KB") {
                format!("{}{:.0} {}", sign, val, unit)
            } else {
                let formatted = format!("{:.1}", val).replace('.', &dec_sep.to_string());
                format!("{sign}{formatted} {unit}")
            };
        }
        val /= 1024.0;
    }

    let formatted = format!("{:.1}", val).replace('.', &dec_sep.to_string());
    format!("{}{}{}", sign, formatted, " TB")
}

pub fn format_eta(seconds: u64, _locale: &str) -> String {
    if seconds >= 3600 {
        let h = seconds / 3600;
        let m = (seconds % 3600) / 60;
        let s = seconds % 60;
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        let m = seconds / 60;
        let s = seconds % 60;
        format!("{:02}:{:02}", m, s)
    }
}
