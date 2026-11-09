//! Port of libs/librepcb/core/job/interactivehtmlbomoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJobType};
use crate::export::{InteractiveHtmlBomHighlightPin1Mode, InteractiveHtmlBomViewMode};
use crate::project::{AssemblyVariantId, BoardId};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Angle, ElementName};

/// Settings of an interactive HTML bill of materials output job (upstream
/// `InteractiveHtmlBomOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InteractiveHtmlBomOutputJob {
    /// Initial layout.
    pub view_mode: InteractiveHtmlBomViewMode,
    /// Initial pin 1 highlighting.
    pub highlight_pin1: InteractiveHtmlBomHighlightPin1Mode,
    /// Whether the dark mode is initially enabled.
    pub dark_mode: bool,
    /// Initial board rotation.
    pub board_rotation: Angle,
    /// Whether the rotation of the back side is offset.
    pub offset_back_rotation: bool,
    /// Whether the silkscreen is shown.
    pub show_silkscreen: bool,
    /// Whether the fabrication layers are shown.
    pub show_fabrication: bool,
    /// Whether pads are shown.
    pub show_pads: bool,
    /// Whether traces are shown.
    pub show_tracks: bool,
    /// Whether zones are shown.
    pub show_zones: bool,
    /// Names of the check box columns.
    pub check_boxes: Vec<String>,
    /// Sort order of the components by designator prefix.
    pub component_order: Vec<String>,
    /// Attribute keys of additional columns.
    pub custom_attributes: Vec<String>,
    /// Boards to export.
    pub boards: ObjectSet<BoardId>,
    /// Assembly variants to export.
    pub assembly_variants: ObjectSet<AssemblyVariantId>,
    /// Output file path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for InteractiveHtmlBomOutputJob {
    fn default() -> Self {
        let strings = |s: &[&str]| s.iter().map(|s| (*s).to_owned()).collect();
        Self {
            view_mode: InteractiveHtmlBomViewMode::LeftRight,
            highlight_pin1: InteractiveHtmlBomHighlightPin1Mode::None,
            dark_mode: false,
            board_rotation: Angle::DEG0,
            offset_back_rotation: false,
            show_silkscreen: true,
            show_fabrication: true,
            show_pads: true,
            show_tracks: true,
            show_zones: true,
            check_boxes: strings(&["Sourced", "Placed"]),
            component_order: strings(&["C", "R", "L", "D", "U", "Y", "X", "F"]),
            custom_attributes: Vec::new(),
            boards: ObjectSet::Default,
            assembly_variants: ObjectSet::All,
            output_path: "assembly/{{PROJECT}}_{{VERSION}}_BOM_{{VARIANT}}.html".into(),
        }
    }
}

impl OutputJobType for InteractiveHtmlBomOutputJob {
    const TYPE_NAME: &'static str = "interactive_bom";

    fn type_tr() -> String {
        format!(
            "{} (*.html)",
            tr!(
                "InteractiveHtmlBomOutputJob",
                "Interactive Bill Of Materials"
            )
        )
    }

    fn default_name() -> ElementName {
        ElementName::from_tr(
            "InteractiveHtmlBomOutputJob",
            "Interactive Bill of Materials",
        )
    }
}

impl SerializeObject for InteractiveHtmlBomOutputJob {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        root.append_child("view_mode", &self.view_mode);
        root.append_child("highlight_pin1", &self.highlight_pin1);
        root.append_child("dark_mode", &self.dark_mode);
        root.ensure_line_break();
        root.append_child("rotation", &self.board_rotation);
        root.append_child("offset_back_rotation", &self.offset_back_rotation);
        root.ensure_line_break();
        root.append_child("show_silkscreen", &self.show_silkscreen);
        root.append_child("show_fabrication", &self.show_fabrication);
        root.ensure_line_break();
        root.append_child("show_pads", &self.show_pads);
        root.append_child("show_tracks", &self.show_tracks);
        root.append_child("show_zones", &self.show_zones);
        root.ensure_line_break();
        root.append_child("checkboxes", &self.check_boxes.join(","));
        root.ensure_line_break();
        root.append_child("component_order", &self.component_order.join(","));
        root.ensure_line_break();
        for attribute in &self.custom_attributes {
            root.append_child("custom_attribute", attribute);
            root.ensure_line_break();
        }
        self.boards.serialize(root, "board");
        root.ensure_line_break();
        self.assembly_variants.serialize(root, "variant");
        root.ensure_line_break();
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for InteractiveHtmlBomOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        // Like `QString::split(",")`: an empty string gives one empty item.
        let split = |path: &str| -> serialization::Result<Vec<String>> {
            let value: String = node.child_value(path)?;
            Ok(value.split(',').map(str::to_owned).collect())
        };
        Ok(Self {
            view_mode: node.child_value("view_mode/@0")?,
            highlight_pin1: node.child_value("highlight_pin1/@0")?,
            dark_mode: node.child_value("dark_mode/@0")?,
            board_rotation: node.child_value("rotation/@0")?,
            offset_back_rotation: node.child_value("offset_back_rotation/@0")?,
            show_silkscreen: node.child_value("show_silkscreen/@0")?,
            show_fabrication: node.child_value("show_fabrication/@0")?,
            show_pads: node.child_value("show_pads/@0")?,
            show_tracks: node.child_value("show_tracks/@0")?,
            show_zones: node.child_value("show_zones/@0")?,
            check_boxes: split("checkboxes/@0")?,
            component_order: split("component_order/@0")?,
            custom_attributes: node
                .children_named("custom_attribute")
                .map(|child| child.child_value("@0"))
                .collect::<serialization::Result<_>>()?,
            boards: ObjectSet::deserialize(node, "board")?,
            assembly_variants: ObjectSet::deserialize(node, "variant")?,
            output_path: node.child_value("output/@0")?,
        })
    }
}
