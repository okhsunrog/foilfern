//! Port of
//! libs/librepcb/core/project/board/drc/boarddesignrulechecksettings.{h,cpp}.
//!
//! Upstream this class belongs to the board, but it is also used by
//! [`OrganizationPcbDesignRules`](super::OrganizationPcbDesignRules) (upstream
//! notes that it should rather be in the common sources). It lives here until
//! the board is ported.
//!
//! Differences to upstream:
//! - The sources are kept in insertion order (upstream: `std::unordered_set`,
//!   i.e. unspecified serialization order for multiple sources).
//! - The colors are sets of `Option<PcbColor>` (`None` = "none") instead of
//!   `QSet<const PcbColor*>`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::str::FromStr;

use crate::geometry::property;
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::types::{
    ElementName, Error, Length, PcbColor, PositiveLength, UnsignedLength, Uuid, Version,
};

/// Which kinds of slots are allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AllowedSlots {
    /// No slots are allowed at all.
    None,
    /// Straight single-segment slots are allowed.
    SingleSegmentStraight,
    /// Straight multi-segment slots are allowed.
    MultiSegmentStraight,
    /// Any kind of slot is allowed (including curves).
    Any,
}

impl AllowedSlots {
    /// Returns the serialization token.
    pub fn to_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::SingleSegmentStraight => "single_segment_straight",
            Self::MultiSegmentStraight => "multi_segment_straight",
            Self::Any => "any",
        }
    }
}

impl fmt::Display for AllowedSlots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_str())
    }
}

impl FromStr for AllowedSlots {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        [
            Self::None,
            Self::SingleSegmentStraight,
            Self::MultiSegmentStraight,
            Self::Any,
        ]
        .into_iter()
        .find(|v| v.to_str() == s)
        .ok_or_else(|| Error::UnknownAllowedSlots(s.to_owned()))
    }
}

// Serde: the file format token.
crate::utils::serde_string::serde_string!(AllowedSlots);

impl ToSExpression for AllowedSlots {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_str())
    }
}

impl FromSExpression for AllowedSlots {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

/// The organization's design rules the settings were imported from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DrcSettingsSource {
    /// UUID of the organization.
    pub organization_uuid: Uuid,
    /// Name of the organization.
    pub organization_name: ElementName,
    /// Version of the organization.
    pub organization_version: Version,
    /// UUID of the PCB design rules.
    pub pcb_design_rules_uuid: Uuid,
    /// Name of the PCB design rules.
    pub pcb_design_rules_name: ElementName,
}

impl SerializeObject for DrcSettingsSource {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        let organization = root.append_list("organization");
        organization.append_value(&self.organization_uuid);
        organization.append_value(&self.organization_name);
        organization.append_value(&self.organization_version);
        root.ensure_line_break();
        let rules = root.append_list("design_rules");
        rules.append_value(&self.pcb_design_rules_uuid);
        rules.append_value(&self.pcb_design_rules_name);
        root.ensure_line_break();
    }
}

impl DeserializeObject for DrcSettingsSource {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            organization_uuid: node.child_value("organization/@0")?,
            organization_name: node.child_value("organization/@1")?,
            organization_version: node.child_value("organization/@2")?,
            pcb_design_rules_uuid: node.child_value("design_rules/@0")?,
            pcb_design_rules_name: node.child_value("design_rules/@1")?,
        })
    }
}

/// A pair of lengths (width, height).
pub type LengthPair = (UnsignedLength, UnsignedLength);

/// Settings of the board design rule check (DRC).
///
/// Serde: an object with one field per setting (lengths in nanometers).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BoardDesignRuleCheckSettings {
    sources: Vec<DrcSettingsSource>,
    min_board_size: LengthPair,
    max_board_size_double_sided: LengthPair,
    max_board_size_multi_layer: LengthPair,
    pcb_thickness: BTreeSet<PositiveLength>,
    max_layer_count: u32,
    solder_resist: HashSet<Option<PcbColor>>,
    silkscreen: HashSet<Option<PcbColor>>,
    min_copper_copper_clearance: UnsignedLength,
    min_copper_board_clearance: UnsignedLength,
    min_copper_npth_clearance: UnsignedLength,
    min_drill_drill_clearance: UnsignedLength,
    min_drill_board_clearance: UnsignedLength,
    min_silkscreen_stopmask_clearance: UnsignedLength,
    min_copper_width: UnsignedLength,
    min_pth_annular_ring: UnsignedLength,
    min_npth_drill_diameter: UnsignedLength,
    min_pth_drill_diameter: UnsignedLength,
    min_npth_slot_width: UnsignedLength,
    min_pth_slot_width: UnsignedLength,
    max_tented_via_drill_diameter: UnsignedLength,
    min_silkscreen_width: UnsignedLength,
    min_silkscreen_text_height: UnsignedLength,
    min_outline_tool_diameter: UnsignedLength,
    blind_vias_allowed: bool,
    buried_vias_allowed: bool,
    allowed_npth_slots: AllowedSlots,
    allowed_pth_slots: AllowedSlots,
    options: BTreeMap<String, Vec<SExpression>>,
}

fn unsigned(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).expect("constant is not negative")
}

impl Default for BoardDesignRuleCheckSettings {
    /// Returns the default settings.
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            min_board_size: (unsigned(0), unsigned(0)), // No minimum
            max_board_size_double_sided: (unsigned(0), unsigned(0)), // No limit
            max_board_size_multi_layer: (unsigned(0), unsigned(0)), // No limit
            pcb_thickness: BTreeSet::new(),             // No restrictions
            max_layer_count: 0,                         // No restrictions
            solder_resist: HashSet::new(),              // No restrictions
            silkscreen: HashSet::new(),                 // No restrictions
            min_copper_copper_clearance: unsigned(200_000), // 200um
            min_copper_board_clearance: unsigned(300_000), // 300um
            min_copper_npth_clearance: unsigned(250_000), // 250um
            min_drill_drill_clearance: unsigned(350_000), // 350um
            min_drill_board_clearance: unsigned(500_000), // 500um
            min_silkscreen_stopmask_clearance: unsigned(127_000), // 127um
            min_copper_width: unsigned(200_000),        // 200um
            min_pth_annular_ring: unsigned(200_000),    // 200um
            min_npth_drill_diameter: unsigned(500_000), // 0.5mm
            min_pth_drill_diameter: unsigned(300_000),  // 0.3mm
            min_npth_slot_width: unsigned(1_000_000),   // 1mm
            min_pth_slot_width: unsigned(700_000),      // 0.7mm
            max_tented_via_drill_diameter: unsigned(3_000_000), // upstream value
            min_silkscreen_width: unsigned(150_000),    // 150um
            min_silkscreen_text_height: unsigned(800_000), // 0.8mm
            min_outline_tool_diameter: unsigned(2_000_000), // 2mm
            blind_vias_allowed: false,                  // Just to be on the safe side
            buried_vias_allowed: false,                 // Just to be on the safe side
            allowed_npth_slots: AllowedSlots::SingleSegmentStraight,
            allowed_pth_slots: AllowedSlots::SingleSegmentStraight,
            options: BTreeMap::new(),
        }
    }
}

impl BoardDesignRuleCheckSettings {
    /// Returns the sources (the organization design rules the settings were
    /// imported from).
    pub fn sources(&self) -> &[DrcSettingsSource] {
        &self.sources
    }

    /// Sets the sources; sources with the same organization and design rules
    /// UUIDs are merged. Returns whether the value was modified.
    pub fn set_sources(&mut self, sources: Vec<DrcSettingsSource>) -> bool {
        let mut unique: Vec<DrcSettingsSource> = Vec::new();
        for src in sources {
            if !unique.iter().any(|s| {
                s.organization_uuid == src.organization_uuid
                    && s.pcb_design_rules_uuid == src.pcb_design_rules_uuid
            }) {
                unique.push(src);
            }
        }
        if unique == self.sources {
            return false;
        }
        self.sources = unique;
        true
    }

    property!(
        /// Returns the minimum board size (0 = no minimum).
        copy min_board_size: LengthPair, set_min_board_size
    );
    property!(
        /// Returns the maximum size of double sided boards (0 = no limit).
        copy max_board_size_double_sided: LengthPair, set_max_board_size_double_sided
    );
    property!(
        /// Returns the maximum size of multilayer boards (0 = no limit).
        copy max_board_size_multi_layer: LengthPair, set_max_board_size_multi_layer
    );
    property!(
        /// Returns the allowed board thicknesses (empty = no restrictions).
        ref pcb_thickness: BTreeSet<PositiveLength>, set_pcb_thickness
    );
    property!(
        /// Returns the maximum number of copper layers (0 = no restrictions).
        copy max_layer_count: u32, set_max_layer_count
    );
    property!(
        /// Returns the available solder resist colors (empty = any/unknown,
        /// `None` = no solder resist).
        ref solder_resist: HashSet<Option<PcbColor>>, set_solder_resist
    );
    property!(
        /// Returns the available silkscreen colors (empty = any/unknown,
        /// `None` = no silkscreen).
        ref silkscreen: HashSet<Option<PcbColor>>, set_silkscreen
    );
    property!(
        /// Returns the minimum copper-copper clearance.
        copy min_copper_copper_clearance: UnsignedLength, set_min_copper_copper_clearance
    );
    property!(
        /// Returns the minimum copper-board clearance.
        copy min_copper_board_clearance: UnsignedLength, set_min_copper_board_clearance
    );
    property!(
        /// Returns the minimum copper-NPTH clearance.
        copy min_copper_npth_clearance: UnsignedLength, set_min_copper_npth_clearance
    );
    property!(
        /// Returns the minimum drill-drill clearance.
        copy min_drill_drill_clearance: UnsignedLength, set_min_drill_drill_clearance
    );
    property!(
        /// Returns the minimum drill-board clearance.
        copy min_drill_board_clearance: UnsignedLength, set_min_drill_board_clearance
    );
    property!(
        /// Returns the minimum silkscreen-stopmask clearance.
        copy min_silkscreen_stopmask_clearance: UnsignedLength,
        set_min_silkscreen_stopmask_clearance
    );
    property!(
        /// Returns the minimum copper width.
        copy min_copper_width: UnsignedLength, set_min_copper_width
    );
    property!(
        /// Returns the minimum PTH annular ring.
        copy min_pth_annular_ring: UnsignedLength, set_min_pth_annular_ring
    );
    property!(
        /// Returns the minimum NPTH drill diameter.
        copy min_npth_drill_diameter: UnsignedLength, set_min_npth_drill_diameter
    );
    property!(
        /// Returns the minimum PTH drill diameter.
        copy min_pth_drill_diameter: UnsignedLength, set_min_pth_drill_diameter
    );
    property!(
        /// Returns the minimum NPTH slot width.
        copy min_npth_slot_width: UnsignedLength, set_min_npth_slot_width
    );
    property!(
        /// Returns the minimum PTH slot width.
        copy min_pth_slot_width: UnsignedLength, set_min_pth_slot_width
    );
    property!(
        /// Returns the maximum drill diameter of tented vias.
        copy max_tented_via_drill_diameter: UnsignedLength, set_max_tented_via_drill_diameter
    );
    property!(
        /// Returns the minimum silkscreen line width.
        copy min_silkscreen_width: UnsignedLength, set_min_silkscreen_width
    );
    property!(
        /// Returns the minimum silkscreen text height.
        copy min_silkscreen_text_height: UnsignedLength, set_min_silkscreen_text_height
    );
    property!(
        /// Returns the minimum outline tool diameter.
        copy min_outline_tool_diameter: UnsignedLength, set_min_outline_tool_diameter
    );
    property!(
        /// Returns whether blind vias are allowed.
        copy blind_vias_allowed: bool, set_blind_vias_allowed
    );
    property!(
        /// Returns whether buried vias are allowed.
        copy buried_vias_allowed: bool, set_buried_vias_allowed
    );
    property!(
        /// Returns which NPTH slots are allowed.
        copy allowed_npth_slots: AllowedSlots, set_allowed_npth_slots
    );
    property!(
        /// Returns which PTH slots are allowed.
        copy allowed_pth_slots: AllowedSlots, set_allowed_pth_slots
    );
    property!(
        /// Returns the additional options (`(option <key> ...)` nodes by key,
        /// for forward compatibility).
        ref options: BTreeMap<String, Vec<SExpression>>, set_options
    );
}

/// Returns the colors sorted by ID, with `None` last (upstream `cmpColor`).
fn sorted_colors(colors: &HashSet<Option<PcbColor>>) -> Vec<Option<PcbColor>> {
    let mut colors: Vec<_> = colors.iter().copied().collect();
    colors.sort_by_key(|c| (c.is_none(), c.map(PcbColor::id)));
    colors
}

fn load_colors(node: &SExpression, name: &str) -> serialization::Result<HashSet<Option<PcbColor>>> {
    node.required_child(name)?
        .children()
        .iter()
        .filter(|c| c.is_token())
        .map(Option::<PcbColor>::from_sexpression)
        .collect()
}

pub(crate) fn load_options(
    node: &SExpression,
) -> serialization::Result<BTreeMap<String, Vec<SExpression>>> {
    let mut options: BTreeMap<String, Vec<SExpression>> = BTreeMap::new();
    for child in node.children_named("option") {
        let key = child.required_child("@0")?.value()?.to_owned();
        options.entry(key).or_default().push(child.clone());
    }
    Ok(options)
}

impl SerializeObject for BoardDesignRuleCheckSettings {
    fn serialize(&self, root: &mut List) {
        for src in &self.sources {
            root.ensure_line_break();
            src.serialize(root.append_list("source"));
        }
        root.ensure_line_break();
        let child = root.append_list("min_pcb_size");
        child.append_value(&self.min_board_size.0);
        child.append_value(&self.min_board_size.1);
        root.ensure_line_break();
        let child = root.append_list("max_pcb_size");
        let double_sided = child.append_list("double_sided");
        double_sided.append_value(&self.max_board_size_double_sided.0);
        double_sided.append_value(&self.max_board_size_double_sided.1);
        let multilayer = child.append_list("multilayer");
        multilayer.append_value(&self.max_board_size_multi_layer.0);
        multilayer.append_value(&self.max_board_size_multi_layer.1);
        root.ensure_line_break();
        let child = root.append_list("pcb_thickness");
        for value in &self.pcb_thickness {
            child.append_value(value);
        }
        root.ensure_line_break();
        root.append_child("max_layers", &self.max_layer_count);
        root.ensure_line_break();
        let child = root.append_list("solder_resist");
        for value in sorted_colors(&self.solder_resist) {
            child.append_value(&value);
        }
        root.ensure_line_break();
        let child = root.append_list("silkscreen");
        for value in sorted_colors(&self.silkscreen) {
            child.append_value(&value);
        }
        root.ensure_line_break();
        let lengths = [
            (
                "min_copper_copper_clearance",
                self.min_copper_copper_clearance,
            ),
            (
                "min_copper_board_clearance",
                self.min_copper_board_clearance,
            ),
            ("min_copper_npth_clearance", self.min_copper_npth_clearance),
            ("min_drill_drill_clearance", self.min_drill_drill_clearance),
            ("min_drill_board_clearance", self.min_drill_board_clearance),
            (
                "min_silkscreen_stopmask_clearance",
                self.min_silkscreen_stopmask_clearance,
            ),
            ("min_copper_width", self.min_copper_width),
            ("min_annular_ring", self.min_pth_annular_ring),
            ("min_npth_drill_diameter", self.min_npth_drill_diameter),
            ("min_pth_drill_diameter", self.min_pth_drill_diameter),
            ("min_npth_slot_width", self.min_npth_slot_width),
            ("min_pth_slot_width", self.min_pth_slot_width),
            (
                "max_tented_via_drill_diameter",
                self.max_tented_via_drill_diameter,
            ),
            ("min_silkscreen_width", self.min_silkscreen_width),
            (
                "min_silkscreen_text_height",
                self.min_silkscreen_text_height,
            ),
            ("min_outline_tool_diameter", self.min_outline_tool_diameter),
        ];
        for (name, value) in lengths {
            root.append_child(name, &value);
            root.ensure_line_break();
        }
        root.append_child("blind_vias_allowed", &self.blind_vias_allowed);
        root.ensure_line_break();
        root.append_child("buried_vias_allowed", &self.buried_vias_allowed);
        root.ensure_line_break();
        root.append_child("allowed_npth_slots", &self.allowed_npth_slots);
        root.ensure_line_break();
        root.append_child("allowed_pth_slots", &self.allowed_pth_slots);
        root.ensure_line_break();
        for node in self.options.values().flatten() {
            root.ensure_line_break();
            root.push(node.clone());
        }
        root.ensure_line_break();
    }
}

impl DeserializeObject for BoardDesignRuleCheckSettings {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let mut settings = Self {
            sources: Vec::new(),
            min_board_size: (
                node.child_value("min_pcb_size/@0")?,
                node.child_value("min_pcb_size/@1")?,
            ),
            max_board_size_double_sided: (
                node.child_value("max_pcb_size/double_sided/@0")?,
                node.child_value("max_pcb_size/double_sided/@1")?,
            ),
            max_board_size_multi_layer: (
                node.child_value("max_pcb_size/multilayer/@0")?,
                node.child_value("max_pcb_size/multilayer/@1")?,
            ),
            pcb_thickness: node
                .required_child("pcb_thickness")?
                .children()
                .iter()
                .filter(|c| c.is_token())
                .map(PositiveLength::from_sexpression)
                .collect::<serialization::Result<_>>()?,
            max_layer_count: node.child_value("max_layers/@0")?,
            solder_resist: load_colors(node, "solder_resist")?,
            silkscreen: load_colors(node, "silkscreen")?,
            min_copper_copper_clearance: node.child_value("min_copper_copper_clearance/@0")?,
            min_copper_board_clearance: node.child_value("min_copper_board_clearance/@0")?,
            min_copper_npth_clearance: node.child_value("min_copper_npth_clearance/@0")?,
            min_drill_drill_clearance: node.child_value("min_drill_drill_clearance/@0")?,
            min_drill_board_clearance: node.child_value("min_drill_board_clearance/@0")?,
            min_silkscreen_stopmask_clearance: node
                .child_value("min_silkscreen_stopmask_clearance/@0")?,
            min_copper_width: node.child_value("min_copper_width/@0")?,
            min_pth_annular_ring: node.child_value("min_annular_ring/@0")?,
            min_npth_drill_diameter: node.child_value("min_npth_drill_diameter/@0")?,
            min_pth_drill_diameter: node.child_value("min_pth_drill_diameter/@0")?,
            min_npth_slot_width: node.child_value("min_npth_slot_width/@0")?,
            min_pth_slot_width: node.child_value("min_pth_slot_width/@0")?,
            max_tented_via_drill_diameter: node.child_value("max_tented_via_drill_diameter/@0")?,
            min_silkscreen_width: node.child_value("min_silkscreen_width/@0")?,
            min_silkscreen_text_height: node.child_value("min_silkscreen_text_height/@0")?,
            min_outline_tool_diameter: node.child_value("min_outline_tool_diameter/@0")?,
            blind_vias_allowed: node.child_value("blind_vias_allowed/@0")?,
            buried_vias_allowed: node.child_value("buried_vias_allowed/@0")?,
            allowed_npth_slots: node.child_value("allowed_npth_slots/@0")?,
            allowed_pth_slots: node.child_value("allowed_pth_slots/@0")?,
            options: load_options(node)?,
        };
        let sources = node
            .children_named("source")
            .map(DrcSettingsSource::deserialize)
            .collect::<serialization::Result<_>>()?;
        settings.set_sources(sources);
        Ok(settings)
    }
}
