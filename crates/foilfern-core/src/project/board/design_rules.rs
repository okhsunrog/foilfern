//! Port of libs/librepcb/core/project/board/boarddesignrules.{h,cpp}.
//!
//! Differences to upstream: plain data with public getters/setters; the
//! `restoreDefaults()` method is [`Default`].

use crate::geometry::property;
use crate::library::org::BoardDesignRuleCheckSettings;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{
    self, BoundedUnsignedRatio, Length, PositiveLength, Ratio, UnsignedLength, UnsignedRatio,
};

/// The design rules of a board (default trace width, via drill, stop mask,
/// solder paste and annular rings).
///
/// Serde: an object with one field per rule.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardDesignRules {
    default_trace_width: PositiveLength,
    default_via_drill_diameter: PositiveLength,
    stop_mask_max_via_drill_diameter: UnsignedLength,
    stop_mask_clearance: BoundedUnsignedRatio,
    solder_paste_clearance: BoundedUnsignedRatio,
    pad_cmp_side_auto_annular_ring: bool,
    pad_inner_auto_annular_ring: bool,
    /// Percentage of the drill diameter.
    pad_annular_ring: BoundedUnsignedRatio,
    /// Percentage of the drill diameter.
    via_annular_ring: BoundedUnsignedRatio,
}

fn positive(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).expect("constant is positive")
}

fn unsigned(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).expect("constant is not negative")
}

fn bounded(percent: i32, min: i64, max: i64) -> BoundedUnsignedRatio {
    BoundedUnsignedRatio::new(
        UnsignedRatio::new(Ratio::from_percent(percent)).expect("constant is not negative"),
        unsigned(min),
        unsigned(max),
    )
    .expect("constant min <= max")
}

impl Default for BoardDesignRules {
    fn default() -> Self {
        Self {
            default_trace_width: positive(500_000),              // 0.5mm
            default_via_drill_diameter: positive(300_000),       // 0.3mm
            stop_mask_max_via_drill_diameter: unsigned(300_000), // 0.3mm
            stop_mask_clearance: bounded(0, 100_000, 100_000),   // 0%, 0.1mm..0.1mm
            solder_paste_clearance: bounded(10, 0, 1_000_000),   // 10%, 0..1mm
            pad_cmp_side_auto_annular_ring: false,
            pad_inner_auto_annular_ring: true,
            pad_annular_ring: bounded(25, 250_000, 2_000_000), // 25%, 0.25..2mm
            via_annular_ring: bounded(25, 200_000, 2_000_000), // 25%, 0.2..2mm
        }
    }
}

impl BoardDesignRules {
    property!(
        /// Returns the default trace width.
        copy default_trace_width: PositiveLength, set_default_trace_width
    );
    property!(
        /// Returns the default via drill diameter.
        copy default_via_drill_diameter: PositiveLength, set_default_via_drill_diameter
    );
    property!(
        /// Returns the maximum drill diameter of vias covered with stop mask
        /// (upstream `getStopMaskMaxViaDiameter()`).
        copy stop_mask_max_via_drill_diameter: UnsignedLength,
        set_stop_mask_max_via_drill_diameter
    );
    property!(
        /// Returns the stop mask clearance.
        copy stop_mask_clearance: BoundedUnsignedRatio, set_stop_mask_clearance
    );
    property!(
        /// Returns the solder paste clearance.
        copy solder_paste_clearance: BoundedUnsignedRatio, set_solder_paste_clearance
    );
    property!(
        /// Returns whether THT pads get an automatic annular ring on the
        /// component side.
        copy pad_cmp_side_auto_annular_ring: bool, set_pad_cmp_side_auto_annular_ring
    );
    property!(
        /// Returns whether THT pads get an automatic annular ring on inner
        /// layers.
        copy pad_inner_auto_annular_ring: bool, set_pad_inner_auto_annular_ring
    );
    property!(
        /// Returns the pad annular ring (relative to the drill diameter).
        copy pad_annular_ring: BoundedUnsignedRatio, set_pad_annular_ring
    );
    property!(
        /// Returns the via annular ring (relative to the drill diameter).
        copy via_annular_ring: BoundedUnsignedRatio, set_via_annular_ring
    );

    /// Raises the rules to the minimum values of the DRC settings
    /// (upstream `adjustToDrcSettings()`, including its use of the pad
    /// annular ring default for the via annular ring).
    pub fn adjust_to_drc_settings(&mut self, s: &BoardDesignRuleCheckSettings) {
        let defaults = Self::default();
        if *self.default_trace_width < *s.min_copper_width() {
            self.default_trace_width =
                PositiveLength::new(*s.min_copper_width()).unwrap_or(self.default_trace_width);
        }
        if *self.default_via_drill_diameter < *s.min_npth_drill_diameter() {
            self.default_via_drill_diameter = PositiveLength::new(*s.min_npth_drill_diameter())
                .unwrap_or(self.default_via_drill_diameter);
        }
        if self.stop_mask_max_via_drill_diameter < s.max_tented_via_drill_diameter() {
            self.stop_mask_max_via_drill_diameter = s.max_tented_via_drill_diameter();
        }
        let min_annular = defaults
            .pad_annular_ring
            .min_value()
            .max(s.min_pth_annular_ring());
        let adjust = |r: BoundedUnsignedRatio| {
            BoundedUnsignedRatio::new(r.ratio(), min_annular, r.max_value().max(min_annular))
                .expect("min <= max by construction")
        };
        self.pad_annular_ring = adjust(self.pad_annular_ring);
        self.via_annular_ring = adjust(self.via_annular_ring);
    }

    /// Whether a via with the given drill diameter needs a stop mask opening
    /// (upstream `doesViaRequireStopMaskOpening()`).
    pub fn does_via_require_stop_mask_opening(&self, drill_diameter: Length) -> bool {
        drill_diameter > *self.stop_mask_max_via_drill_diameter
    }
}

fn parse_pad_auto_annular(node: &SExpression) -> serialization::Result<bool> {
    match node.value()? {
        "auto" => Ok(true),
        "full" => Ok(false),
        other => Err(types::Error::InvalidPadAnnularShape(other.to_owned()).into()),
    }
}

fn pad_auto_annular_token(auto: bool) -> SExpression {
    SExpression::token(if auto { "auto" } else { "full" })
}

impl SerializeObject for BoardDesignRules {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        root.append_child("default_trace_width", &self.default_trace_width);
        root.ensure_line_break();
        root.append_child(
            "default_via_drill_diameter",
            &self.default_via_drill_diameter,
        );
        root.ensure_line_break();
        root.append_child(
            "stopmask_max_via_drill_diameter",
            &self.stop_mask_max_via_drill_diameter,
        );
        root.ensure_line_break();
        self.stop_mask_clearance
            .serialize(root.append_list("stopmask_clearance"));
        root.ensure_line_break();
        self.solder_paste_clearance
            .serialize(root.append_list("solderpaste_clearance"));
        root.ensure_line_break();
        {
            let node = root.append_list("pad_annular_ring");
            node.append_child(
                "outer",
                &pad_auto_annular_token(self.pad_cmp_side_auto_annular_ring),
            );
            node.append_child(
                "inner",
                &pad_auto_annular_token(self.pad_inner_auto_annular_ring),
            );
            self.pad_annular_ring.serialize(node);
        }
        root.ensure_line_break();
        self.via_annular_ring
            .serialize(root.append_list("via_annular_ring"));
        root.ensure_line_break();
    }
}

impl DeserializeObject for BoardDesignRules {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            default_trace_width: node.child_value("default_trace_width/@0")?,
            default_via_drill_diameter: node.child_value("default_via_drill_diameter/@0")?,
            stop_mask_max_via_drill_diameter: node
                .child_value("stopmask_max_via_drill_diameter/@0")?,
            stop_mask_clearance: BoundedUnsignedRatio::deserialize(
                node.required_child("stopmask_clearance")?,
            )?,
            solder_paste_clearance: BoundedUnsignedRatio::deserialize(
                node.required_child("solderpaste_clearance")?,
            )?,
            pad_cmp_side_auto_annular_ring: parse_pad_auto_annular(
                node.required_child("pad_annular_ring/outer/@0")?,
            )?,
            pad_inner_auto_annular_ring: parse_pad_auto_annular(
                node.required_child("pad_annular_ring/inner/@0")?,
            )?,
            pad_annular_ring: BoundedUnsignedRatio::deserialize(
                node.required_child("pad_annular_ring")?,
            )?,
            via_annular_ring: BoundedUnsignedRatio::deserialize(
                node.required_child("via_annular_ring")?,
            )?,
        })
    }
}
