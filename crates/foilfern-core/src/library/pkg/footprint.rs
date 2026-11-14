//! Port of libs/librepcb/core/library/pkg/footprint.{h,cpp}.
//!
//! Differences to upstream: tags and 3D model references are ordered sets
//! (upstream `QSet`s, sorted on serialization), and the bounding rectangle
//! is computed with the `QPainterPath` emulation of
//! [`utils::painter_path`](crate::utils::painter_path).

use std::collections::BTreeSet;

use super::FootprintPadList;
use crate::geometry::{
    CircleList, HoleList, Path, PolygonList, StrokeTextList, ZoneList, impl_uuid, object_list,
    property,
};
use crate::serialization::{
    self, DeserializeObject, List, LocalizedDescriptionMap, LocalizedNameMap, SExpression,
    SerializeObject,
};
use crate::types::{Angle3D, ElementName, Layer, Point, Point3D, PositiveLength, Tag, Uuid};
use crate::utils::painter_path;

/// One footprint variant of a package.
///
/// The UUID and the footprint pads (their UUIDs, neither adding nor removing
/// pads) are part of the package's interface and must never be changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    uuid: Uuid,
    names: LocalizedNameMap,
    descriptions: LocalizedDescriptionMap,
    tags: BTreeSet<Tag>,
    model_position: Point3D,
    model_rotation: Angle3D,
    models: BTreeSet<Uuid>,
    pads: FootprintPadList,
    polygons: PolygonList,
    circles: CircleList,
    stroke_texts: StrokeTextList,
    zones: ZoneList,
    holes: HoleList,
}

// Top-level package aggregate (maps, sets, object lists).
static_assertions::assert_impl_all!(Footprint: Send, Sync);

/// Generates a getter and a mutable getter of an object list or map.
macro_rules! list_accessors {
    ($(#[$doc:meta])* $field:ident, $field_mut:ident: $ty:ty) => {
        $(#[$doc])*
        pub fn $field(&self) -> &$ty {
            &self.$field
        }

        #[doc = concat!("Returns [`", stringify!($field), "()`](Self::", stringify!($field),
            ") for modification.")]
        // upstream: modifications emit onEdited(...Edited)
        pub fn $field_mut(&mut self) -> &mut $ty {
            &mut self.$field
        }
    };
}

impl Footprint {
    /// Creates an empty footprint with the given default (en_US) name and
    /// description.
    pub fn new(uuid: Uuid, name_en_us: ElementName, description_en_us: String) -> Self {
        Self {
            uuid,
            names: LocalizedNameMap::new(name_en_us),
            descriptions: LocalizedDescriptionMap::new(description_en_us),
            tags: BTreeSet::new(),
            model_position: Point3D::default(),
            model_rotation: Angle3D::default(),
            models: BTreeSet::new(),
            pads: FootprintPadList::new(),
            polygons: PolygonList::new(),
            circles: CircleList::new(),
            stroke_texts: StrokeTextList::new(),
            zones: ZoneList::new(),
            holes: HoleList::new(),
        }
    }

    list_accessors!(
        /// Returns the localized names.
        names, names_mut: LocalizedNameMap
    );
    list_accessors!(
        /// Returns the localized descriptions.
        descriptions, descriptions_mut: LocalizedDescriptionMap
    );
    property!(
        /// Returns the tags.
        ref tags: BTreeSet<Tag>, set_tags
    );
    property!(
        /// Returns the 3D model position `(x, y, z)`.
        copy model_position: Point3D, set_model_position
    );
    property!(
        /// Returns the 3D model rotation `(x, y, z)`.
        copy model_rotation: Angle3D, set_model_rotation
    );
    property!(
        /// Returns the UUIDs of the package's 3D models used by this
        /// footprint.
        ref models: BTreeSet<Uuid>, set_models
    );
    list_accessors!(
        /// Returns the pads.
        pads, pads_mut: FootprintPadList
    );
    list_accessors!(
        /// Returns the polygons.
        polygons, polygons_mut: PolygonList
    );
    list_accessors!(
        /// Returns the circles.
        circles, circles_mut: CircleList
    );
    list_accessors!(
        /// Returns the stroke texts.
        stroke_texts, stroke_texts_mut: StrokeTextList
    );
    list_accessors!(
        /// Returns the keepout zones.
        zones, zones_mut: ZoneList
    );
    list_accessors!(
        /// Returns the (non-plated) holes.
        holes, holes_mut: HoleList
    );

    /// Returns the bounding rectangle `(bottom_left, top_right)` of the
    /// footprint's outline: the courtyard polygons (only if `with_courtyard`
    /// is set), or else the package outlines, or else the documentation, or
    /// else the legend polygons. `(origin, origin)` if none of these exist.
    pub fn calculate_bounding_rect(&self, with_courtyard: bool) -> (Point, Point) {
        let layer_paths = |layers: [Layer; 2]| {
            let mut paths = Vec::new();
            for layer in layers {
                for polygon in self.polygons.iter().filter(|p| p.layer() == layer) {
                    let path = polygon.path_for_rendering();
                    match PositiveLength::new(*polygon.line_width()) {
                        Ok(width) => paths.extend(path.to_outline_strokes(width)),
                        Err(_) => paths.push(path),
                    }
                }
            }
            paths
        };

        // First, take only courtyard layers into account.
        let mut paths: Vec<Path> = Vec::new();
        if with_courtyard {
            paths = layer_paths([Layer::TOP_COURTYARD, Layer::BOT_COURTYARD]);
        }
        // Fall back to package outlines, then documentation, then legend.
        for layers in [
            [Layer::TOP_PACKAGE_OUTLINES, Layer::BOT_PACKAGE_OUTLINES],
            [Layer::TOP_DOCUMENTATION, Layer::BOT_DOCUMENTATION],
            [Layer::TOP_LEGEND, Layer::BOT_LEGEND],
        ] {
            if !painter_path::is_empty_px(&paths) {
                break;
            }
            paths.extend(layer_paths(layers));
        }

        let rect = painter_path::bounding_rect_px(&paths);
        // The rectangle is built from coordinates of valid points, so the
        // conversion back cannot overflow.
        let bottom_left = Point::from_px(rect.left(), rect.bottom()).unwrap_or_default();
        let top_right = Point::from_px(rect.right(), rect.top()).unwrap_or_default();
        (bottom_left, top_right)
    }
}

impl_uuid!(Footprint);

object_list!(
    /// List of [`Footprint`]s.
    FootprintList,
    FootprintListTag,
    Footprint,
    "footprint"
);

impl SerializeObject for Footprint {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        self.names.serialize(root);
        root.ensure_line_break();
        self.descriptions.serialize(root);
        root.ensure_line_break();
        for tag in &self.tags {
            root.append_child("tag", tag);
            root.ensure_line_break();
        }
        self.model_position
            .serialize(root.append_list("3d_position"));
        self.model_rotation
            .serialize(root.append_list("3d_rotation"));
        root.ensure_line_break();
        for uuid in &self.models {
            root.append_child("3d_model", uuid);
            root.ensure_line_break();
        }
        self.pads.serialize(root);
        root.ensure_line_break();
        self.polygons.serialize(root);
        root.ensure_line_break();
        self.circles.serialize(root);
        root.ensure_line_break();
        self.stroke_texts.serialize(root);
        root.ensure_line_break();
        self.zones.serialize(root);
        root.ensure_line_break();
        self.holes.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for Footprint {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            names: LocalizedNameMap::deserialize(node)?,
            descriptions: LocalizedDescriptionMap::deserialize(node)?,
            tags: node
                .children_named("tag")
                .map(|child| child.child_value("@0"))
                .collect::<serialization::Result<_>>()?,
            model_position: Point3D::deserialize(node.required_child("3d_position")?)?,
            model_rotation: Angle3D::deserialize(node.required_child("3d_rotation")?)?,
            models: node
                .children_named("3d_model")
                .map(|child| child.child_value("@0"))
                .collect::<serialization::Result<_>>()?,
            pads: FootprintPadList::deserialize(node)?,
            polygons: PolygonList::deserialize(node)?,
            circles: CircleList::deserialize(node)?,
            stroke_texts: StrokeTextList::deserialize(node)?,
            zones: ZoneList::deserialize(node)?,
            holes: HoleList::deserialize(node)?,
        })
    }
}
