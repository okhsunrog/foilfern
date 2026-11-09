//! Port of libs/librepcb/core/job/projectjsonoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use librepcb_i18n::tr;

use super::OutputJobType;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// Settings of a project data JSON output job (upstream
/// `ProjectJsonOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectJsonOutputJob {
    /// Output file path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for ProjectJsonOutputJob {
    fn default() -> Self {
        Self {
            output_path: "{{PROJECT}}_{{VERSION}}.json".into(),
        }
    }
}

impl OutputJobType for ProjectJsonOutputJob {
    const TYPE_NAME: &'static str = "project_json";

    fn type_tr() -> String {
        format!("{} (*.json)", tr!("ProjectJsonOutputJob", "Project Data"))
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("ProjectJsonOutputJob", "Project Data")
    }
}

impl SerializeObject for ProjectJsonOutputJob {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for ProjectJsonOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            output_path: node.child_value("output/@0")?,
        })
    }
}
