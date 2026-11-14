//! Compiles the Slint UI (`ui.slint`, ported from upstream, see
//! `PROVENANCE.md`) with the translations of `lang/` bundled (see
//! `foilfern_i18n::build_support`).
//!
//! Set `SLINT_EMIT_DEBUG_INFO=1` at build time to embed the element
//! metadata that Slint's MCP server needs for introspection (slint-build
//! reruns this script when the variable changes).

use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    // Slint looks for `<dir>/<lang>/LC_MESSAGES/<CARGO_PKG_NAME>.po`, so the
    // shared `lang/*/LC_MESSAGES/librepcb.po` catalogs are staged under this
    // crate's name.
    let domain = std::env::var("CARGO_PKG_NAME").expect("set by cargo");
    let lang = foilfern_i18n::build_support::stage_for_slint(&out_dir, &domain)
        .expect("stage translations");
    // Upstream builds with the "cosmic-dark" style (cmake/FindSlint.cmake).
    let config = slint_build::CompilerConfiguration::new()
        .with_edition((1, 20))
        .with_style("cosmic-dark".into())
        .with_bundled_translations(lang);
    slint_build::compile_with_config("ui.slint", config).expect("compile ui.slint");
}
