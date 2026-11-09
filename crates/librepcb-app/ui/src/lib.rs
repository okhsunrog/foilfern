//! The Slint UI of the LibrePCB application: upstream's
//! `libs/librepcb/ui/**` (see `PROVENANCE.md` next to this crate's
//! `ui.slint`), compiled by `slint-build`.
//!
//! This crate contains only the generated code (the `AppWindow` component,
//! the `Data`, `Backend` and `EditorCommandSet` globals and all structs and
//! enums of `api/*.slint`). It is separate from `librepcb-app` because the
//! generated code is large (tens of MB of Rust): keeping it in its own crate
//! means backend changes do not recompile it.

#[allow(unsafe_code, clippy::all, missing_docs, unused)]
mod generated {
    slint::include_modules!();
}

pub use generated::*;
