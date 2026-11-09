//! Port of libs/librepcb/core/job/graphicsoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields; the nested
//! `Content`, `Content::Type` and `Content::Preset` are [`GraphicsContent`],
//! [`GraphicsContentType`] and [`GraphicsContentPreset`]. The static
//! factories (`schematicPdf()`, ...) return complete [`OutputJob`]s.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJob, OutputJobType};
use crate::export::{
    GraphicsExportSettings, PageOrientation, scale_from_sexpression, scale_to_sexpression,
};
use crate::project::{AssemblyVariantId, BoardId};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::types::{self, Color, ElementName, SimpleString, UnsignedLength, UnsignedRatio, Uuid};

/// Boards of a graphics content (`None` = no board, e.g. for schematics).
pub type GraphicsBoardSet = ObjectSet<Option<BoardId>>;

/// Assembly variants of a graphics content (`None` = no variant).
pub type GraphicsAssemblyVariantSet = ObjectSet<Option<AssemblyVariantId>>;

/// What a [`GraphicsContent`] shows (upstream
/// `GraphicsOutputJob::Content::Type`).
///
/// Serde: the file format token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GraphicsContentType {
    /// Schematic pages.
    #[default]
    Schematic,
    /// Boards (layers with configurable colors).
    Board,
    /// Realistic board rendering.
    BoardRendering,
    /// Reserved for future use.
    AssemblyGuide,
}

impl GraphicsContentType {
    /// Returns the file format token.
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Schematic => "schematic",
            Self::Board => "board",
            Self::BoardRendering => "board_rendering",
            Self::AssemblyGuide => "assembly_guide",
        }
    }
}

impl fmt::Display for GraphicsContentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_str())
    }
}

impl FromStr for GraphicsContentType {
    type Err = types::Error;
    fn from_str(s: &str) -> Result<Self, types::Error> {
        [
            Self::Schematic,
            Self::Board,
            Self::BoardRendering,
            Self::AssemblyGuide,
        ]
        .into_iter()
        .find(|v| v.to_str() == s)
        .ok_or_else(|| types::Error::InvalidGraphicsContentType(s.to_owned()))
    }
}

crate::utils::serde_string::serde_string!(GraphicsContentType);

impl ToSExpression for GraphicsContentType {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_str())
    }
}

impl FromSExpression for GraphicsContentType {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

/// Presets for new contents (upstream `GraphicsOutputJob::Content::Preset`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphicsContentPreset {
    /// Defaults without layers.
    None,
    /// All schematic pages.
    Schematic,
    /// Board with copper layers.
    BoardImage,
    /// Board assembly drawing, top side.
    BoardAssemblyTop,
    /// Board assembly drawing, bottom side.
    BoardAssemblyBottom,
    /// Realistic board rendering, top side.
    BoardRenderingTop,
    /// Realistic board rendering, bottom side.
    BoardRenderingBottom,
}

/// One content (page set) of a graphics output job (upstream
/// `GraphicsOutputJob::Content`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphicsContent {
    /// What is exported.
    pub content_type: GraphicsContentType,
    /// Title (for the UI and PDF bookmarks).
    pub title: String,
    /// Page size key (e.g. `"A4"`), `None` = automatic.
    pub page_size: Option<String>,
    /// Page orientation.
    pub orientation: PageOrientation,
    /// Left page margin.
    pub margin_left: UnsignedLength,
    /// Top page margin.
    pub margin_top: UnsignedLength,
    /// Right page margin.
    pub margin_right: UnsignedLength,
    /// Bottom page margin.
    pub margin_bottom: UnsignedLength,
    /// Whether the content is rotated by 90°.
    pub rotate: bool,
    /// Whether the content is mirrored.
    pub mirror: bool,
    /// Scale factor, `None` = fit into page.
    pub scale: Option<UnsignedRatio>,
    /// Resolution of pixmap exports.
    pub pixmap_dpi: u32,
    /// Whether everything is exported in black/white.
    pub monochrome: bool,
    /// Background color.
    pub background_color: Color,
    /// Minimum line width.
    pub min_line_width: UnsignedLength,
    /// Colors of the exported color roles (e.g. `"board_outlines"`).
    pub layers: BTreeMap<String, Color>,
    /// Boards to export.
    pub boards: GraphicsBoardSet,
    /// Assembly variants to export.
    pub assembly_variants: GraphicsAssemblyVariantSet,
    /// Arbitrary options for forward compatibility (`option` children, by
    /// their first value), written back unchanged. Supported options: none.
    pub options: BTreeMap<String, Vec<SExpression>>,
}

impl GraphicsContent {
    /// Creates a content from a preset (upstream constructor
    /// `Content(Preset)`).
    pub fn new(preset: GraphicsContentPreset) -> Self {
        use GraphicsContentPreset as P;
        let defaults = GraphicsExportSettings::default();
        let mut content = Self {
            content_type: GraphicsContentType::Schematic,
            title: String::new(),
            page_size: None,
            orientation: PageOrientation::Auto,
            margin_left: defaults.margin_left,
            margin_top: defaults.margin_top,
            margin_right: defaults.margin_right,
            margin_bottom: defaults.margin_bottom,
            rotate: false,
            mirror: false,
            scale: None,
            pixmap_dpi: 600,
            monochrome: false,
            background_color: Color::TRANSPARENT,
            min_line_width: defaults.min_line_width,
            layers: BTreeMap::new(),
            boards: ObjectSet::custom([None]),
            assembly_variants: ObjectSet::custom([None]),
            options: BTreeMap::new(),
        };
        let mut enabled: BTreeSet<&str> = BTreeSet::new();
        match preset {
            P::None => {}
            P::Schematic => {
                content.content_type = GraphicsContentType::Schematic;
                content.title = tr!("GraphicsOutputJob", "Schematic");
                enabled.extend([
                    "schematic_frames",
                    "schematic_wires",
                    "schematic_net_labels",
                    "schematic_buses",
                    "schematic_bus_labels",
                    "schematic_image_borders",
                    "schematic_documentation",
                    "schematic_comments",
                    "schematic_guide",
                    "schematic_outlines",
                    "schematic_grab_areas",
                    "schematic_names",
                    "schematic_values",
                    "schematic_pin_lines",
                    "schematic_pin_names",
                    "schematic_pin_numbers",
                ]);
            }
            _ => {
                let (content_type, title, mirror) = match preset {
                    P::BoardAssemblyTop => (
                        GraphicsContentType::Board,
                        tr!("GraphicsOutputJob", "Assembly Top"),
                        false,
                    ),
                    P::BoardAssemblyBottom => (
                        GraphicsContentType::Board,
                        tr!("GraphicsOutputJob", "Assembly Bottom"),
                        true,
                    ),
                    P::BoardRenderingTop => (
                        GraphicsContentType::BoardRendering,
                        tr!("GraphicsOutputJob", "Rendering Top"),
                        false,
                    ),
                    P::BoardRenderingBottom => (
                        GraphicsContentType::BoardRendering,
                        tr!("GraphicsOutputJob", "Rendering Bottom"),
                        true,
                    ),
                    _ => (
                        GraphicsContentType::Board,
                        tr!("GraphicsOutputJob", "Board"),
                        false,
                    ),
                };
                content.content_type = content_type;
                content.title = title;
                content.mirror = mirror;
                content.boards = ObjectSet::Default;
                match preset {
                    P::BoardRenderingTop => enabled.extend([
                        "board_outlines",
                        "board_copper_top",
                        "board_stop_mask_top",
                        "board_legend_top",
                    ]),
                    P::BoardRenderingBottom => enabled.extend([
                        "board_outlines",
                        "board_copper_bottom",
                        "board_stop_mask_bottom",
                        "board_legend_bottom",
                    ]),
                    _ => {
                        enabled.extend([
                            "board_frames",
                            "board_outlines",
                            "board_plated_cutouts",
                            "board_holes",
                            "board_pads",
                            "board_measures",
                            "board_documentation",
                            "board_comments",
                            "board_guide",
                        ]);
                        if preset != P::BoardAssemblyBottom {
                            enabled.extend([
                                "board_legend_top",
                                "board_documentation_top",
                                "board_grab_areas_top",
                                "board_names_top",
                                "board_values_top",
                            ]);
                        }
                        if preset != P::BoardAssemblyTop {
                            enabled.extend([
                                "board_legend_bottom",
                                "board_documentation_bottom",
                                "board_grab_areas_bottom",
                                "board_names_bottom",
                                "board_values_bottom",
                            ]);
                        }
                        if preset == P::BoardImage {
                            enabled.extend([
                                "board_vias",
                                "board_copper_top",
                                "board_copper_bottom",
                            ]);
                        }
                    }
                }
            }
        }
        let mut colors = defaults;
        if matches!(preset, P::BoardRenderingTop | P::BoardRenderingBottom) {
            colors.load_board_rendering_colors(0);
        }
        for (role, color) in colors.colors {
            if enabled.contains(role.as_str()) {
                content.layers.insert(role, color);
            }
        }
        content
    }
}

impl GraphicsContent {
    /// Loads a content from a `content` node.
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let page_size: String = node.child_value("paper/@0")?;
        let mut layers = BTreeMap::new();
        for layer in node.children_named("layer") {
            layers.insert(layer.child_value("@0")?, layer.child_value("color/@0")?);
        }
        let mut options: BTreeMap<String, Vec<SExpression>> = BTreeMap::new();
        for option in node.children_named("option") {
            options
                .entry(option.child_value("@0")?)
                .or_default()
                .push(option.clone());
        }
        Ok(Self {
            content_type: node.child_value("type/@0")?,
            title: node.child_value("title/@0")?,
            page_size: (page_size != "auto").then_some(page_size),
            orientation: node.child_value("orientation/@0")?,
            margin_left: node.child_value("margins/left/@0")?,
            margin_top: node.child_value("margins/top/@0")?,
            margin_right: node.child_value("margins/right/@0")?,
            margin_bottom: node.child_value("margins/bottom/@0")?,
            mirror: node.child_value("mirror/@0")?,
            rotate: node.child_value("rotate/@0")?,
            scale: scale_from_sexpression(node.required_child("scale/@0")?)?,
            pixmap_dpi: node.child_value("dpi/@0")?,
            monochrome: node.child_value("monochrome/@0")?,
            background_color: node.child_value("background/@0")?,
            min_line_width: node.child_value("min_line_width/@0")?,
            layers,
            boards: ObjectSet::deserialize(node, "board")?,
            assembly_variants: ObjectSet::deserialize(node, "variant")?,
            options,
        })
    }

    /// Serializes the content into a `content` node.
    fn serialize(&self, node: &mut List) {
        node.append_child("type", &self.content_type);
        node.append_child("title", &self.title);
        node.ensure_line_break();
        match &self.page_size {
            Some(key) => node.append_child("paper", key),
            None => node.append_child("paper", &SExpression::token("auto")),
        };
        node.append_child("orientation", &self.orientation);
        node.append_child("rotate", &self.rotate);
        node.append_child("mirror", &self.mirror);
        node.append_child("scale", &scale_to_sexpression(self.scale));
        node.ensure_line_break();
        let margins = node.append_list("margins");
        margins.append_child("left", &self.margin_left);
        margins.append_child("top", &self.margin_top);
        margins.append_child("right", &self.margin_right);
        margins.append_child("bottom", &self.margin_bottom);
        node.ensure_line_break();
        node.append_child("dpi", &self.pixmap_dpi);
        node.append_child("min_line_width", &self.min_line_width);
        node.append_child("monochrome", &self.monochrome);
        node.append_child("background", &self.background_color);
        node.ensure_line_break();
        for (role, color) in &self.layers {
            let layer = node.append_list("layer");
            layer.append_value(&SExpression::token(role));
            layer.append_child("color", color);
            node.ensure_line_break();
        }
        self.boards.serialize(node, "board");
        node.ensure_line_break();
        self.assembly_variants.serialize(node, "variant");
        for option in self.options.values().flatten() {
            node.ensure_line_break();
            node.push(option.clone());
        }
        node.ensure_line_break();
    }
}

/// Settings of a PDF/SVG/image output job (upstream `GraphicsOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphicsOutputJob {
    /// Document title (with placeholders like `{{PROJECT}}`).
    pub document_title: SimpleString,
    /// The contents (page sets).
    pub content: Vec<GraphicsContent>,
    /// Output file path (with placeholders).
    pub output_path: String,
}

impl Default for GraphicsOutputJob {
    fn default() -> Self {
        Self {
            // Valid constant.
            document_title: SimpleString::new("{{PROJECT}} - {{VERSION}}").expect("valid constant"),
            content: Vec::new(),
            output_path: "{{PROJECT}}_{{VERSION}}.pdf".into(),
        }
    }
}

impl GraphicsOutputJob {
    fn preset(name: &'static str, content: Vec<GraphicsContent>, output: &str) -> OutputJob {
        let job = Self {
            content,
            output_path: output.into(),
            ..Self::default()
        };
        OutputJob::new(
            Uuid::new_random(),
            ElementName::from_tr("GraphicsOutputJob", name),
            job,
        )
    }

    /// Creates a job exporting the schematics as PDF (upstream
    /// `schematicPdf()`).
    pub fn schematic_pdf() -> OutputJob {
        Self::preset(
            "Schematic PDF",
            vec![GraphicsContent::new(GraphicsContentPreset::Schematic)],
            "{{PROJECT}}_{{VERSION}}_Schematic.pdf",
        )
    }

    /// Creates a job exporting assembly drawings of the default board as PDF
    /// (upstream `boardAssemblyPdf()`).
    pub fn board_assembly_pdf() -> OutputJob {
        Self::preset(
            "Board Assembly PDF",
            vec![
                GraphicsContent::new(GraphicsContentPreset::BoardAssemblyTop),
                GraphicsContent::new(GraphicsContentPreset::BoardAssemblyBottom),
            ],
            "{{PROJECT}}_{{VERSION}}_Assembly.pdf",
        )
    }

    /// Creates a job exporting realistic renderings of the default board as
    /// PDF (upstream `boardRenderingPdf()`).
    pub fn board_rendering_pdf() -> OutputJob {
        Self::preset(
            "Board Rendering PDF",
            vec![
                GraphicsContent::new(GraphicsContentPreset::BoardRenderingTop),
                GraphicsContent::new(GraphicsContentPreset::BoardRenderingBottom),
            ],
            "{{PROJECT}}_{{VERSION}}_Rendering.pdf",
        )
    }
}

impl OutputJobType for GraphicsOutputJob {
    const TYPE_NAME: &'static str = "graphics";

    fn type_tr() -> String {
        tr!("GraphicsOutputJob", "PDF/Image")
    }

    fn default_name() -> ElementName {
        // Upstream uses the untranslated name "PDF/Image".
        ElementName::new("PDF/Image").expect("valid constant")
    }
}

impl SerializeObject for GraphicsOutputJob {
    fn serialize(&self, root: &mut List) {
        root.append_child("title", &self.document_title);
        root.ensure_line_break();
        for content in &self.content {
            content.serialize(root.append_list("content"));
            root.ensure_line_break();
        }
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for GraphicsOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            document_title: node.child_value("title/@0")?,
            content: node
                .children_named("content")
                .map(GraphicsContent::deserialize)
                .collect::<serialization::Result<_>>()?,
            output_path: node.child_value("output/@0")?,
        })
    }
}
