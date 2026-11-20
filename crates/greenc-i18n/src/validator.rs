use crate::catalog::{Catalog, LANGUAGES, SOURCE_LANGUAGE};
use crate::format::extract_tokens;

#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub locale: String,
    pub key: String,
    pub error: String,
}

pub fn validate_all_locales(catalog: &Catalog) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let source_keys = catalog.get_source_keys();

    for (code, _, _) in LANGUAGES {
        if *code == SOURCE_LANGUAGE {
            continue;
        }

        for key in &source_keys {
            match catalog.get_raw(code, key) {
                Some(translated) => {
                    if translated.trim().is_empty() {
                        issues.push(ValidationIssue {
                            locale: code.to_string(),
                            key: key.clone(),
                            error: "Translation is empty".to_string(),
                        });
                        continue;
                    }

                    // Token parity check
                    let src_tokens = extract_tokens(key);
                    let dst_tokens = extract_tokens(translated);
                    for tok in &src_tokens {
                        if !dst_tokens.contains(tok) {
                            issues.push(ValidationIssue {
                                locale: code.to_string(),
                                key: key.clone(),
                                error: format!(
                                    "Missing token {{{}}} in translation: '{}'",
                                    tok, translated
                                ),
                            });
                        }
                    }
                }
                None => {
                    issues.push(ValidationIssue {
                        locale: code.to_string(),
                        key: key.clone(),
                        error: "Missing translation key".to_string(),
                    });
                }
            }
        }
    }

    issues
}
