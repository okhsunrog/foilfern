//! Port of libs/librepcb/core/job/archiveoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields;
//! `getDependencies()`/`removeDependency()` are
//! [`OutputJobKind::dependencies()`](super::OutputJobKind::dependencies) and
//! [`OutputJobKind::remove_dependency()`](super::OutputJobKind::remove_dependency).

use std::collections::BTreeMap;

use librepcb_i18n::tr;

use super::OutputJobType;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{ElementName, Uuid};

/// Settings of an output job which packs the output of other jobs into a
/// ZIP archive (upstream `ArchiveOutputJob`).
///
/// Serde: an object with the fields below (`input_jobs` as object keyed by
/// job UUID).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArchiveOutputJob {
    /// Input jobs (by UUID) and their destination directory in the archive.
    pub input_jobs: BTreeMap<Uuid, String>,
    /// Output file path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for ArchiveOutputJob {
    fn default() -> Self {
        Self {
            input_jobs: BTreeMap::new(),
            output_path: "{{PROJECT}}_{{VERSION}}.zip".into(),
        }
    }
}

impl OutputJobType for ArchiveOutputJob {
    const TYPE_NAME: &'static str = "archive";

    fn type_tr() -> String {
        format!("{} (*.zip)", tr!("ArchiveOutputJob", "Archive"))
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("ArchiveOutputJob", "Output Archive")
    }
}

impl SerializeObject for ArchiveOutputJob {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        for (job, destination) in &self.input_jobs {
            let child = root.append_list("input");
            child.append_value(job);
            child.append_child("destination", destination);
            root.ensure_line_break();
        }
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for ArchiveOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let mut input_jobs = BTreeMap::new();
        for child in node.children_named("input") {
            input_jobs.insert(
                child.child_value("@0")?,
                child.child_value("destination/@0")?,
            );
        }
        Ok(Self {
            input_jobs,
            output_path: node.child_value("output/@0")?,
        })
    }
}
