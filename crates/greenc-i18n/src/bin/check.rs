//! Standalone CLI validator for i18n catalog completeness.
use greenc_i18n::catalog::Catalog;
use greenc_i18n::validator::validate_all_locales;

fn main() {
    println!("Validating GreenC i18n localization catalogs across all 50 locales...");
    let catalog = Catalog::load_embedded();
    let issues = validate_all_locales(&catalog);

    if issues.is_empty() {
        println!("All 50 locales validated successfully with 100% parity (298 keys each)!");
    } else {
        eprintln!("Found {} validation issues:", issues.len());
        for issue in &issues {
            eprintln!("[{}] Key '{}': {}", issue.locale, issue.key, issue.error);
        }
        std::process::exit(1);
    }
}
