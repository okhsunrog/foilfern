//! Embedded, lazily decoded translation catalogs.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

use crate::plural::PluralForms;

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// Translations of one language, borrowing from the embedded data.
#[derive(Debug)]
pub(crate) struct Catalog {
    pub(crate) plural: PluralForms,
    /// context → msgid → forms (one for singular entries).
    entries: HashMap<&'static str, HashMap<&'static str, Box<[&'static str]>>>,
}

impl Catalog {
    /// Decodes a blob produced by `build.rs`. Returns `None` if malformed,
    /// which can only happen if `build.rs` and this code disagree.
    fn decode(mut data: &'static [u8]) -> Option<Self> {
        fn next(data: &mut &'static [u8]) -> Option<&'static str> {
            let (len, rest) = data.split_first_chunk::<4>()?;
            let len = usize::try_from(u32::from_le_bytes(*len)).ok()?;
            let (s, rest) = (rest.get(..len)?, rest.get(len..)?);
            *data = rest;
            std::str::from_utf8(s).ok()
        }
        let plural = PluralForms::parse(next(&mut data)?).ok()?;
        let mut entries: HashMap<&str, HashMap<&str, Box<[&str]>>> = HashMap::new();
        while !data.is_empty() {
            let context = next(&mut data)?;
            let msgid = next(&mut data)?;
            let count: usize = next(&mut data)?.parse().ok()?;
            let forms = (0..count)
                .map(|_| next(&mut data))
                .collect::<Option<Box<[_]>>>()?;
            entries.entry(context).or_default().insert(msgid, forms);
        }
        Some(Self { plural, entries })
    }

    /// All forms of the translation of `msgid` in `context`.
    pub(crate) fn lookup(&self, context: &str, msgid: &str) -> Option<&[&'static str]> {
        self.entries.get(context)?.get(msgid).map(|f| &**f)
    }

    /// Number of translated messages.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.values().map(HashMap::len).sum()
    }
}

/// Language tags of all embedded catalogs, sorted.
pub(crate) static LANGUAGES: LazyLock<Box<[&'static str]>> =
    LazyLock::new(|| CATALOG_DATA.iter().map(|(tag, _)| *tag).collect());

static CATALOGS: LazyLock<Box<[OnceLock<Catalog>]>> =
    LazyLock::new(|| CATALOG_DATA.iter().map(|_| OnceLock::new()).collect());

/// Returns the catalog at `index` into [`LANGUAGES`], decoding it on first use.
pub(crate) fn catalog(index: usize) -> Option<&'static Catalog> {
    let cell = CATALOGS.get(index)?;
    Some(cell.get_or_init(|| {
        let (_, data) = CATALOG_DATA[index];
        // Decoding only fails if build.rs and `decode` disagree on the format.
        Catalog::decode(data).unwrap_or_else(|| Catalog {
            plural: PluralForms::english(),
            entries: HashMap::new(),
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(po: &str) -> Catalog {
        let compiled = crate::compile::compile(po).unwrap();
        Catalog::decode(Box::leak(compiled.blob.into_boxed_slice())).unwrap()
    }

    #[test]
    fn only_finished_translations_are_kept() {
        let c = compile(
            r#"# header comment
msgid ""
msgstr ""
"Language: ru\n"
"Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\n"

#: a.cpp:1
msgctxt "Ctx"
msgid "Done"
msgstr "Готово"

#, fuzzy
msgctxt "Ctx"
msgid "Fuzzy"
msgstr "Нечётко"

msgctxt "Ctx"
msgid "Untranslated"
msgstr ""

msgctxt "Ctx"
msgid ""
"Multi\n"
"line"
msgstr ""
"Много\n"
"строк"

msgctxt "Ctx"
msgid "{n} file"
msgid_plural "{n} files"
msgstr[0] "{n} файл"
msgstr[1] "{n} файла"
msgstr[2] "{n} файлов"

msgctxt "Ctx"
msgid "{n} partial"
msgid_plural "{n} partial"
msgstr[0] "{n} частично"
msgstr[1] ""
msgstr[2] ""

msgid "No context"
msgstr "Без контекста"

#~ msgctxt "Ctx"
#~ msgid "Obsolete"
#~ msgstr "Устарело"
"#,
        );
        assert_eq!(c.plural.nplurals(), 3);
        assert_eq!(c.lookup("Ctx", "Done"), Some(&["Готово"][..]));
        assert_eq!(c.lookup("Ctx", "Fuzzy"), None);
        assert_eq!(c.lookup("Ctx", "Untranslated"), None);
        assert_eq!(c.lookup("Ctx", "Multi\nline"), Some(&["Много\nстрок"][..]));
        assert_eq!(
            c.lookup("Ctx", "{n} file"),
            Some(&["{n} файл", "{n} файла", "{n} файлов"][..])
        );
        assert_eq!(c.lookup("Ctx", "{n} partial"), None);
        assert_eq!(c.lookup("", "No context"), Some(&["Без контекста"][..]));
        assert_eq!(c.lookup("Ctx", "Obsolete"), None);
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn wrong_plural_count_warns() {
        let compiled = crate::compile::compile(
            "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=3; plural=n%3;\\n\"\n\n\
             msgid \"a\"\nmsgid_plural \"b\"\nmsgstr[0] \"x\"\nmsgstr[1] \"y\"\n",
        )
        .unwrap();
        assert_eq!(
            compiled.warnings,
            vec!["\"a\" has 2 plural forms, expected 3".to_owned()]
        );
    }

    #[test]
    fn malformed_po_is_rejected() {
        assert!(crate::compile::compile("msgid \"x\"\nmsgstr oops\n").is_err());
        assert!(
            crate::compile::compile(
                "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=2; plural=n+;\\n\"\n"
            )
            .is_err()
        );
    }
}
