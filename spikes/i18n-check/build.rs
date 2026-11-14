use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    // Slint looks for `<dir>/<lang>/LC_MESSAGES/<CARGO_PKG_NAME>.po`, so the
    // shared `lang/*/LC_MESSAGES/librepcb.po` catalogs are staged under this
    // crate's name.
    let domain = std::env::var("CARGO_PKG_NAME").expect("set by cargo");
    let lang = foilfern_i18n::build_support::stage_for_slint(&out_dir, &domain)
        .expect("stage translations");
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new()
            .with_edition((1, 20))
            .with_bundled_translations(lang),
    )
    .expect("compile ui/main.slint");
}
