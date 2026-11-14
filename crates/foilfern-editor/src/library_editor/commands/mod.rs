//! Library element commands (port of libs/librepcb/editor/library/cmd/*
//! and of the element editing logic of the library editor tabs): plain
//! parameter structs implementing [`ElementCommand`](super::ElementCommand).
//!
//! - [`metadata`]: names, version, author, categories, ... of any element.
//! - [`symbol`]: pins, polygons, circles, texts, images of symbols;
//!   transform, remove, copy and paste.
//! - [`package`]: package pads, footprints, footprint pads, polygons,
//!   circles, stroke texts, zones, holes; transform, remove, copy, paste,
//!   generate outline/courtyard.
//! - [`component`]: signals, symbol variants, gates, pin-signal-map.
//! - [`device`]: component/package, pad-signal-map (pinout), parts.
//! - [`transform`] and [`drag`]: the geometric transformations shared by
//!   symbols and footprints.

pub mod component;
pub mod device;
pub mod drag;
pub mod metadata;
pub mod package;
pub mod symbol;
pub mod transform;

pub use component::*;
pub use device::*;
pub use drag::{DragSelectedItems, ItemContainer, TransformOp};
pub use metadata::EditElementMetadata;
pub use package::*;
pub use symbol::*;
pub use transform::Transformable;
