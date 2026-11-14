//! Port of libs/librepcb/core/types (basic value types).
//!
//! All types are small, validated value types: constrained upstream types
//! (`type_safe::constrained_type`) are newtypes with fallible constructors
//! (`new()`, [`TryFrom`], [`FromStr`](std::str::FromStr)), and upstream
//! singleton classes (layers, colors, roles) are `Copy` enums/handles.
//!
//! Additionally, [`Color`] replaces `QColor` where the file format needs it.

mod alignment;
mod angle;
mod bounded_unsigned_ratio;
mod bus_name;
mod circuit_identifier;
mod color;
mod element_name;
mod enums;
mod error;
mod file_proof_name;
mod layer;
mod length;
mod length_unit;
mod mask_config;
mod pcb_color;
mod point;
mod ratio;
mod signal_role;
mod simple_string;
pub(crate) mod string_newtype;
mod stroke_text_spacing;
mod tag;
mod uuid;
mod version;

pub use alignment::{Alignment, HAlign, VAlign};
pub use angle::{Angle, Angle3D};
pub use bounded_unsigned_ratio::BoundedUnsignedRatio;
pub use bus_name::BusName;
pub use circuit_identifier::CircuitIdentifier;
pub use color::Color;
pub use element_name::ElementName;
pub use enums::{AutoUpdateMode, GridStyle};
pub use error::Error;
pub use file_proof_name::FileProofName;
pub use layer::Layer;
pub use length::{Length, Point3D, PositiveLength, UnsignedLength};
pub use length_unit::LengthUnit;
pub use mask_config::MaskConfig;
pub use pcb_color::PcbColor;
pub use point::{Orientation, Point};
pub use ratio::{Ratio, UnsignedLimitedRatio, UnsignedRatio};
pub use signal_role::SignalRole;
pub use simple_string::SimpleString;
pub use stroke_text_spacing::StrokeTextSpacing;
pub use tag::Tag;
pub use uuid::Uuid;
pub use version::Version;
