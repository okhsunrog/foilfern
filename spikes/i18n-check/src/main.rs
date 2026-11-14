//! Checks that the `lang/` catalogs work with Slint's bundled translations
//! and give the same results as `foilfern_i18n`.

use foilfern_i18n::{tr, trn};

slint::include_modules!();

fn check(label: &str, slint: slint::SharedString, expected: &str) {
    println!("  {label:<14} {slint:?}");
    assert_eq!(slint.as_str(), expected, "{label}");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainMenuBar::new()?;

    // Every catalog known to foilfern_i18n must be selectable in Slint under
    // the same tag.
    for lang in foilfern_i18n::available_languages() {
        slint::select_bundled_translation(lang)?;
    }

    for (lang, count) in [
        ("de", 1),
        ("de", 2),
        ("ru", 1),
        ("ru", 3),
        ("ru", 5),
        ("en", 2),
    ] {
        let tag = foilfern_i18n::set_language(lang)?;
        slint::select_bundled_translation(tag)?;
        ui.set_count(count);
        println!("{tag} (count = {count}):");

        // Slint and foilfern_i18n must agree on every string.
        let file = tr!("MainMenuBar", "File");
        let undo = tr!("MainMenuBar", "Undo: {}", "X");
        let choose = tr!("DeviceDependencyCard", "Choose {}", "Symbol");
        let w = "{n} warning(s)";
        let warnings = trn!("LibraryRuleCheckLink", w, w, count);
        let m = "Apply {n} Modification(s)";
        let modifications = trn!("LibrariesPanel", m, m, count);
        check("file", ui.get_file(), &file);
        check("undo", ui.get_undo(), &undo);
        check("choose", ui.get_choose(), &choose);
        check("warnings", ui.get_warnings(), &warnings);
        check("modifications", ui.get_modifications(), &modifications);
        check("untranslated", ui.get_untranslated(), "Not translated 42");

        // And match the known translations.
        match (tag, count) {
            ("de", 1) => {
                assert_eq!(file, "Datei");
                assert_eq!(ui.get_undo(), "Verwerfe: X");
                assert_eq!(ui.get_choose(), "Wähle Symbol");
                assert_eq!(warnings, "1 Warnung");
                assert_eq!(modifications, "1 Änderung anwenden");
            }
            ("de", 2) => {
                assert_eq!(warnings, "2 Warnungen");
                assert_eq!(modifications, "2 Änderungen anwenden");
            }
            ("ru", 1) => assert_eq!(modifications, "Применить 1 модификацию"),
            ("ru", 3) => assert_eq!(modifications, "Применить 3 модификации"),
            ("ru", 5) => assert_eq!(modifications, "Применить 5 модификаций"),
            ("en", _) => {
                assert_eq!(file, "File");
                assert_eq!(ui.get_undo(), "Undo: X");
                assert_eq!(warnings, "2 warning(s)");
            }
            _ => unreachable!(),
        }
    }
    println!("OK");
    Ok(())
}
