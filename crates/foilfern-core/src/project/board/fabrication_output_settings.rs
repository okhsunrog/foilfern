//! Port of libs/librepcb/core/project/board/boardfabricationoutputsettings.{h,cpp}.
//!
//! Differences to upstream: plain data with public fields.

use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};

/// Settings of the Gerber/Excellon fabrication output of a board.
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardFabricationOutputSettings {
    /// Output base path (with placeholders like `{{PROJECT}}`).
    pub output_base_path: String,
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
}

impl Default for BoardFabricationOutputSettings {
    fn default() -> Self {
        Self {
            output_base_path: "./output/{{VERSION}}/gerber/{{PROJECT}}".into(),
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
        }
    }
}

impl SerializeObject for BoardFabricationOutputSettings {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        root.append_child("base_path", &self.output_base_path);
        for (name, suffix) in [
            ("outlines", &self.suffix_outlines),
            ("copper_top", &self.suffix_copper_top),
            ("copper_inner", &self.suffix_copper_inner),
            ("copper_bot", &self.suffix_copper_bot),
            ("soldermask_top", &self.suffix_solder_mask_top),
            ("soldermask_bot", &self.suffix_solder_mask_bot),
            ("silkscreen_top", &self.suffix_silkscreen_top),
            ("silkscreen_bot", &self.suffix_silkscreen_bot),
        ] {
            root.ensure_line_break();
            root.append_list(name).append_child("suffix", suffix);
        }
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
        let top = root.append_list("solderpaste_top");
        top.append_child("create", &self.enable_solder_paste_top);
        top.append_child("suffix", &self.suffix_solder_paste_top);
        root.ensure_line_break();
        let bot = root.append_list("solderpaste_bot");
        bot.append_child("create", &self.enable_solder_paste_bot);
        bot.append_child("suffix", &self.suffix_solder_paste_bot);
        root.ensure_line_break();
    }
}

impl DeserializeObject for BoardFabricationOutputSettings {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            output_base_path: node.child_value("base_path/@0")?,
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
        })
    }
}
