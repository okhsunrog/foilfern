//! Port of libs/librepcb/core/library/pkg (packages: pads, 3D models,
//! footprints and the package check).
//!
//! Like the [`geometry`](crate::geometry) objects, all types are plain data;
//! upstream `onEdited` signals are marked with `// upstream: emits ...`
//! notes. [`FootprintPad`] embeds a [`Pad`](crate::geometry::Pad) (upstream
//! derives from it), accessible with [`FootprintPad::pad()`] and
//! [`FootprintPad::pad_mut()`].
//!
//! Not ported (UI specific, to be implemented in the rendering layer):
//! `FootprintPainter`.

mod check_area;
mod error;
mod footprint;
mod footprint_pad;
mod package;
mod package_check;
mod package_check_messages;
mod package_model;
mod package_pad;

pub use error::Error;
pub use footprint::{Footprint, FootprintList, FootprintListTag};
pub use footprint_pad::{FootprintPad, FootprintPadList, FootprintPadListTag};
pub use package::{AlternativeName, AssemblyType, Package};
pub use package_check::run_package_checks;
pub use package_check_messages::{FootprintRef, PackageCheckMessage, PadRef, WidthObject};
pub use package_model::{PackageModel, PackageModelList, PackageModelListTag};
pub use package_pad::{PackagePad, PackagePadList, PackagePadListTag};
