//! Language codes and plural rules.
//!
//! The `.ts` files come from Transifex, which stores `<numerusform>`s in CLDR
//! category order (`one`, `few`, `many`, `other`, skipping categories the
//! language doesn't have) — *not* in the order of Qt's own numerus rules
//! (`qttools/src/linguist/shared/numerus.cpp`) that `QTranslator` uses to
//! pick a form at runtime. We therefore interpret the forms as CLDR
//! categories and emit a gettext `Plural-Forms` expression that selects the
//! right CLDR category for integer `n`, dropping categories that only apply to
//! fractional numbers.
//!
//! Differences to Qt's runtime behavior (where Qt was actually wrong):
//!
//! | lang        | .ts forms (CLDR)          | Qt rule (numerus.cpp)        | effect in Qt              |
//! |-------------|---------------------------|------------------------------|---------------------------|
//! | cs, sk      | one, few, many, other     | 3 forms: 1 / 2–4 / else      | "else" showed `many` (fractions) instead of `other` |
//! | es, it      | one, many, other          | 2 forms: n==1 / else         | plural showed `many` (millions) instead of `other` |
//! | fr, pt_BR   | one, many, other          | 2 forms: n<=1 / else         | plural showed `many` instead of `other` |
//! | hu, tr      | one, other                | 1 form (no plural)           | always showed `one`       |
//! | gsw         | one, other                | unknown language → no rules  | always showed `one`       |
//! | lo          | other                     | English-style, 2 forms       | out of range → first form |
//! | pl, ru, uk  | one, few, many, other     | 3 forms, same as CLDR ints   | ok (`other` is fractions only) |
//! | ro, is, de… | as CLDR                   | same                         | ok                        |

/// Plural configuration of a language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluralRule {
    /// Number of `<numerusform>`s Transifex writes (CLDR categories).
    pub ts_forms: usize,
    /// For each gettext plural index, the `.ts` form index to use.
    pub ts_form_for_po_index: &'static [usize],
    /// The gettext `Plural-Forms` header value.
    pub plural_forms: &'static str,
}

const ONE_FORM: PluralRule = PluralRule {
    ts_forms: 1,
    ts_form_for_po_index: &[0],
    plural_forms: "nplurals=1; plural=0;",
};

const ENGLISH: PluralRule = PluralRule {
    ts_forms: 2,
    ts_form_for_po_index: &[0, 1],
    plural_forms: "nplurals=2; plural=(n != 1);",
};

const ICELANDIC: PluralRule = PluralRule {
    ts_forms: 2,
    ts_form_for_po_index: &[0, 1],
    plural_forms: "nplurals=2; plural=(n%10==1 && n%100!=11 ? 0 : 1);",
};

/// es, it: CLDR one (n=1), many (n≠0 ∧ n mod 10⁶ = 0), other.
const SPANISH: PluralRule = PluralRule {
    ts_forms: 3,
    ts_form_for_po_index: &[0, 1, 2],
    plural_forms: "nplurals=3; plural=(n == 1 ? 0 : n != 0 && n % 1000000 == 0 ? 1 : 2);",
};

/// fr, pt: CLDR one (n=0,1), many (n≠0 ∧ n mod 10⁶ = 0), other.
const FRENCH: PluralRule = PluralRule {
    ts_forms: 3,
    ts_form_for_po_index: &[0, 1, 2],
    plural_forms: "nplurals=3; plural=(n == 0 || n == 1 ? 0 : n % 1000000 == 0 ? 1 : 2);",
};

const ROMANIAN: PluralRule = PluralRule {
    ts_forms: 3,
    ts_form_for_po_index: &[0, 1, 2],
    plural_forms: "nplurals=3; plural=(n == 1 ? 0 : (n == 0 || (n%100 > 0 && n%100 < 20)) ? 1 : 2);",
};

/// cs, sk: CLDR one, few, many (fractions only), other → skip `many`.
const CZECH: PluralRule = PluralRule {
    ts_forms: 4,
    ts_form_for_po_index: &[0, 1, 3],
    plural_forms: "nplurals=3; plural=(n == 1 ? 0 : (n >= 2 && n <= 4) ? 1 : 2);",
};

/// pl: CLDR one, few, many, other (fractions only) → skip `other`.
const POLISH: PluralRule = PluralRule {
    ts_forms: 4,
    ts_form_for_po_index: &[0, 1, 2],
    plural_forms: "nplurals=3; plural=(n == 1 ? 0 : n%10 >= 2 && n%10 <= 4 && (n%100 < 12 || n%100 > 14) ? 1 : 2);",
};

/// ru, uk: CLDR one, few, many, other (fractions only) → skip `other`.
const RUSSIAN: PluralRule = PluralRule {
    ts_forms: 4,
    ts_form_for_po_index: &[0, 1, 2],
    plural_forms: "nplurals=3; plural=(n%10 == 1 && n%100 != 11 ? 0 : n%10 >= 2 && n%10 <= 4 && (n%100 < 12 || n%100 > 14) ? 1 : 2);",
};

/// Returns the plural rule for a normalized language tag.
pub fn plural_rule(tag: &str) -> Option<PluralRule> {
    let lang = tag.split('-').next().unwrap_or(tag);
    Some(match lang {
        "id" | "ja" | "jv" | "ko" | "lo" | "zh" | "vi" | "th" | "ms" => ONE_FORM,
        "de" | "en" | "eo" | "gsw" | "sv" | "hu" | "tr" | "nl" | "da" | "nb" | "fi" | "et"
        | "el" | "bg" | "ca" => ENGLISH,
        "is" => ICELANDIC,
        "es" | "it" => SPANISH,
        "fr" | "pt" => FRENCH,
        "ro" => ROMANIAN,
        "cs" | "sk" => CZECH,
        "pl" => POLISH,
        "ru" | "uk" | "be" => RUSSIAN,
        _ => return None,
    })
}

/// Normalizes a Qt/POSIX language code to the BCP 47 tag used as directory
/// name in `lang/`.
///
/// * `_` → `-` (`sys-locale` reports BCP 47 tags such as `pt-BR`, and Slint
///   first tries an exact match against the directory name, then a match on
///   the part before the first `-`/`_`/`@`).
/// * The region is dropped where it is the only/primary one for the
///   language (`ru_RU` → `ru`, `uk_UA` → `uk`, `ko_KR` → `ko`), so that e.g.
///   `ru-UA` still matches.
/// * Regions/scripts that distinguish variants are kept (`pt-BR`, `zh-CN`,
///   `zh-TW`, `zh-Hans`).
pub fn normalize_language(code: &str) -> String {
    let code = code.replace('_', "-");
    let mut parts = code.split('-');
    let lang = parts.next().unwrap_or_default().to_ascii_lowercase();
    let rest: Vec<String> = parts
        .map(|p| match p.len() {
            // Script subtag: title case.
            4 => {
                let mut s = p.to_ascii_lowercase();
                s[..1].make_ascii_uppercase();
                s
            }
            // Region subtag: upper case.
            _ => p.to_ascii_uppercase(),
        })
        .collect();
    /// Language/region pairs where the region adds no information.
    const PRIMARY_REGIONS: &[(&str, &str)] = &[
        ("de", "DE"),
        ("fr", "FR"),
        ("ja", "JP"),
        ("ko", "KR"),
        ("ru", "RU"),
        ("uk", "UA"),
    ];
    let redundant_region = match rest.as_slice() {
        [region] => PRIMARY_REGIONS.contains(&(lang.as_str(), region.as_str())),
        _ => false,
    };
    if rest.is_empty() || redundant_region {
        lang
    } else {
        format!("{lang}-{}", rest.join("-"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codes() {
        assert_eq!(normalize_language("de"), "de");
        assert_eq!(normalize_language("ru_RU"), "ru");
        assert_eq!(normalize_language("uk_UA"), "uk");
        assert_eq!(normalize_language("ko_KR"), "ko");
        assert_eq!(normalize_language("pt_BR"), "pt-BR");
        assert_eq!(normalize_language("zh_CN"), "zh-CN");
        assert_eq!(normalize_language("zh_TW"), "zh-TW");
        assert_eq!(normalize_language("zh-Hans"), "zh-Hans");
        assert_eq!(normalize_language("zh_hans"), "zh-Hans");
        assert_eq!(normalize_language("en_US"), "en-US");
        assert_eq!(normalize_language("gsw"), "gsw");
    }

    /// Straightforward CLDR integer plural categories, as index into the
    /// `.ts` forms Transifex writes.
    fn cldr_ts_index(lang: &str, n: u64) -> usize {
        let (m10, m100) = (n % 10, n % 100);
        match lang {
            "ja" | "zh-CN" | "ko" | "id" | "lo" => 0,
            "de" | "hu" | "tr" | "sv" | "eo" => usize::from(n != 1),
            "is" => usize::from(!(m10 == 1 && m100 != 11)),
            "es" | "it" => {
                if n == 1 {
                    0
                } else if n != 0 && n.is_multiple_of(1_000_000) {
                    1
                } else {
                    2
                }
            }
            "fr" | "pt-BR" => {
                if n <= 1 {
                    0
                } else if n.is_multiple_of(1_000_000) {
                    1
                } else {
                    2
                }
            }
            "ro" => {
                if n == 1 {
                    0
                } else if n == 0 || (1..=19).contains(&m100) {
                    1
                } else {
                    2
                }
            }
            "cs" | "sk" => match n {
                1 => 0,
                2..=4 => 1,
                _ => 3,
            },
            "pl" => {
                if n == 1 {
                    0
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    1
                } else {
                    2
                }
            }
            "ru" | "uk" => {
                if m10 == 1 && m100 != 11 {
                    0
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    1
                } else {
                    2
                }
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn plural_rules_select_cldr_forms() {
        for lang in [
            "ja", "zh-CN", "ko", "id", "lo", "de", "hu", "tr", "sv", "eo", "is", "es", "it", "fr",
            "pt-BR", "ro", "cs", "sk", "pl", "ru", "uk",
        ] {
            let rule = plural_rule(lang).unwrap();
            let nplurals = rule.ts_form_for_po_index.len();
            assert!(
                rule.plural_forms
                    .starts_with(&format!("nplurals={nplurals};")),
                "{lang}"
            );
            for n in (0..300).chain([1000, 1001, 1_000_000, 2_000_000, 1_000_001]) {
                let po_index = librepcb_plural(rule.plural_forms, n);
                assert!(po_index < nplurals, "{lang} n={n}");
                assert_eq!(
                    rule.ts_form_for_po_index[po_index],
                    cldr_ts_index(lang, n),
                    "{lang} n={n}"
                );
            }
        }
    }

    /// Evaluates the header with the runtime's evaluator.
    fn librepcb_plural(header: &str, n: u64) -> usize {
        foilfern_i18n::PluralForms::parse(header)
            .expect("valid plural expression")
            .index(n)
    }
}
