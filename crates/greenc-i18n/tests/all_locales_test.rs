use greenc_i18n::catalog::Catalog;
use greenc_i18n::format::{
    decimal_separator, extract_tokens, format_eta, format_size, interpolate, is_rtl,
    match_system_language,
};
use greenc_i18n::validator::validate_all_locales;

#[test]
fn test_all_50_locales_100_percent_parity() {
    let catalog = Catalog::load_embedded();
    let issues = validate_all_locales(&catalog);

    if !issues.is_empty() {
        for issue in &issues {
            eprintln!("[{}] Key '{}': {}", issue.locale, issue.key, issue.error);
        }
        panic!(
            "Found {} i18n validation issues across the 50 locales",
            issues.len()
        );
    }
}

#[test]
fn test_rtl_detection() {
    assert!(is_rtl("ar"));
    assert!(is_rtl("he"));
    assert!(is_rtl("fa"));
    assert!(is_rtl("ur"));
    assert!(!is_rtl("vi"));
    assert!(!is_rtl("en"));
}

#[test]
fn test_comma_decimal_formatting() {
    assert_eq!(decimal_separator("vi"), ',');
    assert_eq!(decimal_separator("de"), ',');
    assert_eq!(decimal_separator("fr"), ',');
    assert_eq!(decimal_separator("en"), '.');
    assert_eq!(decimal_separator("ja"), '.');

    assert_eq!(format_size(5_000_000, "vi"), "4,8 MB");
    assert_eq!(format_size(5_000_000, "en"), "4.8 MB");
    assert_eq!(format_size(1024, "vi"), "1 KB");
    assert_eq!(format_size(0, "en"), "0 B");
    assert_eq!(format_size(-1_500_000_000, "vi"), "-1,4 GB");
}

#[test]
fn test_format_eta() {
    assert_eq!(format_eta(45, "en"), "00:45");
    assert_eq!(format_eta(125, "vi"), "02:05");
    assert_eq!(format_eta(3665, "en"), "1:01:05");
}

#[test]
fn test_system_language_matching() {
    assert_eq!(match_system_language("vi_VN"), "vi");
    assert_eq!(match_system_language("zh-Hans-CN"), "zh_CN");
    assert_eq!(match_system_language("zh-Hant-TW"), "zh_TW");
    assert_eq!(match_system_language("pt-BR"), "pt_BR");
    assert_eq!(match_system_language("pt-PT"), "pt_PT");
    assert_eq!(match_system_language("no-NO"), "nb");
    assert_eq!(match_system_language("tl-PH"), "fil");
    assert_eq!(match_system_language("unknown-XX"), "en");
}

#[test]
fn test_token_extraction_and_unclosed_braces() {
    // Valid tokens
    assert_eq!(
        extract_tokens("Scanning {drive}: {path}"),
        vec!["drive", "path"]
    );
    assert_eq!(extract_tokens("{count:.0f} files remaining"), vec!["count"]);

    // Negative tests: Unclosed braces MUST be rejected
    assert_eq!(extract_tokens("{unclosed_token"), Vec::<String>::new());
    assert_eq!(
        extract_tokens("Quét ổ đĩa: {unclosed_token"),
        Vec::<String>::new()
    );
    assert_eq!(extract_tokens("Valid {valid} and {unclosed"), vec!["valid"]);

    // Negative tests: Empty braces MUST NOT be emitted
    assert_eq!(extract_tokens("{}"), Vec::<String>::new());
    assert_eq!(extract_tokens("Empty {} and {key}"), vec!["key"]);
}

#[test]
fn test_interpolation_unclosed_braces() {
    let args = [("drive", "C:"), ("count", "42")];
    // Closed token is interpolated
    assert_eq!(
        interpolate("Scanning {drive} now", &args),
        "Scanning C: now"
    );

    // Unclosed token is preserved verbatim, not mangled
    assert_eq!(
        interpolate("Scanning {drive unclosed", &args),
        "Scanning {drive unclosed"
    );
}

#[test]
fn test_catalog_from_json_resilience_to_bom() {
    let clean_json = r#"{"key1": "value1", "key2": "value2"}"#;
    let bom_json = format!("\u{feff}{}", clean_json);

    // Standard serde_json fails on BOM string
    let std_err = serde_json::from_str::<std::collections::HashMap<String, String>>(&bom_json);
    assert!(
        std_err.is_err(),
        "Standard serde_json must fail on BOM string"
    );

    // Catalog::from_json succeeds on clean JSON
    let map_clean =
        Catalog::from_json(clean_json).expect("Catalog::from_json failed on clean JSON");
    assert_eq!(map_clean.get("key1").unwrap(), "value1");

    // Catalog::from_json succeeds on BOM JSON
    let map_bom = Catalog::from_json(&bom_json).expect("Catalog::from_json failed on BOM JSON");
    assert_eq!(map_bom.get("key1").unwrap(), "value1");
    assert_eq!(map_bom.get("key2").unwrap(), "value2");
}

#[test]
fn test_catalog_load_file_resilience_to_bom() {
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("greenc_test_bom_{}.json", std::process::id()));

    let content_with_bom = "\u{feff}{\"test_key\": \"test_value\"}";
    std::fs::write(&temp_file, content_with_bom).expect("Failed to write temp BOM file");

    let map =
        Catalog::load_file(&temp_file).expect("Catalog::load_file failed to load file with BOM");
    assert_eq!(map.get("test_key").unwrap(), "test_value");

    let _ = std::fs::remove_file(temp_file);
}

#[test]
fn test_all_locale_files_byte_level_bom_absence() {
    let locales_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    let mut bom_files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(locales_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                if let Ok(bytes) = std::fs::read(&path) {
                    if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF
                    {
                        bom_files.push(path.file_name().unwrap().to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    assert!(
        bom_files.is_empty(),
        "Found locale files containing UTF-8 BOM: {:?}",
        bom_files
    );
}
