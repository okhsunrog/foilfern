//! Matching of requested locales against the available catalogs.

/// Normalizes a POSIX (`de_CH.UTF-8@euro`) or BCP 47 (`de-CH`) locale into
/// lowercase BCP 47 subtags.
fn subtags(locale: &str) -> Vec<String> {
    let locale = locale.trim();
    let locale = locale.split(['.', '@']).next().unwrap_or_default();
    locale
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Outcome of resolving a locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// The untranslated source language (English).
    Source,
    /// Index into the available languages.
    Catalog(usize),
}

/// Resolves `locale` against `available` (catalog language tags), trying in
/// order:
///
/// 1. an exact match (case-insensitive, `_` and `-` equivalent),
/// 2. for Chinese, the variant matching the script/region
///    (`zh-Hant`, `zh-HK`, `zh-MO` → `zh-TW`; `zh-Hans`, `zh-SG` → `zh-CN`),
/// 3. successively shorter prefixes (`de-CH-1901` → `de-CH` → `de`),
/// 4. English (`en`, `en-GB`, ...) → the source language,
/// 5. any catalog of the same language (`pt-PT` → `pt-BR`).
///
/// `C`/`POSIX` and empty strings resolve to the source language.
pub(crate) fn resolve(locale: &str, available: &[&str]) -> Option<Resolved> {
    let tags = subtags(locale);
    let Some(lang) = tags.first() else {
        return Some(Resolved::Source);
    };
    if lang == "c" || lang == "posix" {
        return Some(Resolved::Source);
    }
    let find = |candidate: &[&str]| {
        available.iter().position(|a| {
            let a = subtags(a);
            a.len() == candidate.len() && a.iter().zip(candidate).all(|(x, y)| x == y)
        })
    };
    let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
    if let Some(i) = find(&tag_refs) {
        return Some(Resolved::Catalog(i));
    }
    if lang == "zh" {
        let rest = &tag_refs[1..];
        let traditional = rest
            .iter()
            .any(|t| matches!(*t, "hant" | "tw" | "hk" | "mo"));
        let simplified = rest.iter().any(|t| matches!(*t, "hans" | "cn" | "sg"));
        let candidates: &[&[&str]] = if traditional {
            &[&["zh", "tw"], &["zh", "hant"]]
        } else if simplified {
            &[&["zh", "cn"], &["zh", "hans"]]
        } else {
            &[]
        };
        if let Some(i) = candidates.iter().find_map(|c| find(c)) {
            return Some(Resolved::Catalog(i));
        }
        if traditional {
            // Don't fall back to simplified Chinese for traditional locales.
            return None;
        }
    }
    for len in (1..tag_refs.len()).rev() {
        if let Some(i) = find(&tag_refs[..len]) {
            return Some(Resolved::Catalog(i));
        }
    }
    if lang == "en" {
        return Some(Resolved::Source);
    }
    available
        .iter()
        .position(|a| subtags(a).first() == Some(lang))
        .map(Resolved::Catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVAILABLE: &[&str] = &["de", "gsw", "pt-BR", "ru", "zh-CN", "zh-Hans", "zh-TW"];

    fn r(locale: &str) -> Option<&'static str> {
        match resolve(locale, AVAILABLE)? {
            Resolved::Source => Some("en"),
            Resolved::Catalog(i) => Some(AVAILABLE[i]),
        }
    }

    #[test]
    fn resolution() {
        assert_eq!(r("de"), Some("de"));
        assert_eq!(r("de_CH.UTF-8"), Some("de"));
        assert_eq!(r("de-CH"), Some("de"));
        assert_eq!(r("DE-at"), Some("de"));
        assert_eq!(r("gsw-CH"), Some("gsw"));
        assert_eq!(r("pt_BR"), Some("pt-BR"));
        assert_eq!(r("pt-PT"), Some("pt-BR"));
        assert_eq!(r("pt"), Some("pt-BR"));
        assert_eq!(r("ru_RU.UTF-8"), Some("ru"));
        assert_eq!(r("ru-UA"), Some("ru"));
        assert_eq!(r("zh-CN"), Some("zh-CN"));
        assert_eq!(r("zh_CN.UTF-8"), Some("zh-CN"));
        assert_eq!(r("zh-Hans"), Some("zh-Hans"));
        assert_eq!(r("zh-Hans-CN"), Some("zh-CN"));
        assert_eq!(r("zh-SG"), Some("zh-CN"));
        assert_eq!(r("zh-Hant-TW"), Some("zh-TW"));
        assert_eq!(r("zh-HK"), Some("zh-TW"));
        assert_eq!(r("zh"), Some("zh-CN"));
        assert_eq!(r("en"), Some("en"));
        assert_eq!(r("en_US.UTF-8"), Some("en"));
        assert_eq!(r("en-GB"), Some("en"));
        assert_eq!(r("C"), Some("en"));
        assert_eq!(r("POSIX"), Some("en"));
        assert_eq!(r(""), Some("en"));
        assert_eq!(r("xx"), None);
        assert_eq!(resolve("zh-TW", &["zh-CN"]), None);
    }
}
