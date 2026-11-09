//! Port of libs/librepcb/core/job/gerberexcellonoutputjob.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields; `defaultStyle()`
//! is `Default` (wrapped into an [`OutputJob`] by
//! [`GerberExcellonOutputJob::default_style()`]).

use librepcb_i18n::tr;

use super::{ObjectSet, OutputJob, OutputJobType};
use crate::project::BoardId;
use crate::project::board::BoardFabricationOutputSettings;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{ElementName, Uuid};

/// Settings of a Gerber/Excellon output job (upstream
/// `GerberExcellonOutputJob`).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GerberExcellonOutputJob {
    /// Suffix of the merged (PTH + NPTH) drill file.
    pub suffix_drills: String,
    /// Suffix of the NPTH drill file.
    pub suffix_drills_npth: String,
    /// Suffix of the PTH drill file.
    pub suffix_drills_pth: String,
    /// Suffix of the blind/buried via drill files.
    pub suffix_drills_blind_buried: String,
    /// Suffix of the board outlines file.
    pub suffix_outlines: String,
    /// Suffix of the top copper file.
    pub suffix_copper_top: String,
    /// Suffix of the inner copper files.
    pub suffix_copper_inner: String,
    /// Suffix of the bottom copper file.
    pub suffix_copper_bot: String,
    /// Suffix of the top solder mask file.
    pub suffix_solder_mask_top: String,
    /// Suffix of the bottom solder mask file.
    pub suffix_solder_mask_bot: String,
    /// Suffix of the top silkscreen file.
    pub suffix_silkscreen_top: String,
    /// Suffix of the bottom silkscreen file.
    pub suffix_silkscreen_bot: String,
    /// Suffix of the top solder paste file.
    pub suffix_solder_paste_top: String,
    /// Suffix of the bottom solder paste file.
    pub suffix_solder_paste_bot: String,
    /// Whether PTH and NPTH drills are merged into one file.
    pub merge_drill_files: bool,
    /// Whether slots are written with the G85 command.
    pub use_g85_slot_command: bool,
    /// Whether the top solder paste file is created.
    pub enable_solder_paste_top: bool,
    /// Whether the bottom solder paste file is created.
    pub enable_solder_paste_bot: bool,
    /// Boards to export.
    pub boards: ObjectSet<BoardId>,
    /// Output base path (with placeholders like `{{PROJECT}}`).
    pub output_path: String,
}

impl Default for GerberExcellonOutputJob {
    /// The "default style" configuration (upstream `defaultStyle()`).
    fn default() -> Self {
        Self {
            suffix_drills: "_DRILLS.drl".into(),
            suffix_drills_npth: "_DRILLS-NPTH.drl".into(),
            suffix_drills_pth: "_DRILLS-PTH.drl".into(),
            suffix_drills_blind_buried: "_DRILLS-PLATED-{{START_LAYER}}-{{END_LAYER}}.drl".into(),
            suffix_outlines: "_OUTLINES.gbr".into(),
            suffix_copper_top: "_COPPER-TOP.gbr".into(),
            suffix_copper_inner: "_COPPER-IN{{CU_LAYER}}.gbr".into(),
            suffix_copper_bot: "_COPPER-BOTTOM.gbr".into(),
            suffix_solder_mask_top: "_SOLDERMASK-TOP.gbr".into(),
            suffix_solder_mask_bot: "_SOLDERMASK-BOTTOM.gbr".into(),
            suffix_silkscreen_top: "_SILKSCREEN-TOP.gbr".into(),
            suffix_silkscreen_bot: "_SILKSCREEN-BOTTOM.gbr".into(),
            suffix_solder_paste_top: "_SOLDERPASTE-TOP.gbr".into(),
            suffix_solder_paste_bot: "_SOLDERPASTE-BOTTOM.gbr".into(),
            merge_drill_files: false,
            use_g85_slot_command: false,
            enable_solder_paste_top: true,
            enable_solder_paste_bot: true,
            boards: ObjectSet::Default,
            output_path: "gerber/{{PROJECT}}_{{VERSION}}".into(),
        }
    }
}

impl GerberExcellonOutputJob {
    /// Creates a job with the default style (upstream `defaultStyle()`).
    pub fn default_style() -> OutputJob {
        OutputJob::new_default::<Self>()
    }

    /// Creates a job with Protel style file names (upstream
    /// `protelStyle()`).
    pub fn protel_style() -> OutputJob {
        let job = Self {
            suffix_drills: ".drl".into(),
            suffix_drills_npth: "_NPTH.drl".into(),
            suffix_drills_pth: "_PTH.drl".into(),
            suffix_drills_blind_buried: "_L{{START_NUMBER}}-L{{END_NUMBER}}.drl".into(),
            suffix_outlines: ".gm1".into(),
            suffix_copper_top: ".gtl".into(),
            suffix_copper_inner: ".g{{CU_LAYER}}".into(),
            suffix_copper_bot: ".gbl".into(),
            suffix_solder_mask_top: ".gts".into(),
            suffix_solder_mask_bot: ".gbs".into(),
            suffix_silkscreen_top: ".gto".into(),
            suffix_silkscreen_bot: ".gbo".into(),
            suffix_solder_paste_top: ".gtp".into(),
            suffix_solder_paste_bot: ".gbp".into(),
            merge_drill_files: true,
            ..Self::default()
        };
        OutputJob::new(Uuid::new_random(), Self::default_name(), job)
    }

    /// Creates the default style job with the suffixes and options of the
    /// legacy fabrication output settings of a board (upstream
    /// `migratedBoardFabSettings()` of the output jobs dialog). The output
    /// path and boards keep their defaults.
    pub fn from_fabrication_settings(old: &BoardFabricationOutputSettings) -> Self {
        Self {
            suffix_drills: old.suffix_drills.clone(),
            suffix_drills_npth: old.suffix_drills_npth.clone(),
            suffix_drills_pth: old.suffix_drills_pth.clone(),
            suffix_drills_blind_buried: old.suffix_drills_blind_buried.clone(),
            suffix_outlines: old.suffix_outlines.clone(),
            suffix_copper_top: old.suffix_copper_top.clone(),
            suffix_copper_inner: old.suffix_copper_inner.clone(),
            suffix_copper_bot: old.suffix_copper_bot.clone(),
            suffix_solder_mask_top: old.suffix_solder_mask_top.clone(),
            suffix_solder_mask_bot: old.suffix_solder_mask_bot.clone(),
            suffix_silkscreen_top: old.suffix_silkscreen_top.clone(),
            suffix_silkscreen_bot: old.suffix_silkscreen_bot.clone(),
            suffix_solder_paste_top: old.suffix_solder_paste_top.clone(),
            suffix_solder_paste_bot: old.suffix_solder_paste_bot.clone(),
            merge_drill_files: old.merge_drill_files,
            use_g85_slot_command: old.use_g85_slot_command,
            enable_solder_paste_top: old.enable_solder_paste_top,
            enable_solder_paste_bot: old.enable_solder_paste_bot,
            ..Self::default()
        }
    }
}

impl OutputJobType for GerberExcellonOutputJob {
    const TYPE_NAME: &'static str = "gerber_excellon";

    fn type_tr() -> String {
        tr!("GerberExcellonOutputJob", "Gerber/Excellon")
    }

    fn default_name() -> ElementName {
        ElementName::from_tr("GerberExcellonOutputJob", "Gerber/Excellon")
    }
}

impl SerializeObject for GerberExcellonOutputJob {
    fn serialize(&self, root: &mut List) {
        let suffix = |root: &mut List, name: &str, suffix: &String| {
            root.ensure_line_break();
            root.append_list(name).append_child("suffix", suffix);
        };
        suffix(root, "outlines", &self.suffix_outlines);
        suffix(root, "copper_top", &self.suffix_copper_top);
        suffix(root, "copper_inner", &self.suffix_copper_inner);
        suffix(root, "copper_bot", &self.suffix_copper_bot);
        suffix(root, "soldermask_top", &self.suffix_solder_mask_top);
        suffix(root, "soldermask_bot", &self.suffix_solder_mask_bot);
        suffix(root, "silkscreen_top", &self.suffix_silkscreen_top);
        suffix(root, "silkscreen_bot", &self.suffix_silkscreen_bot);
        root.ensure_line_break();
        let paste_top = root.append_list("solderpaste_top");
        paste_top.append_child("create", &self.enable_solder_paste_top);
        paste_top.append_child("suffix", &self.suffix_solder_paste_top);
        root.ensure_line_break();
        let paste_bot = root.append_list("solderpaste_bot");
        paste_bot.append_child("create", &self.enable_solder_paste_bot);
        paste_bot.append_child("suffix", &self.suffix_solder_paste_bot);
        root.ensure_line_break();
        let drills = root.append_list("drills");
        drills.append_child("merge", &self.merge_drill_files);
        drills.ensure_line_break();
        drills.append_child("suffix_pth", &self.suffix_drills_pth);
        drills.ensure_line_break();
        drills.append_child("suffix_npth", &self.suffix_drills_npth);
        drills.ensure_line_break();
        drills.append_child("suffix_merged", &self.suffix_drills);
        drills.ensure_line_break();
        drills.append_child("suffix_buried", &self.suffix_drills_blind_buried);
        drills.ensure_line_break();
        drills.append_child("g85_slots", &self.use_g85_slot_command);
        drills.ensure_line_break();
        root.ensure_line_break();
        self.boards.serialize(root, "board");
        root.ensure_line_break();
        root.append_child("output", &self.output_path);
    }
}

impl DeserializeObject for GerberExcellonOutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            suffix_drills: node.child_value("drills/suffix_merged/@0")?,
            suffix_drills_npth: node.child_value("drills/suffix_npth/@0")?,
            suffix_drills_pth: node.child_value("drills/suffix_pth/@0")?,
            suffix_drills_blind_buried: node.child_value("drills/suffix_buried/@0")?,
            suffix_outlines: node.child_value("outlines/suffix/@0")?,
            suffix_copper_top: node.child_value("copper_top/suffix/@0")?,
            suffix_copper_inner: node.child_value("copper_inner/suffix/@0")?,
            suffix_copper_bot: node.child_value("copper_bot/suffix/@0")?,
            suffix_solder_mask_top: node.child_value("soldermask_top/suffix/@0")?,
            suffix_solder_mask_bot: node.child_value("soldermask_bot/suffix/@0")?,
            suffix_silkscreen_top: node.child_value("silkscreen_top/suffix/@0")?,
            suffix_silkscreen_bot: node.child_value("silkscreen_bot/suffix/@0")?,
            suffix_solder_paste_top: node.child_value("solderpaste_top/suffix/@0")?,
            suffix_solder_paste_bot: node.child_value("solderpaste_bot/suffix/@0")?,
            merge_drill_files: node.child_value("drills/merge/@0")?,
            use_g85_slot_command: node.child_value("drills/g85_slots/@0")?,
            enable_solder_paste_top: node.child_value("solderpaste_top/create/@0")?,
            enable_solder_paste_bot: node.child_value("solderpaste_bot/create/@0")?,
            boards: ObjectSet::deserialize(node, "board")?,
            output_path: node.child_value("output/@0")?,
        })
    }
}
