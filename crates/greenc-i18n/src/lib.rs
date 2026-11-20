//! In-memory 50-locale translation catalog, RTL text direction, and numeric formatting.

#![deny(unsafe_code)]

pub mod catalog;
pub mod format;
pub mod validator;

pub use catalog::{Catalog, FALLBACK_LANGUAGE, LANGUAGES, SOURCE_LANGUAGE};
pub use format::{
    decimal_separator, extract_tokens, format_eta, format_size, interpolate, is_rtl,
    match_system_language, COMMA_DECIMAL_LANGUAGES, RTL_LANGUAGES,
};
pub use validator::{validate_all_locales, ValidationIssue};

/// Top-level helper function to translate a key using the embedded global catalog.
pub fn tr(locale: &str, key: &str, args: &[(&str, &str)]) -> String {
    static CATALOG: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
    let catalog = CATALOG.get_or_init(Catalog::load_embedded);
    catalog.tr(locale, key, args)
}
