//! Port of libs/librepcb/core/job/bomoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJobType};
use crate::project::{AssemblyVariantId, BoardId};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// Settings of a bill of materials CSV output job (upstream
/// `BomOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BomOutputJob {
    /// Attribute keys of additional columns.
    pub custom_attributes: Vec<String>,
    /// Boards to export (`None` = without board, i.e. without assembly
    /// information).
    pub boards: ObjectSet<Option<BoardId>>,
    /// Assembly variants to export.
    pub assembly_variants: ObjectSet<AssemblyVariantId>,
    /// Output file path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for BomOutputJob {
    fn default() -> Self {
        Self {
            custom_attributes: Vec::new(),
            boards: ObjectSet::Default,
            assembly_variants: ObjectSet::All,
            output_path: "assembly/{{PROJECT}}_{{VERSION}}_BOM_{{VARIANT}}.csv".into(),
        }
    }
}

impl OutputJobType for BomOutputJob {
    const TYPE_NAME: &'static str = "bom";

    fn type_tr() -> String {
        format!("{} (*.csv)", tr!("BomOutputJob", "Bill Of Materials"))
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("BomOutputJob", "Bill of Materials")
    }
}

impl SerializeObject for BomOutputJob {
    fn serialize(&self, root: &mut List) {
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

impl DeserializeObject for BomOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
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
