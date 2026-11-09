//! Port of libs/librepcb/core/job/pickplaceoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields; the
//! `Technologies` flags are the struct [`PickPlaceTechnologies`].

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJobType};
use crate::export::PickPlaceType;
use crate::project::{AssemblyVariantId, BoardId};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// Mounting technologies exported by a pick&place job (upstream
/// `PickPlaceOutputJob::Technologies`).
///
/// Serde: an object with the boolean fields below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PickPlaceTechnologies {
    /// Pure THT parts.
    pub tht: bool,
    /// Pure SMT parts.
    pub smt: bool,
    /// Mixed THT/SMT parts.
    pub mixed: bool,
    /// Fiducials.
    pub fiducial: bool,
    /// Anything else.
    pub other: bool,
}

impl PickPlaceTechnologies {
    /// All technologies.
    pub const ALL: Self = Self {
        tht: true,
        smt: true,
        mixed: true,
        fiducial: true,
        other: true,
    };

    /// Returns whether parts of type `ty` are exported.
    pub fn contains(self, ty: PickPlaceType) -> bool {
        match ty {
            PickPlaceType::Tht => self.tht,
            PickPlaceType::Smt => self.smt,
            PickPlaceType::Mixed => self.mixed,
            PickPlaceType::Fiducial => self.fiducial,
            PickPlaceType::Other => self.other,
        }
    }
}

impl Default for PickPlaceTechnologies {
    fn default() -> Self {
        Self::ALL
    }
}

/// Settings of a pick&place CSV output job (upstream `PickPlaceOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PickPlaceOutputJob {
    /// Exported mounting technologies.
    pub technologies: PickPlaceTechnologies,
    /// Whether a comment header is written.
    pub include_comment: bool,
    /// Boards to export.
    pub boards: ObjectSet<BoardId>,
    /// Assembly variants to export.
    pub assembly_variants: ObjectSet<AssemblyVariantId>,
    /// Whether the top side file is created.
    pub create_top: bool,
    /// Whether the bottom side file is created.
    pub create_bottom: bool,
    /// Whether the file for both sides is created.
    pub create_both: bool,
    /// Output path of the top side file.
    pub output_path_top: String,
    /// Output path of the bottom side file.
    pub output_path_bottom: String,
    /// Output path of the file for both sides.
    pub output_path_both: String,
}

impl Default for PickPlaceOutputJob {
    fn default() -> Self {
        Self {
            technologies: PickPlaceTechnologies::ALL,
            include_comment: true,
            boards: ObjectSet::Default,
            assembly_variants: ObjectSet::All,
            create_top: true,
            create_bottom: true,
            create_both: false,
            output_path_top: "assembly/{{PROJECT}}_{{VERSION}}_PnP_{{VARIANT}}_TOP.csv".into(),
            output_path_bottom: "assembly/{{PROJECT}}_{{VERSION}}_PnP_{{VARIANT}}_BOT.csv".into(),
            output_path_both: "assembly/{{PROJECT}}_{{VERSION}}_PnP_{{VARIANT}}.csv".into(),
        }
    }
}

impl OutputJobType for PickPlaceOutputJob {
    const TYPE_NAME: &'static str = "pnp";

    fn type_tr() -> String {
        format!("{} (*.csv)", tr!("PickPlaceOutputJob", "Pick&Place"))
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("PickPlaceOutputJob", "Pick&Place CSV")
    }
}

impl SerializeObject for PickPlaceOutputJob {
    fn serialize(&self, root: &mut List) {
        root.append_child("comment", &self.include_comment);
        root.ensure_line_break();
        root.append_child("tht", &self.technologies.tht);
        root.append_child("smt", &self.technologies.smt);
        root.append_child("mixed", &self.technologies.mixed);
        root.append_child("fiducial", &self.technologies.fiducial);
        root.append_child("other", &self.technologies.other);
        root.ensure_line_break();
        self.boards.serialize(root, "board");
        root.ensure_line_break();
        self.assembly_variants.serialize(root, "variant");
        let files = [
            ("top", self.create_top, &self.output_path_top),
            ("bottom", self.create_bottom, &self.output_path_bottom),
            ("both", self.create_both, &self.output_path_both),
        ];
        for (name, create, output) in files {
            root.ensure_line_break();
            let node = root.append_list(name);
            node.append_child("create", &create);
            node.append_child("output", output);
        }
    }
}

impl DeserializeObject for PickPlaceOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            technologies: PickPlaceTechnologies {
                tht: node.child_value("tht/@0")?,
                smt: node.child_value("smt/@0")?,
                mixed: node.child_value("mixed/@0")?,
                fiducial: node.child_value("fiducial/@0")?,
                other: node.child_value("other/@0")?,
            },
            include_comment: node.child_value("comment/@0")?,
            boards: ObjectSet::deserialize(node, "board")?,
            assembly_variants: ObjectSet::deserialize(node, "variant")?,
            create_top: node.child_value("top/create/@0")?,
            create_bottom: node.child_value("bottom/create/@0")?,
            create_both: node.child_value("both/create/@0")?,
            output_path_top: node.child_value("top/output/@0")?,
            output_path_bottom: node.child_value("bottom/output/@0")?,
            output_path_both: node.child_value("both/output/@0")?,
        })
    }
}
