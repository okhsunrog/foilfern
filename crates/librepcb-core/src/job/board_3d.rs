//! Port of libs/librepcb/core/job/board3doutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJobType};
use crate::project::{AssemblyVariantId, BoardId};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// Settings of a 3D model (STEP) output job (upstream `Board3DOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Board3DOutputJob {
    /// Boards to export.
    pub boards: ObjectSet<BoardId>,
    /// Assembly variants to export (`None` = no variant, i.e. without
    /// devices).
    pub assembly_variants: ObjectSet<Option<AssemblyVariantId>>,
    /// Output file path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for Board3DOutputJob {
    fn default() -> Self {
        Self {
            boards: ObjectSet::Default,
            assembly_variants: ObjectSet::Default,
            output_path: "{{PROJECT}}_{{VERSION}}.step".into(),
        }
    }
}

impl OutputJobType for Board3DOutputJob {
    const TYPE_NAME: &'static str = "3d_model";

    fn type_tr() -> String {
        format!("{} (*.step)", tr!("Board3DOutputJob", "3D Model"))
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("Board3DOutputJob", "STEP Model")
    }
}

impl SerializeObject for Board3DOutputJob {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        self.boards.serialize(root, "board");
        root.ensure_line_break();
        self.assembly_variants.serialize(root, "variant");
        root.ensure_line_break();
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for Board3DOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            boards: ObjectSet::deserialize(node, "board")?,
            assembly_variants: ObjectSet::deserialize(node, "variant")?,
            output_path: node.child_value("output/@0")?,
        })
    }
}
