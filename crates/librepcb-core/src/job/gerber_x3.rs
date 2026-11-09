//! Port of libs/librepcb/core/job/gerberx3outputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJobType};
use crate::project::{AssemblyVariantId, BoardId};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// Settings of a pick&place / glue mask Gerber X3 output job (upstream
/// `GerberX3OutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GerberX3OutputJob {
    /// Boards to export.
    pub boards: ObjectSet<BoardId>,
    /// Assembly variants to export.
    pub assembly_variants: ObjectSet<AssemblyVariantId>,
    /// Whether the top components file is created.
    pub enable_components_top: bool,
    /// Whether the bottom components file is created.
    pub enable_components_bot: bool,
    /// Output path of the top components file.
    pub output_path_components_top: String,
    /// Output path of the bottom components file.
    pub output_path_components_bot: String,
    /// Whether the top glue mask file is created.
    pub enable_glue_top: bool,
    /// Whether the bottom glue mask file is created.
    pub enable_glue_bot: bool,
    /// Output path of the top glue mask file.
    pub output_path_glue_top: String,
    /// Output path of the bottom glue mask file.
    pub output_path_glue_bot: String,
}

impl Default for GerberX3OutputJob {
    fn default() -> Self {
        Self {
            boards: ObjectSet::Default,
            assembly_variants: ObjectSet::All,
            enable_components_top: true,
            enable_components_bot: true,
            output_path_components_top: "assembly/{{PROJECT}}_{{VERSION}}_PnP_{{VARIANT}}_TOP.gbr"
                .into(),
            output_path_components_bot: "assembly/{{PROJECT}}_{{VERSION}}_PnP_{{VARIANT}}_BOT.gbr"
                .into(),
            enable_glue_top: false,
            enable_glue_bot: false,
            output_path_glue_top: "assembly/{{PROJECT}}_{{VERSION}}_GLUE_{{VARIANT}}_TOP.gbr"
                .into(),
            output_path_glue_bot: "assembly/{{PROJECT}}_{{VERSION}}_GLUE_{{VARIANT}}_BOT.gbr"
                .into(),
        }
    }
}

impl OutputJobType for GerberX3OutputJob {
    const TYPE_NAME: &'static str = "gerber_x3";

    fn type_tr() -> String {
        tr!("GerberX3OutputJob", "Pick&Place / Glue Mask (Gerber X3)")
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("GerberX3OutputJob", "Pick&Place / Glue Mask")
    }
}

impl SerializeObject for GerberX3OutputJob {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        self.boards.serialize(root, "board");
        root.ensure_line_break();
        self.assembly_variants.serialize(root, "variant");
        let files = [
            (
                "components_top",
                self.enable_components_top,
                &self.output_path_components_top,
            ),
            (
                "components_bot",
                self.enable_components_bot,
                &self.output_path_components_bot,
            ),
            ("glue_top", self.enable_glue_top, &self.output_path_glue_top),
            ("glue_bot", self.enable_glue_bot, &self.output_path_glue_bot),
        ];
        for (name, create, output) in files {
            root.ensure_line_break();
            let node = root.append_list(name);
            node.append_child("create", &create);
            node.append_child("output", output);
        }
        root.ensure_line_break();
    }
}

impl DeserializeObject for GerberX3OutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            boards: ObjectSet::deserialize(node, "board")?,
            assembly_variants: ObjectSet::deserialize(node, "variant")?,
            enable_components_top: node.child_value("components_top/create/@0")?,
            enable_components_bot: node.child_value("components_bot/create/@0")?,
            output_path_components_top: node.child_value("components_top/output/@0")?,
            output_path_components_bot: node.child_value("components_bot/output/@0")?,
            enable_glue_top: node.child_value("glue_top/create/@0")?,
            enable_glue_bot: node.child_value("glue_bot/create/@0")?,
            output_path_glue_top: node.child_value("glue_top/output/@0")?,
            output_path_glue_bot: node.child_value("glue_bot/output/@0")?,
        })
    }
}
