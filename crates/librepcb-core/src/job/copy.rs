//! Port of libs/librepcb/core/job/copyoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJobType};
use crate::project::{AssemblyVariantId, BoardId};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// Settings of a file copy output job (upstream `CopyOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CopyOutputJob {
    /// Whether `{{VARIABLES}}` in the file content are substituted.
    pub substitute_variables: bool,
    /// Boards to run for (for the variable substitution, `None` = no
    /// board).
    pub boards: ObjectSet<Option<BoardId>>,
    /// Assembly variants to run for (`None` = no variant).
    pub assembly_variants: ObjectSet<Option<AssemblyVariantId>>,
    /// Input file path, relative to the project directory.
    pub input_path: String,
    /// Output file path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for CopyOutputJob {
    fn default() -> Self {
        Self {
            substitute_variables: false,
            boards: ObjectSet::custom([None]),
            assembly_variants: ObjectSet::custom([None]),
            input_path: "resources/template.txt".into(),
            output_path: "{{PROJECT}}_{{VERSION}}.txt".into(),
        }
    }
}

impl OutputJobType for CopyOutputJob {
    const TYPE_NAME: &'static str = "copy";

    fn type_tr() -> String {
        tr!("CopyOutputJob", "File Copy")
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("CopyOutputJob", "Custom File")
    }
}

impl SerializeObject for CopyOutputJob {
    fn serialize(&self, root: &mut List) {
        root.append_child("substitute_variables", &self.substitute_variables);
        root.ensure_line_break();
        self.boards.serialize(root, "board");
        root.ensure_line_break();
        self.assembly_variants.serialize(root, "variant");
        root.ensure_line_break();
        root.append_child("input", &self.input_path);
        root.ensure_line_break();
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for CopyOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            substitute_variables: node.child_value("substitute_variables/@0")?,
            boards: ObjectSet::deserialize(node, "board")?,
            assembly_variants: ObjectSet::deserialize(node, "variant")?,
            input_path: node.child_value("input/@0")?,
            output_path: node.child_value("output/@0")?,
        })
    }
}
