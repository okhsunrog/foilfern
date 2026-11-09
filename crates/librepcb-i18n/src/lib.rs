//! Translation of user-visible strings from Rust code.
//!
//! Strings use the upstream Qt translation context (C++ class name) and
//! positional `{0}`, `{1}` placeholders, matching the `.po` catalogs in `lang/`
//! (generated from the upstream Qt `.ts` files by `tools/ts2po`).
//!
//! # Catalogs
//!
//! `build.rs` compiles `lang/<lang>/LC_MESSAGES/librepcb.po` into compact
//! blobs (finished translations only; fuzzy and untranslated entries are
//! dropped) that are embedded with `include_bytes!`. A catalog is decoded on
//! first use of its language into a hash map borrowing from the embedded data,
//! so lookups return `&'static str` without copying. This is pure Rust (no
//! libintl), works for every target, and costs nothing for languages that are
//! never selected.
//!
//! # Language selection
//!
//! The language defaults to the best match for the system's preferred locales
//! ([`sys_locale::get_locales`]) the first time a string is translated. It can
//! be changed at any time with [`set_language`]; the lookup chain is e.g.
//! `de_CH` → `de-CH` → `de` → English source text (see [`resolve_language`]).
//! All functions are thread-safe.
//!
//! # Keeping Slint in sync
//!
//! The same `.po` files are compiled into the UI by Slint's bundled
//! translations (`slint_build::CompilerConfiguration::with_bundled_translations`).
//! Slint looks for `<dir>/<lang>/LC_MESSAGES/<CARGO_PKG_NAME>.po`, where
//! `CARGO_PKG_NAME` is the crate that runs `slint_build`; use
//! [`build_support::stage_for_slint`] in that crate's `build.rs` to provide the
//! catalogs under the right name. Slint picks its own default language from
//! the system locale with a simpler matching algorithm, so the UI layer must
//! select the language explicitly — after creating the first component — and
//! again whenever it changes:
//!
//! ```ignore
//! let lang = librepcb_i18n::set_language(&settings.language)
//!     .unwrap_or_else(|_| librepcb_i18n::set_language_from_system());
//! let window = MainWindow::new()?; // Slint requires a component to exist first
//! slint::select_bundled_translation(lang)?; // "en" selects the source strings
//! ```
//!
//! Because both use the same language tags (directory names in `lang/`), the
//! value returned by [`set_language`]/[`current_language`] can be passed to
//! `slint::select_bundled_translation` unchanged.

mod catalog;
#[cfg(test)]
mod compile;
#[cfg(test)]
mod context_check;
mod language;
mod plural;

use std::borrow::Cow;
use std::fmt::{self, Display};
use std::sync::atomic::{AtomicUsize, Ordering};

pub use plural::{PluralForms, PluralFormsError};

use language::Resolved;

/// Tag of the source language, i.e. the untranslated strings.
pub const SOURCE_LANGUAGE: &str = "en";

/// [`CURRENT`] value before the language was initialized.
const UNINITIALIZED: usize = usize::MAX;
/// [`CURRENT`] value for the source language.
const SOURCE: usize = usize::MAX - 1;

/// Index of the current language into [`catalog::LANGUAGES`], or one of the
/// special values above.
static CURRENT: AtomicUsize = AtomicUsize::new(UNINITIALIZED);

/// Error returned by [`set_language`] for languages without a catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownLanguage(pub String);

impl Display for UnknownLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no translation available for language {:?}", self.0)
    }
}

impl std::error::Error for UnknownLanguage {}

fn tag_of(index: usize) -> &'static str {
    catalog::LANGUAGES
        .get(index)
        .copied()
        .unwrap_or(SOURCE_LANGUAGE)
}

fn to_index(resolved: Resolved) -> usize {
    match resolved {
        Resolved::Source => SOURCE,
        Resolved::Catalog(i) => i,
    }
}

/// Language tags of all embedded catalogs (e.g. `"de"`, `"pt-BR"`,
/// `"zh-CN"`), sorted. The source language [`SOURCE_LANGUAGE`] is always
/// available in addition.
pub fn available_languages() -> &'static [&'static str] {
    &catalog::LANGUAGES
}

/// Resolves a locale (`de_CH.UTF-8`, `pt-PT`, `zh-Hant-TW`, ...) to the tag
/// of the best available catalog, or [`SOURCE_LANGUAGE`] for English/`C`.
///
/// Returns `None` if nothing matches.
pub fn resolve_language(locale: &str) -> Option<&'static str> {
    language::resolve(locale, available_languages()).map(|r| tag_of(to_index(r)))
}

/// Selects the language for subsequent translations and returns the tag of
/// the catalog actually used (see [`resolve_language`]).
///
/// On error, the current language is left unchanged.
pub fn set_language(locale: &str) -> Result<&'static str, UnknownLanguage> {
    let resolved = language::resolve(locale, available_languages())
        .ok_or_else(|| UnknownLanguage(locale.to_owned()))?;
    let index = to_index(resolved);
    CURRENT.store(index, Ordering::Release);
    Ok(tag_of(index))
}

/// Selects the best language for the system's preferred locales, falling
/// back to the source language, and returns its tag.
pub fn set_language_from_system() -> &'static str {
    let index = system_language();
    CURRENT.store(index, Ordering::Release);
    tag_of(index)
}

fn system_language() -> usize {
    sys_locale::get_locales()
        .find_map(|l| language::resolve(&l, available_languages()))
        .map_or(SOURCE, to_index)
}

fn current_index() -> usize {
    let index = CURRENT.load(Ordering::Acquire);
    if index != UNINITIALIZED {
        return index;
    }
    let detected = system_language();
    // Keep a language set concurrently by another thread.
    match CURRENT.compare_exchange(UNINITIALIZED, detected, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => detected,
        Err(other) => other,
    }
}

/// Tag of the currently selected language ([`SOURCE_LANGUAGE`] if
/// untranslated). Initializes it from the system locale if not yet set.
pub fn current_language() -> &'static str {
    tag_of(current_index())
}

fn current_catalog() -> Option<&'static catalog::Catalog> {
    match current_index() {
        SOURCE => None,
        index => catalog::catalog(index),
    }
}

/// Translates `msgid` within `context` into the currently selected language.
///
/// Falls back to `msgid` if no translation is available.
pub fn translate(context: &str, msgid: &str) -> Cow<'static, str> {
    current_catalog()
        .and_then(|c| c.lookup(context, msgid))
        .and_then(|forms| forms.first())
        .map_or_else(|| Cow::Owned(msgid.to_owned()), |s| Cow::Borrowed(*s))
}

/// Translates a message with plural forms for count `n`.
///
/// The catalog entry is looked up by `(context, singular)`. Without a
/// translation, `singular` is returned for `n == 1` and `plural` otherwise.
pub fn translate_plural(context: &str, singular: &str, plural: &str, n: u64) -> Cow<'static, str> {
    let translated = current_catalog().and_then(|c| {
        let forms = c.lookup(context, singular)?;
        forms
            .get(c.plural.index(n))
            .or_else(|| forms.first().filter(|_| forms.len() == 1))
    });
    match translated {
        Some(s) => Cow::Borrowed(*s),
        None if n == 1 => Cow::Owned(singular.to_owned()),
        None => Cow::Owned(plural.to_owned()),
    }
}

/// Converts a count to the `u64` used for plural selection (used by [`trn!`]).
/// Negative counts select the form for `u64::MAX`.
#[doc(hidden)]
pub fn plural_count<T: TryInto<u64>>(n: T) -> u64 {
    n.try_into().unwrap_or(u64::MAX)
}

fn format_impl(template: &str, args: &[&dyn Display], count: Option<&dyn Display>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();
    // Next argument for `{}` (auto-numbered like in Slint's `@tr`).
    let mut next_auto = 0;
    while let Some((i, c)) = chars.next() {
        match c {
            '{' if chars.peek().is_some_and(|&(_, n)| n == '{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek().is_some_and(|&(_, n)| n == '}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let rest = &template[i + 1..];
                let arg = rest.find('}').and_then(|end| {
                    let name = &rest[..end];
                    let value = match (name, count) {
                        ("n", Some(count)) => count,
                        ("", _) => {
                            next_auto += 1;
                            *args.get(next_auto - 1)?
                        }
                        _ => *args.get(name.parse::<usize>().ok()?)?,
                    };
                    Some((end, value))
                });
                match arg {
                    Some((end, arg)) => {
                        out.push_str(&arg.to_string());
                        for _ in 0..end + 1 {
                            chars.next();
                        }
                    }
                    None => out.push('{'),
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Substitutes positional `{0}`, `{1}`, ... placeholders in `template`.
///
/// `{}` takes the next argument (as in Slint's `@tr`). `{{` and `}}` produce
/// literal braces. Unknown placeholders are kept as-is.
pub fn format_positional(template: &str, args: &[&dyn Display]) -> String {
    format_impl(template, args, None)
}

/// Like [`format_positional`], additionally replacing `{n}` with `count`.
pub fn format_plural(template: &str, count: &dyn Display, args: &[&dyn Display]) -> String {
    format_impl(template, args, Some(count))
}

/// Translates a string and substitutes positional placeholders.
///
/// The result is always passed through [`format_positional`], so `{{`/`}}`
/// in the message yield literal braces even without arguments (same as
/// Slint's `@tr`).
///
/// ```
/// use librepcb_i18n::tr;
/// # librepcb_i18n::set_language("en").unwrap();
/// let key = "foo";
/// assert_eq!(
///     tr!("AttributeKey", "Invalid attribute key: '{0}'", key),
///     "Invalid attribute key: 'foo'"
/// );
/// ```
#[macro_export]
macro_rules! tr {
    ($context:expr, $msgid:expr $(,)?) => {
        $crate::format_positional(&$crate::translate($context, $msgid), &[])
    };
    ($context:expr, $msgid:expr, $($arg:expr),+ $(,)?) => {
        $crate::format_positional(
            &$crate::translate($context, $msgid),
            &[$(&$arg as &dyn ::std::fmt::Display),+],
        )
    };
}

/// Translates a message with plural forms and substitutes `{n}` (the count)
/// and positional `{0}`, `{1}`, ... placeholders.
///
/// For strings converted from Qt's `tr(source, disambiguation, n)`, there is
/// only one source string: pass it as both `singular` and `plural`.
///
/// ```
/// use librepcb_i18n::trn;
/// # librepcb_i18n::set_language("en").unwrap();
/// let n = 3;
/// assert_eq!(
///     trn!("Ctx", "{n} file in {0}", "{n} files in {0}", n, "lib"),
///     "3 files in lib"
/// );
/// ```
#[macro_export]
macro_rules! trn {
    ($context:expr, $singular:expr, $plural:expr, $n:expr $(, $arg:expr)* $(,)?) => {{
        let n = $n;
        $crate::format_plural(
            &$crate::translate_plural($context, $singular, $plural, $crate::plural_count(n)),
            &n,
            &[$(&$arg as &dyn ::std::fmt::Display),*],
        )
    }};
}

/// Helpers for build scripts of crates that compile `.slint` files.
pub mod build_support {
    use std::io;
    use std::path::{Path, PathBuf};

    /// File name (gettext domain) of the catalogs in `lang/`.
    pub const DOMAIN: &str = "librepcb";

    /// The `lang/` directory of the workspace.
    pub fn lang_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lang")
    }

    /// Copies `lang/<lang>/LC_MESSAGES/librepcb.po` to
    /// `<out_dir>/lang/<lang>/LC_MESSAGES/<domain>.po` and returns
    /// `<out_dir>/lang`, suitable for
    /// `slint_build::CompilerConfiguration::with_bundled_translations`.
    ///
    /// Slint uses the name of the crate running `slint_build` as the
    /// domain, so pass `env!("CARGO_PKG_NAME")` (or
    /// `std::env::var("CARGO_PKG_NAME")`). Emits `cargo::rerun-if-changed`
    /// for the source catalogs.
    pub fn stage_for_slint(out_dir: &Path, domain: &str) -> io::Result<PathBuf> {
        let src = lang_dir();
        let dst = out_dir.join("lang");
        println!("cargo::rerun-if-changed={}", src.display());
        for entry in std::fs::read_dir(&src)? {
            let entry = entry?;
            let po = entry
                .path()
                .join("LC_MESSAGES")
                .join(format!("{DOMAIN}.po"));
            if po.is_file() {
                println!("cargo::rerun-if-changed={}", po.display());
                let target_dir = dst.join(entry.file_name()).join("LC_MESSAGES");
                std::fs::create_dir_all(&target_dir)?;
                // Copy only if changed: slint-build reruns the build script
                // when the staged files are newer than its last run, so
                // rewriting them would rebuild the UI crate every time.
                let target = target_dir.join(format!("{domain}.po"));
                let content = std::fs::read(&po)?;
                if std::fs::read(&target).ok().as_ref() != Some(&content) {
                    std::fs::write(&target, content)?;
                }
            }
        }
        Ok(dst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Tests that change the global language must not run concurrently.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn positional_formatting() {
        assert_eq!(format_positional("{1} {0}", &[&"a", &2]), "2 a");
        assert_eq!(format_positional("{{0}} {0}", &[&"x"]), "{0} x");
        assert_eq!(format_positional("{5} {x}", &[&"x"]), "{5} {x}");
        assert_eq!(format_positional("{n} {0}", &[&"x"]), "{n} x");
        assert_eq!(format_positional("{} {} {0} {}", &[&"a", &"b"]), "a b a {}");
        assert_eq!(format_plural("{n} {0} {{n}}", &5, &[&"x"]), "5 x {n}");
    }

    #[test]
    fn catalogs_are_embedded() {
        let langs = available_languages();
        assert!(langs.contains(&"de"), "{langs:?}");
        assert!(langs.contains(&"pt-BR"), "{langs:?}");
        assert!(!langs.contains(&"en"), "{langs:?}");
        let mut total = 0;
        for (i, lang) in langs.iter().enumerate() {
            let c = catalog::catalog(i).unwrap();
            assert!(c.plural.nplurals() >= 1, "{lang}");
            total += c.len();
            if *lang == "de" {
                assert!(c.len() > 1000, "de has only {} entries", c.len());
            }
        }
        assert!(total > 20_000, "{total}");
    }

    #[test]
    fn translate_german() {
        let _guard = LOCK.lock().unwrap();
        assert_eq!(set_language("de_DE.UTF-8"), Ok("de"));
        assert_eq!(current_language(), "de");
        assert_eq!(translate("ArchiveOutputJob", "Archive"), "Archiv");
        assert_eq!(
            tr!("AttributeKey", "Invalid attribute key: '{0}'", "x"),
            "Ungültiger Attribut-Schlüssel: 'x'"
        );
        // Untranslated / unknown strings fall back to the source.
        assert_eq!(translate("NoSuchContext", "Archive"), "Archive");
        assert_eq!(translate("ArchiveOutputJob", "Nope {{x}}"), "Nope {{x}}");
        assert_eq!(tr!("ArchiveOutputJob", "Nope {{x}}"), "Nope {x}");

        assert_eq!(set_language("en_US"), Ok("en"));
        assert_eq!(translate("ArchiveOutputJob", "Archive"), "Archive");
        assert_eq!(set_language("xx"), Err(UnknownLanguage("xx".into())));
        assert_eq!(current_language(), "en");
    }

    #[test]
    fn plurals() {
        let _guard = LOCK.lock().unwrap();
        let ctx = "librepcb::editor::LibrariesModel";
        let src = "Update available for {n} libraries";
        set_language("ru").unwrap();
        assert_eq!(
            trn!(ctx, src, src, 1),
            "Обновление доступно для 1 библиотеки"
        );
        assert_eq!(
            trn!(ctx, src, src, 5),
            "Обновление доступно для 5 библиотек"
        );
        assert_eq!(
            trn!(ctx, src, src, 22),
            "Обновление доступно для 22 библиотек"
        );
        set_language("de").unwrap();
        let slint = "{n} warning(s)";
        assert_eq!(
            trn!("LibraryRuleCheckLink", slint, slint, 1usize),
            "1 Warnung"
        );
        assert_eq!(
            trn!("LibraryRuleCheckLink", slint, slint, 2u32),
            "2 Warnungen"
        );
        set_language("en").unwrap();
        assert_eq!(trn!("X", "{n} file", "{n} files", 1), "1 file");
        assert_eq!(
            trn!("X", "{n} file in {0}", "{n} files in {0}", 0, "a"),
            "0 files in a"
        );
    }

    #[test]
    fn thread_safety() {
        let _guard = LOCK.lock().unwrap();
        set_language("de").unwrap();
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| translate("ArchiveOutputJob", "Archive").into_owned()))
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap(), "Archiv");
        }
    }
}
