//! Port of libs/librepcb/core/geometry (geometric primitives of symbols,
//! footprints, schematics and boards).
//!
//! All objects are plain data: fields are private, getters return values
//! (or references for non-`Copy` data), and setters return whether the value
//! changed. Upstream emits `onEdited` signals from the setters; these are not
//! ported yet (change tracking will be designed together with the editor
//! architecture), see the `// upstream: emits onEdited(...)` notes.
//!
//! Upstream copy-constructors taking a new UUID (`Circle(uuid, other)`) are
//! replaced by `with_uuid()`, and `operator=` by [`Clone`]. List types are
//! [`SerializableObjectList`](crate::serialization::SerializableObjectList)
//! aliases, e.g. [`CircleList`].
//!
//! Not ported (UI specific, to be implemented in the rendering layer):
//! `toQPainterPathPx()` of paths, vias and pad geometries, and the image
//! loading (`Image::tryLoad()`) which depends on `QImage`/`QSvgRenderer`.

mod circle;
mod error;
mod hole;
mod image;
mod junction;
mod net_label;
mod net_line;
mod pad;
mod pad_geometry;
mod pad_hole;
mod path;
mod polygon;
mod stroke_text;
mod text;
mod trace;
mod vertex;
mod via;
mod zone;

pub use circle::{Circle, CircleList, CircleListTag};
pub use error::Error;
pub use hole::{Hole, HoleList, HoleListTag};
pub use image::{Image, ImageList, ImageListTag};
pub use junction::{Junction, JunctionList, JunctionListTag};
pub use net_label::{NetLabel, NetLabelList, NetLabelListTag};
pub use net_line::{NetLine, NetLineAnchor, NetLineList, NetLineListTag};
pub use pad::{ComponentSide, Pad, PadFunction, PadShape};
pub use pad_geometry::{PadGeometry, PadGeometryShape};
pub use pad_hole::{PadHole, PadHoleList, PadHoleListTag};
pub use path::{NonEmptyPath, Path, StraightAreaPath};
pub use polygon::{Polygon, PolygonList, PolygonListTag};
pub use stroke_text::{StrokeText, StrokeTextList, StrokeTextListTag};
pub use text::{Text, TextList, TextListTag};
pub use trace::{Trace, TraceAnchor, TraceList, TraceListTag};
pub use vertex::Vertex;
pub use via::{Via, ViaList, ViaListTag};
pub use zone::{Zone, ZoneLayers, ZoneList, ZoneListTag, ZoneRules};

/// Generates a getter and a setter for a field.
///
/// `copy` getters return the value, `ref` getters a reference. The setter
/// returns whether the value was modified.
macro_rules! property {
    ($(#[$doc:meta])* copy $field:ident: $ty:ty, $setter:ident) => {
        $(#[$doc])*
        pub fn $field(&self) -> $ty {
            self.$field
        }

        #[doc = concat!("Sets [`", stringify!($field), "()`](Self::", stringify!($field),
            "), returns whether the value was modified.")]
        pub fn $setter(&mut self, value: $ty) -> bool {
            if value == self.$field {
                return false;
            }
            self.$field = value;
            // upstream: emits onEdited(...Changed)
            true
        }
    };
    ($(#[$doc:meta])* ref $field:ident: $ty:ty, $setter:ident) => {
        $(#[$doc])*
        pub fn $field(&self) -> &$ty {
            &self.$field
        }

        #[doc = concat!("Sets [`", stringify!($field), "()`](Self::", stringify!($field),
            "), returns whether the value was modified.")]
        pub fn $setter(&mut self, value: $ty) -> bool {
            if value == self.$field {
                return false;
            }
            self.$field = value;
            // upstream: emits onEdited(...Changed)
            true
        }
    };
}

/// Implements [`HasUuid`](crate::serialization::HasUuid), a `uuid()` getter
/// and `with_uuid()` for a type with a `uuid` field.
macro_rules! impl_uuid {
    ($ty:ident) => {
        impl $ty {
            /// Returns the UUID.
            pub fn uuid(&self) -> $crate::types::Uuid {
                self.uuid
            }

            /// Returns a copy of `self` with another UUID (upstream
            #[doc = concat!("`", stringify!($ty), "(uuid, other)`).")]
            pub fn with_uuid(&self, uuid: $crate::types::Uuid) -> Self {
                Self {
                    uuid,
                    ..self.clone()
                }
            }
        }

        impl $crate::serialization::HasUuid for $ty {
            fn uuid(&self) -> $crate::types::Uuid {
                self.uuid
            }
        }
    };
}

/// Defines the list tag type and the list alias of an element type.
macro_rules! object_list {
    ($(#[$doc:meta])* $list:ident, $tag:ident, $ty:ident, $tagname:literal) => {
        #[doc = concat!("Tag of [`", stringify!($list), "`] elements (`", $tagname, "`).")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $tag;

        impl $crate::serialization::ListTagName for $tag {
            const TAG_NAME: &'static str = $tagname;
        }

        $(#[$doc])*
        pub type $list = $crate::serialization::SerializableObjectList<$ty, $tag>;
    };
}

pub(crate) use impl_uuid;
pub(crate) use object_list;
pub(crate) use property;
