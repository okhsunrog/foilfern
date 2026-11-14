//! Port of libs/librepcb/core/geometry/pad.{h,cpp}.
//!
//! Upstream `Pad` is the base class of footprint and board pads, which add
//! their own attributes and serialization. Here it is a plain struct meant
//! to be embedded (e.g. in
//! [`FootprintPad`](crate::library::pkg::FootprintPad)). It implements
//! deserialization of the common attributes, and
//! [`Pad::serialize_with()`] writes them with the attributes of the
//! embedding type (both upstream `serialize()` implementations only differ
//! in one line).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use foilfern_i18n::tr;

use super::{Error, PadGeometry, PadHoleList, Path, property};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::types::{
    Angle, Layer, Length, MaskConfig, Point, PositiveLength, Ratio, UnsignedLength,
    UnsignedLimitedRatio, Uuid,
};

/// Generates a token enum with string conversion and (de)serialization.
macro_rules! token_enum {
    ($(#[$meta:meta])* $name:ident, $err:path, { $($(#[$vmeta:meta])* $variant:ident => $token:literal,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)*
        }

        impl $name {
            /// All values.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// Returns the serialization token.
            pub fn to_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $token,)*
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.to_str())
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self, Error> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|v| v.to_str() == s)
                    .ok_or_else(|| $err(s.to_owned()))
            }
        }

        impl ToSExpression for $name {
            fn to_sexpression(&self) -> SExpression {
                SExpression::token(self.to_str())
            }
        }

        impl FromSExpression for $name {
            fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
                Ok(node.value()?.parse()?)
            }
        }

        // Serde: the file format token.
        crate::utils::serde_string::serde_string!($name);
    };
}

token_enum!(
    /// Shape of a [`Pad`].
    PadShape, Error::UnknownPadShape, {
        /// Rectangle with rounded corners.
        RoundedRect => "roundrect",
        /// Octagon with rounded corners.
        RoundedOctagon => "octagon",
        /// Custom outline.
        Custom => "custom",
    }
);

token_enum!(
    /// Board side of an SMT pad.
    ComponentSide, Error::UnknownPadComponentSide, {
        /// Top side.
        Top => "top",
        /// Bottom side.
        Bottom => "bottom",
    }
);

token_enum!(
    /// Function of a [`Pad`] (upstream `Pad::Function`).
    PadFunction, Error::UnknownPadFunction, {
        /// Not specified.
        Unspecified => "unspecified",
        /// Standard pad (soldered).
        StandardPad => "standard",
        /// Press-fit pad (THT, soldered).
        PressFitPad => "pressfit",
        /// Thermal pad (SMT, soldered).
        ThermalPad => "thermal",
        /// BGA pad (SMT, soldered).
        BgaPad => "bga",
        /// Edge connector pad (SMT, no soldering).
        EdgeConnectorPad => "edge_connector",
        /// Test pad (SMT, no soldering).
        TestPad => "test",
        /// Local footprint fiducial (SMT, no soldering).
        LocalFiducial => "local_fiducial",
        /// Global board fiducial (SMT, no soldering).
        GlobalFiducial => "global_fiducial",
    }
);

impl PadFunction {
    /// Returns the translated description.
    pub fn description_tr(self) -> String {
        match self {
            Self::Unspecified => tr!("Pad", "Not Specified"),
            Self::StandardPad => tr!("Pad", "Standard Pad (soldered)"),
            Self::PressFitPad => tr!("Pad", "Press-Fit Pad (THT, soldered)"),
            Self::ThermalPad => tr!("Pad", "Thermal Pad (SMT, soldered)"),
            Self::BgaPad => tr!("Pad", "BGA Pad (SMT, soldered)"),
            Self::EdgeConnectorPad => tr!("Pad", "Edge Connector Pad (SMT, no soldering)"),
            Self::TestPad => tr!("Pad", "Test Pad (SMT, no soldering)"),
            Self::LocalFiducial => tr!("Pad", "Local Footprint Fiducial (SMT, no soldering)"),
            Self::GlobalFiducial => tr!("Pad", "Global Board Fiducial (SMT, no soldering)"),
        }
    }

    /// Returns whether the pad is a fiducial.
    pub fn is_fiducial(self) -> bool {
        matches!(self, Self::LocalFiducial | Self::GlobalFiducial)
    }

    /// Returns whether the pad needs to be soldered.
    pub fn needs_soldering(self) -> bool {
        !matches!(
            self,
            Self::EdgeConnectorPad | Self::TestPad | Self::LocalFiducial | Self::GlobalFiducial
        )
    }
}

/// The common attributes of footprint pads and board pads.
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pad {
    uuid: Uuid,
    position: Point,
    rotation: Angle,
    shape: PadShape,
    width: PositiveLength,
    height: PositiveLength,
    radius: UnsignedLimitedRatio,
    /// Empty if not needed; implicitly closed.
    custom_shape_outline: Path,
    stop_mask_config: MaskConfig,
    solder_paste_config: MaskConfig,
    copper_clearance: UnsignedLength,
    component_side: ComponentSide,
    function: PadFunction,
    /// If not empty, it's a THT pad.
    holes: PadHoleList,
}

// Top-level geometry aggregate (paths, object lists, value types).
static_assertions::assert_impl_all!(Pad: Send, Sync);

impl Pad {
    /// Creates a pad.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        uuid: Uuid,
        position: Point,
        rotation: Angle,
        shape: PadShape,
        width: PositiveLength,
        height: PositiveLength,
        radius: UnsignedLimitedRatio,
        custom_shape_outline: Path,
        stop_mask_config: MaskConfig,
        solder_paste_config: MaskConfig,
        copper_clearance: UnsignedLength,
        component_side: ComponentSide,
        function: PadFunction,
        holes: PadHoleList,
    ) -> Self {
        Self {
            uuid,
            position,
            rotation,
            shape,
            width,
            height,
            radius,
            custom_shape_outline,
            stop_mask_config,
            solder_paste_config,
            copper_clearance,
            component_side,
            function,
            holes,
        }
    }

    property!(
        /// Returns the UUID.
        copy uuid: Uuid, set_uuid
    );
    property!(
        /// Returns the position.
        copy position: Point, set_position
    );
    property!(
        /// Returns the rotation.
        copy rotation: Angle, set_rotation
    );
    property!(
        /// Returns the shape.
        copy shape: PadShape, set_shape
    );
    property!(
        /// Returns the width.
        copy width: PositiveLength, set_width
    );
    property!(
        /// Returns the height.
        copy height: PositiveLength, set_height
    );
    property!(
        /// Returns the corner radius (relative to the size).
        copy radius: UnsignedLimitedRatio, set_radius
    );
    property!(
        /// Returns the custom shape outline (empty if not needed, implicitly
        /// closed).
        ref custom_shape_outline: Path, set_custom_shape_outline
    );
    property!(
        /// Returns the stop mask configuration.
        copy stop_mask_config: MaskConfig, set_stop_mask_config
    );
    property!(
        /// Returns the solder paste configuration.
        copy solder_paste_config: MaskConfig, set_solder_paste_config
    );
    property!(
        /// Returns the copper clearance.
        copy copper_clearance: UnsignedLength, set_copper_clearance
    );
    property!(
        /// Returns the component side (for SMT pads).
        copy component_side: ComponentSide, set_component_side
    );
    property!(
        /// Returns the function.
        copy function: PadFunction, set_function
    );
    property!(
        /// Returns the holes (THT pad if not empty).
        ref holes: PadHoleList, set_holes
    );

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self {
            uuid,
            ..self.clone()
        }
    }

    /// Returns whether the pad is a fiducial.
    pub fn function_is_fiducial(&self) -> bool {
        self.function.is_fiducial()
    }

    /// Returns whether the pad needs to be soldered.
    pub fn function_needs_soldering(&self) -> bool {
        self.function.needs_soldering()
    }

    /// Returns whether it is a THT pad (has holes).
    pub fn is_tht(&self) -> bool {
        !self.holes.is_empty()
    }

    /// Returns whether the pad has copper on the given layer.
    pub fn is_on_layer(&self, layer: Layer) -> bool {
        if self.is_tht() {
            layer.is_copper()
        } else {
            layer == self.smt_layer()
        }
    }

    /// Returns the copper layer of an SMT pad.
    pub fn smt_layer(&self) -> Layer {
        match self.component_side {
            ComponentSide::Bottom => Layer::BOT_COPPER,
            ComponentSide::Top => Layer::TOP_COPPER,
        }
    }

    /// Returns whether the pad has copper on the top layer.
    pub fn has_top_copper(&self) -> bool {
        self.is_tht() || (self.component_side == ComponentSide::Top)
    }

    /// Returns whether the pad has copper on the bottom layer.
    pub fn has_bottom_copper(&self) -> bool {
        self.is_tht() || (self.component_side == ComponentSide::Bottom)
    }

    /// Returns whether the pad has an automatic top stop mask opening.
    pub fn has_auto_top_stop_mask(&self) -> bool {
        self.stop_mask_config.is_enabled() && self.has_top_copper()
    }

    /// Returns whether the pad has an automatic bottom stop mask opening.
    pub fn has_auto_bottom_stop_mask(&self) -> bool {
        self.stop_mask_config.is_enabled() && self.has_bottom_copper()
    }

    /// Returns whether the pad has automatic top solder paste (SMT pads on
    /// top, THT pads on the opposite side of the component).
    pub fn has_auto_top_solder_paste(&self) -> bool {
        self.solder_paste_config.is_enabled()
            && (self.is_tht() != (self.component_side == ComponentSide::Top))
    }

    /// Returns whether the pad has automatic bottom solder paste.
    pub fn has_auto_bottom_solder_paste(&self) -> bool {
        self.solder_paste_config.is_enabled()
            && (self.is_tht() != (self.component_side == ComponentSide::Bottom))
    }

    /// Returns the copper geometry.
    pub fn geometry(&self) -> PadGeometry {
        match self.shape {
            PadShape::RoundedRect => {
                PadGeometry::rounded_rect(self.width, self.height, self.radius, self.holes.clone())
            }
            PadShape::RoundedOctagon => PadGeometry::rounded_octagon(
                self.width,
                self.height,
                self.radius,
                self.holes.clone(),
            ),
            PadShape::Custom => {
                PadGeometry::custom(self.custom_shape_outline.clone(), self.holes.clone())
            }
        }
    }

    /// Returns the geometries per layer for previews (library editor), with
    /// the stop mask and solder paste offsets of the default design rules.
    pub fn build_preview_geometries(&self) -> BTreeMap<Layer, Vec<PadGeometry>> {
        let geometry = self.geometry();
        let stop_mask_offset = self
            .stop_mask_config
            .offset()
            .unwrap_or(Length::new(100_000));
        let solder_paste_offset = self
            .solder_paste_config
            .offset()
            .unwrap_or_else(|| calc_auto_solder_paste_offset(self.shape, self.width, self.height));
        let mut geometries = BTreeMap::new();
        if self.has_top_copper() {
            geometries.insert(Layer::TOP_COPPER, vec![geometry.clone()]);
        }
        if self.has_auto_top_stop_mask() {
            geometries.insert(
                Layer::TOP_STOP_MASK,
                vec![geometry.with_offset(stop_mask_offset)],
            );
        }
        if self.has_auto_top_solder_paste() {
            geometries.insert(
                Layer::TOP_SOLDER_PASTE,
                vec![geometry.with_offset(-solder_paste_offset)],
            );
        }
        if self.has_bottom_copper() {
            geometries.insert(Layer::BOT_COPPER, vec![geometry.clone()]);
        }
        if self.has_auto_bottom_stop_mask() {
            geometries.insert(
                Layer::BOT_STOP_MASK,
                vec![geometry.with_offset(stop_mask_offset)],
            );
        }
        if self.has_auto_bottom_solder_paste() {
            geometries.insert(
                Layer::BOT_SOLDER_PASTE,
                vec![geometry.with_offset(-solder_paste_offset)],
            );
        }
        geometries
    }

    /// Returns the recommended corner radius: 50%, but at most 0.25mm (as
    /// recommended by IPC7351C), rounded down to whole percents.
    pub fn recommended_radius(
        width: PositiveLength,
        height: PositiveLength,
    ) -> UnsignedLimitedRatio {
        let size = width.min(height);
        let max_radius = Ratio::from_normalized(0.5 / size.to_mm());
        // Round down to whole percents (truncating integer division).
        let one_percent = Ratio::from_percent(1).to_ppm();
        let max_radius = Ratio::new(max_radius.to_ppm() / one_percent * one_percent);
        let clamped = max_radius.clamp(Ratio::from_percent(0), Ratio::from_percent(50));
        UnsignedLimitedRatio::new(clamped).expect("clamped to 0..50%")
    }

    /// Serializes the pad into `root` (upstream `FootprintPad::serialize()`
    /// and `BoardPadData::serialize()`).
    ///
    /// `derived` appends the attributes of the embedding type (e.g.
    /// `(package_pad ...)` or `(lock ...)`) on their own line, between the
    /// common attributes and the custom shape outline.
    pub fn serialize_with(&self, root: &mut List, derived: impl FnOnce(&mut List)) {
        root.append_value(&self.uuid);
        root.append_child("side", &self.component_side);
        root.append_child("shape", &self.shape);
        root.ensure_line_break();
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        Point::new(*self.width, *self.height).serialize(root.append_list("size"));
        root.append_child("radius", &self.radius);
        root.ensure_line_break();
        root.append_child("stop_mask", &self.stop_mask_config);
        root.append_child("solder_paste", &self.solder_paste_config);
        root.append_child("clearance", &self.copper_clearance);
        root.append_child("function", &self.function);
        root.ensure_line_break();
        derived(root);
        root.ensure_line_break();
        self.custom_shape_outline.serialize(root);
        root.ensure_line_break();
        self.holes.serialize(root);
        root.ensure_line_break();
    }
}

/// Solder paste offset calculated the same way as in the board (with the
/// default design rules).
fn calc_auto_solder_paste_offset(
    shape: PadShape,
    width: PositiveLength,
    height: PositiveLength,
) -> Length {
    let size = if shape == PadShape::Custom {
        Length::ZERO
    } else {
        *width.min(height)
    };
    size.scaled(0.1).clamp(Length::ZERO, Length::new(1_000_000))
}

impl DeserializeObject for Pad {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
            rotation: node.child_value("rotation/@0")?,
            shape: node.child_value("shape/@0")?,
            width: node.child_value("size/@0")?,
            height: node.child_value("size/@1")?,
            radius: node.child_value("radius/@0")?,
            custom_shape_outline: Path::deserialize(node)?,
            stop_mask_config: node.child_value("stop_mask/@0")?,
            solder_paste_config: node.child_value("solder_paste/@0")?,
            copper_clearance: node.child_value("clearance/@0")?,
            component_side: node.child_value("side/@0")?,
            function: node.child_value("function/@0")?,
            holes: PadHoleList::deserialize(node)?,
        })
    }
}
