//! Every string the Svelte UI passes to `t(...)` must exist in the embedded catalog.
//!
//! `validate_all_locales` only checks that the 50 locale files agree with each other; it cannot
//! see keys the UI uses but nobody added. The UI does not fall back to English
//! (`translations[key] || key`), so a missing key shows the Vietnamese source text in every
//! other language. This test reads the UI sources directly to close that gap.

use greenc_i18n::catalog::Catalog;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "__tests__") {
                continue;
            }
            collect_files(&p, out);
        } else if p
            .extension()
            .is_some_and(|e| e == "svelte" || e == "ts")
        {
            out.push(p);
        }
    }
}

/// Reads a quoted string literal starting at `chars[start]` (the opening quote).
fn read_literal(chars: &[char], start: usize) -> Option<String> {
    let quote = chars[start];
    let mut s = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                // Decode like JavaScript: `t()` receives the decoded string at runtime.
                s.push(match chars[i + 1] {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                });
                i += 2;
            }
            c if c == quote => return Some(s),
            c => {
                s.push(c);
                i += 1;
            }
        }
    }
    None
}

/// Extracts literal keys from `t('...')`, `t("...")`, `` t(`...`) `` (without `${}`) and
/// `labelKey: '...'`.
fn extract_keys(src: &str) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut keys = Vec::new();
    let mut i = 0;
    while i + 2 < chars.len() {
        let is_t_call = chars[i] == 't'
            && chars[i + 1] == '('
            && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == '.'));
        if is_t_call {
            let mut j = i + 2;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && matches!(chars[j], '\'' | '"' | '`') {
                if let Some(lit) = read_literal(&chars, j) {
                    if !(chars[j] == '`' && lit.contains("${")) {
                        keys.push(lit);
                    }
                }
            }
        }
        i += 1;
    }
    let marker = "labelKey:";
    let mut rest = src;
    while let Some(pos) = rest.find(marker) {
        let after = rest[pos + marker.len()..].trim_start();
        let ac: Vec<char> = after.chars().collect();
        if !ac.is_empty() && matches!(ac[0], '\'' | '"') {
            if let Some(lit) = read_literal(&ac, 0) {
                keys.push(lit);
            }
        }
        rest = &rest[pos + marker.len()..];
    }
    keys
}

#[test]
fn extractor_handles_quotes_escapes_and_templates() {
    let src = r#"t('Xin chào') t("Ổ {drive}") at('no') t(`a ${x}`) t(`plain`) { labelKey: 'Thu gọn' } t('It\'s') t('a\n\nb') t('{d}:\\')"#;
    assert_eq!(
        extract_keys(src),
        vec!["Xin chào", "Ổ {drive}", "plain", "It's", "a\n\nb", "{d}:\\", "Thu gọn"]
    );
}

#[test]
fn every_ui_translation_key_exists_in_catalog() {
    let ui_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src");
    assert!(ui_src.exists(), "UI sources not found at {}", ui_src.display());

    let mut files = Vec::new();
    collect_files(&ui_src, &mut files);
    assert!(!files.is_empty());

    let catalog = Catalog::load_embedded();
    let en = catalog.get_translations_for_locale("en");

    let mut missing: BTreeMap<String, String> = BTreeMap::new();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        for key in extract_keys(&src) {
            if !en.contains_key(&key) {
                missing
                    .entry(key)
                    .or_insert_with(|| f.strip_prefix(&ui_src).unwrap_or(f).display().to_string());
            }
        }
    }

    if !missing.is_empty() {
        for (k, f) in &missing {
            eprintln!("{f}: {k}");
        }
        panic!(
            "{} UI string(s) are missing from the i18n catalog (see list above)",
            missing.len()
        );
    }
}
