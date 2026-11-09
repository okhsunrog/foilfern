//! Port of libs/librepcb/core/job/netlistoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJobType};
use crate::project::BoardId;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// Settings of an IPC-D-356A netlist output job (upstream
/// `NetlistOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetlistOutputJob {
    /// Boards to export.
    pub boards: ObjectSet<BoardId>,
    /// Output file path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for NetlistOutputJob {
    fn default() -> Self {
        Self {
            boards: ObjectSet::Default,
            output_path: "{{PROJECT}}_{{VERSION}}_Netlist.d356".into(),
        }
    }
}

impl OutputJobType for NetlistOutputJob {
    const TYPE_NAME: &'static str = "netlist";

    fn type_tr() -> String {
        format!("{} (*.d356)", tr!("NetlistOutputJob", "Netlist"))
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("NetlistOutputJob", "Netlist")
    }
}

impl SerializeObject for NetlistOutputJob {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        self.boards.serialize(root, "board");
        root.ensure_line_break();
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for NetlistOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            boards: ObjectSet::deserialize(node, "board")?,
            output_path: node.child_value("output/@0")?,
        })
    }
}
