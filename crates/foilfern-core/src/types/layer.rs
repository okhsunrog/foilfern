//! Port of libs/librepcb/core/types/layer.{h,cpp}.
//!
//! Layers are a static registry. [`Layer`] is a `Copy` handle; comparing and
//! ordering handles follows the upstream layer order (`Layer::lessThan()`),
//! so a set of layers can simply be sorted.
//!
//! The graphics color role (upstream `ColorRole` from `core/workspace`) is
//! exposed as its identifier string until the workspace module is ported.

use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

use foilfern_i18n::tr;

use super::Error;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

// Flags (upstream `Layer::Flag`).
const NUMBER_MASK: u32 = 0xFF;
const SCHEMATIC: u32 = 1 << 8;
const BOARD: u32 = 1 << 16;
const TOP: u32 = 1 << 17;
const INNER: u32 = 1 << 18;
const BOTTOM: u32 = 1 << 19;
const COPPER: u32 = 1 << 20;
const STOP_MASK: u32 = 1 << 21;
const SOLDER_PASTE: u32 = 1 << 22;
const BOARD_EDGE: u32 = 1 << 23;
const PACKAGE_OUTLINE: u32 = 1 << 24;
const PACKAGE_COURTYARD: u32 = 1 << 25;
const POLYGONS_REPRESENT_AREAS: u32 = 1 << 26;

/// Number of inner copper layers (results in 64 copper layers in total).
const INNER_COPPER_COUNT: usize = 62;
/// Index of the first inner copper layer in [`Layer::all()`].
const FIRST_INNER_INDEX: u8 = 30;
/// Total number of layers.
const LAYER_COUNT: usize = 104;

/// A geometry layer (handle into the static layer registry).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Layer(u8);

struct LayerData {
    id: String,
    name: LayerName,
    color_role: String,
    flags: u32,
}

enum LayerName {
    Static(&'static str),
    InnerCopper(usize),
}

/// Definition of a layer which is not an inner copper layer.
struct FixedLayer {
    layer: Layer,
    id: &'static str,
    name: &'static str,
    color_role: &'static str,
    flags: u32,
}

macro_rules! fixed_layers {
    ($($const:ident = $index:literal, $id:literal, $name:literal, $role:literal, $flags:expr;)*) => {
        impl Layer {
            $(
                #[doc = concat!("Layer `", $id, "` (", $name, ").")]
                pub const $const: Layer = Layer($index);
            )*
        }

        const FIXED_LAYERS: &[FixedLayer] = &[
            $(FixedLayer {
                layer: Layer::$const,
                id: $id,
                name: $name,
                color_role: $role,
                flags: $flags,
            },)*
        ];
    };
}

fixed_layers! {
    SCHEMATIC_SHEET_FRAMES = 0, "sch_frames", "Sheet Frames", "schematic_frames", SCHEMATIC;
    SCHEMATIC_DOCUMENTATION = 1, "sch_documentation", "Documentation", "schematic_documentation", SCHEMATIC;
    SCHEMATIC_COMMENTS = 2, "sch_comments", "Comments", "schematic_comments", SCHEMATIC;
    SCHEMATIC_GUIDE = 3, "sch_guide", "Guide", "schematic_guide", SCHEMATIC;
    SYMBOL_OUTLINES = 4, "sym_outlines", "Outlines", "schematic_outlines", SCHEMATIC;
    SYMBOL_HIDDEN_GRAB_AREAS = 5, "sym_hidden_grab_areas", "Hidden Grab Areas", "schematic_hidden_grab_areas", SCHEMATIC;
    SYMBOL_NAMES = 6, "sym_names", "Names", "schematic_names", SCHEMATIC;
    SYMBOL_VALUES = 7, "sym_values", "Values", "schematic_values", SCHEMATIC;
    SYMBOL_PIN_NAMES = 8, "sym_pin_names", "Pin Names", "schematic_pin_names", SCHEMATIC;
    BOARD_SHEET_FRAMES = 9, "brd_frames", "Sheet Frames", "board_frames", BOARD;
    BOARD_OUTLINES = 10, "brd_outlines", "Board Outlines", "board_outlines", BOARD | BOARD_EDGE;
    BOARD_CUTOUTS = 11, "brd_cutouts", "Board Cutouts", "board_outlines", BOARD | BOARD_EDGE;
    BOARD_PLATED_CUTOUTS = 12, "brd_plated_cutouts", "Plated Board Cutouts", "board_plated_cutouts", BOARD | BOARD_EDGE;
    BOARD_MEASURES = 13, "brd_measures", "Measures", "board_measures", BOARD;
    BOARD_ALIGNMENT = 14, "brd_alignment", "Alignment", "board_alignment", BOARD;
    BOARD_DOCUMENTATION = 15, "brd_documentation", "Documentation", "board_documentation", BOARD;
    BOARD_COMMENTS = 16, "brd_comments", "Comments", "board_comments", BOARD;
    BOARD_GUIDE = 17, "brd_guide", "Guide", "board_guide", BOARD;
    TOP_NAMES = 18, "top_names", "Top Names", "board_names_top", BOARD | TOP;
    TOP_VALUES = 19, "top_values", "Top Values", "board_values_top", BOARD | TOP;
    TOP_LEGEND = 20, "top_legend", "Top Legend", "board_legend_top", BOARD | TOP;
    TOP_DOCUMENTATION = 21, "top_documentation", "Top Documentation", "board_documentation_top", BOARD | TOP;
    TOP_PACKAGE_OUTLINES = 22, "top_package_outlines", "Top Package Outlines", "board_package_outlines_top", BOARD | TOP | PACKAGE_OUTLINE | POLYGONS_REPRESENT_AREAS;
    TOP_COURTYARD = 23, "top_courtyard", "Top Courtyard", "board_courtyard_top", BOARD | TOP | PACKAGE_COURTYARD | POLYGONS_REPRESENT_AREAS;
    TOP_HIDDEN_GRAB_AREAS = 24, "top_hidden_grab_areas", "Top Hidden Grab Areas", "board_hidden_grab_areas_top", BOARD | TOP;
    TOP_STOP_MASK = 25, "top_stop_mask", "Top Stop Mask", "board_stop_mask_top", BOARD | TOP | STOP_MASK;
    TOP_SOLDER_PASTE = 26, "top_solder_paste", "Top Solder Paste", "board_solder_paste_top", BOARD | TOP | SOLDER_PASTE;
    TOP_FINISH = 27, "top_finish", "Top Finish", "board_finish_top", BOARD | TOP;
    TOP_GLUE = 28, "top_glue", "Top Glue", "board_glue_top", BOARD | TOP;
    TOP_COPPER = 29, "top_cu", "Top Copper", "board_copper_top", BOARD | TOP | COPPER;
    // Indices 30..=91: inner copper layers.
    BOT_COPPER = 92, "bot_cu", "Bottom Copper", "board_copper_bottom", BOARD | BOTTOM | COPPER | (INNER_COPPER_COUNT as u32 + 1);
    BOT_NAMES = 93, "bot_names", "Bottom Names", "board_names_bottom", BOARD | BOTTOM;
    BOT_VALUES = 94, "bot_values", "Bottom Values", "board_values_bottom", BOARD | BOTTOM;
    BOT_LEGEND = 95, "bot_legend", "Bottom Legend", "board_legend_bottom", BOARD | BOTTOM;
    BOT_DOCUMENTATION = 96, "bot_documentation", "Bottom Documentation", "board_documentation_bottom", BOARD | BOTTOM;
    BOT_PACKAGE_OUTLINES = 97, "bot_package_outlines", "Bottom Package Outlines", "board_package_outlines_bottom", BOARD | BOTTOM | PACKAGE_OUTLINE | POLYGONS_REPRESENT_AREAS;
    BOT_COURTYARD = 98, "bot_courtyard", "Bottom Courtyard", "board_courtyard_bottom", BOARD | BOTTOM | PACKAGE_COURTYARD | POLYGONS_REPRESENT_AREAS;
    BOT_HIDDEN_GRAB_AREAS = 99, "bot_hidden_grab_areas", "Bottom Hidden Grab Areas", "board_hidden_grab_areas_bottom", BOARD | BOTTOM;
    BOT_STOP_MASK = 100, "bot_stop_mask", "Bottom Stop Mask", "board_stop_mask_bottom", BOARD | BOTTOM | STOP_MASK;
    BOT_SOLDER_PASTE = 101, "bot_solder_paste", "Bottom Solder Paste", "board_solder_paste_bottom", BOARD | BOTTOM | SOLDER_PASTE;
    BOT_FINISH = 102, "bot_finish", "Bottom Finish", "board_finish_bottom", BOARD | BOTTOM;
    BOT_GLUE = 103, "bot_glue", "Bottom Glue", "board_glue_bottom", BOARD | BOTTOM;
}

/// Pairs of layers swapped by [`Layer::mirrored()`].
const MIRROR_PAIRS: [(Layer, Layer); 12] = [
    (Layer::TOP_NAMES, Layer::BOT_NAMES),
    (Layer::TOP_VALUES, Layer::BOT_VALUES),
    (Layer::TOP_LEGEND, Layer::BOT_LEGEND),
    (Layer::TOP_DOCUMENTATION, Layer::BOT_DOCUMENTATION),
    (Layer::TOP_PACKAGE_OUTLINES, Layer::BOT_PACKAGE_OUTLINES),
    (Layer::TOP_COURTYARD, Layer::BOT_COURTYARD),
    (Layer::TOP_HIDDEN_GRAB_AREAS, Layer::BOT_HIDDEN_GRAB_AREAS),
    (Layer::TOP_STOP_MASK, Layer::BOT_STOP_MASK),
    (Layer::TOP_SOLDER_PASTE, Layer::BOT_SOLDER_PASTE),
    (Layer::TOP_FINISH, Layer::BOT_FINISH),
    (Layer::TOP_GLUE, Layer::BOT_GLUE),
    (Layer::TOP_COPPER, Layer::BOT_COPPER),
];

const ALL: [Layer; LAYER_COUNT] = {
    let mut all = [Layer(0); LAYER_COUNT];
    let mut i = 0;
    while i < LAYER_COUNT {
        all[i] = Layer(i as u8);
        i += 1;
    }
    all
};

static REGISTRY: LazyLock<Vec<LayerData>> = LazyLock::new(|| {
    let mut slots: Vec<Option<LayerData>> = (0..LAYER_COUNT).map(|_| None).collect();
    for fixed in FIXED_LAYERS {
        slots[usize::from(fixed.layer.0)] = Some(LayerData {
            id: fixed.id.to_owned(),
            name: LayerName::Static(fixed.name),
            color_role: fixed.color_role.to_owned(),
            flags: fixed.flags,
        });
    }
    for number in 1..=INNER_COPPER_COUNT {
        slots[usize::from(FIRST_INNER_INDEX) + number - 1] = Some(LayerData {
            id: format!("in{number}_cu"),
            name: LayerName::InnerCopper(number),
            color_role: format!("board_copper_inner_{number}"),
            flags: BOARD | INNER | COPPER | number as u32,
        });
    }
    slots
        .into_iter()
        .map(|slot| slot.expect("every layer index is defined exactly once"))
        .collect()
});

impl Layer {
    /// Number of inner copper layers.
    pub const INNER_COPPER_COUNT: usize = INNER_COPPER_COUNT;

    fn data(self) -> &'static LayerData {
        &REGISTRY[usize::from(self.0)]
    }

    fn has(self, flag: u32) -> bool {
        self.data().flags & flag != 0
    }

    /// Returns all layers, sorted by function (this is also the [`Ord`]
    /// order of layers).
    pub fn all() -> &'static [Layer] {
        &ALL
    }

    /// Returns all inner copper layers (from top to bottom).
    pub fn inner_copper_layers() -> &'static [Layer] {
        let first = usize::from(FIRST_INNER_INDEX);
        &ALL[first..first + INNER_COPPER_COUNT]
    }

    /// Returns the inner copper layer with the given number
    /// (1..=[`INNER_COPPER_COUNT`](Self::INNER_COPPER_COUNT)).
    pub fn inner_copper(number: usize) -> Option<Layer> {
        Self::inner_copper_layers()
            .get(number.checked_sub(1)?)
            .copied()
    }

    /// Returns the copper layer with the given number (0 = top, 63 = bottom).
    pub fn copper(number: usize) -> Option<Layer> {
        match number {
            0 => Some(Self::TOP_COPPER),
            n if n == INNER_COPPER_COUNT + 1 => Some(Self::BOT_COPPER),
            n => Self::inner_copper(n),
        }
    }

    /// Returns the layer with the given identifier, if it exists (see also
    /// [`FromStr`], which returns an error instead).
    pub fn from_id(id: &str) -> Option<Layer> {
        Self::all().iter().copied().find(|l| l.id() == id)
    }

    /// Returns the serialization identifier (lower_snake_case), e.g. `"top_cu"`.
    pub fn id(self) -> &'static str {
        &self.data().id
    }

    /// Returns the translated, human readable name.
    pub fn name_tr(self) -> String {
        match self.data().name {
            LayerName::Static(name) => tr!("Layer", name),
            LayerName::InnerCopper(number) => tr!("Layer", "Inner Copper {0}", number),
        }
    }

    /// Returns the identifier of the graphics color role (e.g.
    /// `"board_copper_top"`).
    pub fn color_role(self) -> &'static str {
        &self.data().color_role
    }

    /// Returns whether this is a schematic layer.
    pub fn is_schematic(self) -> bool {
        self.has(SCHEMATIC)
    }

    /// Returns whether this is a board layer.
    pub fn is_board(self) -> bool {
        self.has(BOARD)
    }

    /// Returns whether this is a top board layer.
    pub fn is_top(self) -> bool {
        self.has(TOP)
    }

    /// Returns whether this is an inner board layer.
    pub fn is_inner(self) -> bool {
        self.has(INNER)
    }

    /// Returns whether this is a bottom board layer.
    pub fn is_bottom(self) -> bool {
        self.has(BOTTOM)
    }

    /// Returns whether this is a copper layer.
    pub fn is_copper(self) -> bool {
        self.has(COPPER)
    }

    /// Returns whether this is a stop mask layer.
    pub fn is_stop_mask(self) -> bool {
        self.has(STOP_MASK)
    }

    /// Returns whether this is a solder paste layer.
    pub fn is_solder_paste(self) -> bool {
        self.has(SOLDER_PASTE)
    }

    /// Returns whether this layer defines the board edge (outlines, cutouts,
    /// plated cutouts).
    pub fn is_board_edge(self) -> bool {
        self.has(BOARD_EDGE)
    }

    /// Returns whether this is a package outline layer.
    pub fn is_package_outline(self) -> bool {
        self.has(PACKAGE_OUTLINE)
    }

    /// Returns whether this is a package courtyard layer.
    pub fn is_package_courtyard(self) -> bool {
        self.has(PACKAGE_COURTYARD)
    }

    /// Returns whether polygons on this layer always represent areas.
    pub fn polygons_represent_areas(self) -> bool {
        self.has(POLYGONS_REPRESENT_AREAS)
    }

    /// Returns the copper layer number (0 = top, 1 = first inner,
    /// 63 = bottom); 0 for non-copper layers.
    pub fn copper_number(self) -> usize {
        (self.data().flags & NUMBER_MASK) as usize
    }

    /// Returns the layer mirrored to the other board side, or `self` if not
    /// mirrorable.
    ///
    /// If `inner_layers` (the number of used inner layers) is given, inner
    /// copper layers are mirrored within this layer count as well.
    pub fn mirrored(self, inner_layers: Option<usize>) -> Layer {
        for (top, bot) in MIRROR_PAIRS {
            if self == top {
                return bot;
            } else if self == bot {
                return top;
            }
        }
        match inner_layers {
            Some(count) if count <= INNER_COPPER_COUNT && self.is_inner() => (count + 1)
                .checked_sub(self.copper_number())
                .and_then(Self::inner_copper)
                .unwrap_or(self),
            _ => self,
        }
    }
}

impl fmt::Debug for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Layer({})", self.id())
    }
}

impl fmt::Display for Layer {
    /// Formats the identifier.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

// Serde: serialized as the identifier, e.g. `"top_cu"`.
crate::utils::serde_string::serde_string!(Layer);

impl FromStr for Layer {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::from_id(s).ok_or_else(|| Error::UnknownLayer(s.to_owned()))
    }
}

impl ToSExpression for Layer {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.id())
    }
}

impl FromSExpression for Layer {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_order() {
        let mut expected: Vec<String> = [
            "sch_frames",
            "sch_documentation",
            "sch_comments",
            "sch_guide",
            "sym_outlines",
            "sym_hidden_grab_areas",
            "sym_names",
            "sym_values",
            "sym_pin_names",
            "brd_frames",
            "brd_outlines",
            "brd_cutouts",
            "brd_plated_cutouts",
            "brd_measures",
            "brd_alignment",
            "brd_documentation",
            "brd_comments",
            "brd_guide",
            "top_names",
            "top_values",
            "top_legend",
            "top_documentation",
            "top_package_outlines",
            "top_courtyard",
            "top_hidden_grab_areas",
            "top_stop_mask",
            "top_solder_paste",
            "top_finish",
            "top_glue",
            "top_cu",
        ]
        .map(String::from)
        .to_vec();
        expected.extend((1..=62).map(|i| format!("in{i}_cu")));
        expected.extend(
            [
                "bot_cu",
                "bot_names",
                "bot_values",
                "bot_legend",
                "bot_documentation",
                "bot_package_outlines",
                "bot_courtyard",
                "bot_hidden_grab_areas",
                "bot_stop_mask",
                "bot_solder_paste",
                "bot_finish",
                "bot_glue",
            ]
            .map(String::from),
        );
        let actual: Vec<&str> = Layer::all().iter().map(|l| l.id()).collect();
        assert_eq!(actual, expected);
        for layer in Layer::all() {
            assert_eq!(Layer::from_id(layer.id()), Some(*layer));
            assert_eq!(layer.id().parse(), Ok(*layer));
        }
    }

    #[test]
    fn copper_numbers() {
        assert_eq!(Layer::TOP_COPPER.copper_number(), 0);
        assert_eq!(Layer::inner_copper(1).unwrap().copper_number(), 1);
        assert_eq!(Layer::inner_copper(62).unwrap().id(), "in62_cu");
        assert_eq!(Layer::BOT_COPPER.copper_number(), 63);
        assert_eq!(Layer::copper(63), Some(Layer::BOT_COPPER));
        assert_eq!(Layer::copper(5).unwrap().id(), "in5_cu");
        assert_eq!(Layer::inner_copper(0), None);
        assert_eq!(Layer::inner_copper(63), None);
        assert_eq!(Layer::copper(64), None);
        assert!(Layer::BOT_COPPER.is_copper() && Layer::BOT_COPPER.is_bottom());
        assert_eq!(Layer::inner_copper(3).unwrap().name_tr(), "Inner Copper 3");
        assert_eq!(
            Layer::inner_copper(3).unwrap().color_role(),
            "board_copper_inner_3"
        );
    }

    #[test]
    fn mirrored() {
        assert_eq!(Layer::TOP_COPPER.mirrored(None), Layer::BOT_COPPER);
        assert_eq!(Layer::BOT_LEGEND.mirrored(None), Layer::TOP_LEGEND);
        assert_eq!(
            Layer::BOARD_OUTLINES.mirrored(Some(2)),
            Layer::BOARD_OUTLINES
        );
        let in1 = Layer::inner_copper(1).unwrap();
        let in2 = Layer::inner_copper(2).unwrap();
        assert_eq!(in1.mirrored(None), in1);
        assert_eq!(in1.mirrored(Some(2)), in2);
        assert_eq!(in2.mirrored(Some(2)), in1);
        assert_eq!(in1.mirrored(Some(1)), in1);
    }

    #[test]
    fn serialization() {
        assert_eq!(
            Layer::TOP_COPPER.to_sexpression(),
            SExpression::token("top_cu")
        );
        assert_eq!(
            Layer::from_sexpression(&SExpression::token("in3_cu")),
            Ok(Layer::inner_copper(3).unwrap())
        );
        assert!(Layer::from_sexpression(&SExpression::token("foo")).is_err());
    }
}
