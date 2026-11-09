//! Port of libs/librepcb/core/job/lppzoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use librepcb_i18n::tr;

use super::OutputJobType;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// Settings of a project archive (`*.lppz`) output job (upstream
/// `LppzOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LppzOutputJob {
    /// Output file path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for LppzOutputJob {
    fn default() -> Self {
        Self {
            output_path: "{{PROJECT}}_{{VERSION}}.lppz".into(),
        }
    }
}

impl OutputJobType for LppzOutputJob {
    const TYPE_NAME: &'static str = "lppz";

    fn type_tr() -> String {
        format!("{} (*.lppz)", tr!("LppzOutputJob", "Project Archive"))
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("LppzOutputJob", "Project Archive")
    }
}

impl SerializeObject for LppzOutputJob {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for LppzOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            output_path: node.child_value("output/@0")?,
        })
    }
}
