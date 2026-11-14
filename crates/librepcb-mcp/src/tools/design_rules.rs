//! Read, preview and update the native board manufacturing profile.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::library::org::{AllowedSlots, LengthPair};
use librepcb_core::project::board::{
    BoardSettings,
    drc::{DrcMessageKind, DrcResult},
};
use librepcb_core::project::{BoardId, Project};
use librepcb_core::rule_check::Severity;
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{BoundedUnsignedRatio, Layer, PcbColor, Ratio, UnsignedRatio, Uuid};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::EditBoardSettings;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::board_edit::{BoardArgs, positive, unsigned};
use super::write::{check_revision, write};
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve;
use crate::session::Session;
use crate::units::mm;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SizeMm {
    width: f64,
    height: f64,
}

impl From<LengthPair> for SizeMm {
    fn from((width, height): LengthPair) -> Self {
        Self {
            width: mm(width),
            height: mm(height),
        }
    }
}

impl SizeMm {
    fn parse(self, field: &str) -> ToolResult<LengthPair> {
        Ok((
            unsigned(self.width, &format!("{field}.width"))?,
            unsigned(self.height, &format!("{field}.height"))?,
        ))
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundedRatioMm {
    /// Percentage in integer parts per million (250000 = 25%).
    ratio_ppm: i32,
    min_mm: f64,
    max_mm: f64,
}

impl From<BoundedUnsignedRatio> for BoundedRatioMm {
    fn from(value: BoundedUnsignedRatio) -> Self {
        Self {
            ratio_ppm: value.ratio().to_ppm(),
            min_mm: mm(value.min_value()),
            max_mm: mm(value.max_value()),
        }
    }
}

impl BoundedRatioMm {
    fn parse(self, field: &str) -> ToolResult<BoundedUnsignedRatio> {
        let ratio = UnsignedRatio::new(Ratio::new(self.ratio_ppm))
            .map_err(|err| ToolError::invalid(format!("{field}.ratio_ppm: {err}")))?;
        let min = unsigned(self.min_mm, &format!("{field}.min_mm"))?;
        let max = unsigned(self.max_mm, &format!("{field}.max_mm"))?;
        BoundedUnsignedRatio::new(ratio, min, max)
            .map_err(|err| ToolError::invalid(format!("{field}: {err}")))
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotPolicy {
    None,
    SingleSegmentStraight,
    MultiSegmentStraight,
    Any,
}

impl From<SlotPolicy> for AllowedSlots {
    fn from(policy: SlotPolicy) -> Self {
        match policy {
            SlotPolicy::None => Self::None,
            SlotPolicy::SingleSegmentStraight => Self::SingleSegmentStraight,
            SlotPolicy::MultiSegmentStraight => Self::MultiSegmentStraight,
            SlotPolicy::Any => Self::Any,
        }
    }
}

// Map scalar dimensions and booleans once for both read-back and updates.
macro_rules! rule_fields {
    (
        $( $(#[$dim_doc:meta])* $dim:ident : $units:ident => $settings:ident.$get_dim:ident, $set_dim:ident; )*
        @bools {
            $( $(#[$flag_doc:meta])* $flag:ident => $flag_settings:ident.$get_flag:ident, $set_flag:ident; )*
        }
    ) => {
        #[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct DesignRulesSetArgs {
            /// Board name, index or UUID (default: first board).
            pub board: Option<String>,
            /// Fail with stale_revision if the project revision differs.
            pub expected_revision: Option<u64>,
            /// Number of inner copper layers (0 = 2-layer board).
            pub inner_layers: Option<u32>,
            /// Selected PCB thickness, in mm.
            pub pcb_thickness: Option<f64>,
            /// Selected solder resist color ID; use "none" for exposed copper.
            pub solder_resist: Option<String>,
            /// Selected silkscreen color ID.
            pub silkscreen_color: Option<String>,
            /// Native layer IDs included in the top silkscreen. Empty disables it.
            pub silkscreen_layers_top: Option<Vec<String>>,
            /// Native layer IDs included in the bottom silkscreen. Empty disables it.
            pub silkscreen_layers_bottom: Option<Vec<String>>,
            /// Minimum board width and height in mm. Zero leaves a dimension unrestricted.
            pub min_board_size: Option<SizeMm>,
            /// Maximum board width and height for two layers in mm.
            pub max_board_size_double_sided: Option<SizeMm>,
            /// Maximum board width and height for multilayer boards in mm.
            pub max_board_size_multi_layer: Option<SizeMm>,
            /// Allowed PCB thicknesses in mm; empty leaves all thicknesses available.
            pub allowed_pcb_thicknesses: Option<Vec<f64>>,
            /// Maximum copper layer count; 0 leaves it unrestricted.
            pub max_layer_count: Option<u32>,
            /// Allowed solder resist color IDs. "none" permits exposed copper; empty allows any.
            pub allowed_solder_resist: Option<Vec<String>>,
            /// Allowed silkscreen color IDs. "none" permits absent silkscreen; empty allows any.
            pub allowed_silkscreen: Option<Vec<String>>,
            /// Allowed types of non-plated slots.
            pub allowed_npth_slots: Option<SlotPolicy>,
            /// Allowed types of plated slots.
            pub allowed_pth_slots: Option<SlotPolicy>,
            /// Solder mask clearance as a bounded proportion of pad size.
            pub stop_mask_clearance: Option<BoundedRatioMm>,
            /// Paste shrinkage as a bounded proportion of pad size.
            pub solder_paste_clearance: Option<BoundedRatioMm>,
            /// Pad annular ring as a bounded proportion of drill size.
            pub pad_annular_ring: Option<BoundedRatioMm>,
            /// Via annular ring as a bounded proportion of drill size.
            pub via_annular_ring: Option<BoundedRatioMm>,
            $( $(#[$dim_doc])* pub $dim: Option<f64>, )*
            $( $(#[$flag_doc])* pub $flag: Option<bool>, )*
        }

        impl DesignRulesSetArgs {
            fn apply_scalars(&self, next: &mut BoardSettings) -> ToolResult<()> {
                $(
                    if let Some(value) = self.$dim {
                        next.$settings.$set_dim($units(value, stringify!($dim))?);
                    }
                )*
                $(
                    if let Some(value) = self.$flag {
                        next.$flag_settings.$set_flag(value);
                    }
                )*
                Ok(())
            }
        }

        fn profile(settings: &BoardSettings) -> BTreeMap<&'static str, Value> {
            let mut result = profile_choices(settings);
            $( result.insert(stringify!($dim), json!(mm(settings.$settings.$get_dim()))); )*
            $( result.insert(stringify!($flag), json!(settings.$flag_settings.$get_flag())); )*
            result
        }
    };
}

rule_fields! {
    /// Default routing trace width in mm.
    default_trace_width: positive => design_rules.default_trace_width, set_default_trace_width;
    /// Default via drill in mm.
    default_via_drill: positive => design_rules.default_via_drill_diameter, set_default_via_drill_diameter;
    /// Maximum via drill without a default solder mask opening, in mm.
    stop_mask_max_via_drill_diameter: unsigned => design_rules.stop_mask_max_via_drill_diameter, set_stop_mask_max_via_drill_diameter;
    /// Minimum copper-to-copper clearance in mm.
    min_copper_clearance: unsigned => drc_settings.min_copper_copper_clearance, set_min_copper_copper_clearance;
    /// Minimum copper-to-board-edge clearance in mm.
    min_copper_board_clearance: unsigned => drc_settings.min_copper_board_clearance, set_min_copper_board_clearance;
    /// Minimum copper-to-non-plated-hole clearance in mm.
    min_copper_npth_clearance: unsigned => drc_settings.min_copper_npth_clearance, set_min_copper_npth_clearance;
    /// Minimum drill-to-drill clearance in mm.
    min_drill_clearance: unsigned => drc_settings.min_drill_drill_clearance, set_min_drill_drill_clearance;
    /// Minimum drill-to-board-edge clearance in mm.
    min_drill_board_clearance: unsigned => drc_settings.min_drill_board_clearance, set_min_drill_board_clearance;
    /// Minimum silkscreen-to-solder-mask-opening clearance in mm.
    min_silkscreen_stopmask_clearance: unsigned => drc_settings.min_silkscreen_stopmask_clearance, set_min_silkscreen_stopmask_clearance;
    /// Minimum copper width in mm.
    min_copper_width: unsigned => drc_settings.min_copper_width, set_min_copper_width;
    /// Minimum plated-hole annular ring in mm.
    min_annular_ring: unsigned => drc_settings.min_pth_annular_ring, set_min_pth_annular_ring;
    /// Minimum non-plated drill diameter in mm.
    min_npth_drill_diameter: unsigned => drc_settings.min_npth_drill_diameter, set_min_npth_drill_diameter;
    /// Minimum plated drill diameter in mm.
    min_drill_diameter: unsigned => drc_settings.min_pth_drill_diameter, set_min_pth_drill_diameter;
    /// Minimum non-plated slot width in mm.
    min_npth_slot_width: unsigned => drc_settings.min_npth_slot_width, set_min_npth_slot_width;
    /// Minimum plated slot width in mm.
    min_pth_slot_width: unsigned => drc_settings.min_pth_slot_width, set_min_pth_slot_width;
    /// Maximum permitted drill diameter for tented vias, in mm.
    max_tented_via_drill_diameter: unsigned => drc_settings.max_tented_via_drill_diameter, set_max_tented_via_drill_diameter;
    /// Minimum silkscreen stroke or polygon line width, in mm.
    min_silkscreen_width: unsigned => drc_settings.min_silkscreen_width, set_min_silkscreen_width;
    /// Minimum silkscreen text height in mm.
    min_silkscreen_text_height: unsigned => drc_settings.min_silkscreen_text_height, set_min_silkscreen_text_height;
    /// Minimum outline milling tool diameter in mm.
    min_outline_tool_diameter: unsigned => drc_settings.min_outline_tool_diameter, set_min_outline_tool_diameter;
    @bools {
        /// Use an automatically sized annular ring on the component side.
        pad_cmp_side_auto_annular_ring => design_rules.pad_cmp_side_auto_annular_ring, set_pad_cmp_side_auto_annular_ring;
        /// Use automatically sized annular rings on inner layers.
        pad_inner_auto_annular_ring => design_rules.pad_inner_auto_annular_ring, set_pad_inner_auto_annular_ring;
        /// Allow blind vias in DRC.
        blind_vias_allowed => drc_settings.blind_vias_allowed, set_blind_vias_allowed;
        /// Allow buried vias in DRC.
        buried_vias_allowed => drc_settings.buried_vias_allowed, set_buried_vias_allowed;
    }
}

fn color_id(color: Option<PcbColor>) -> &'static str {
    color.map_or("none", PcbColor::id)
}

fn color(id: &str) -> ToolResult<PcbColor> {
    PcbColor::from_id(id).ok_or_else(|| ToolError::invalid(format!("Unknown PCB color {id:?}.")))
}

fn optional_color(id: &str) -> ToolResult<Option<PcbColor>> {
    if id == "none" {
        Ok(None)
    } else {
        color(id).map(Some)
    }
}

fn layer_ids(layers: &[Layer]) -> Vec<&'static str> {
    layers.iter().map(|layer| layer.id()).collect()
}

fn silkscreen_layers(ids: &[String]) -> ToolResult<Vec<Layer>> {
    ids.iter()
        .map(|id| id.parse().map_err(ToolError::from))
        .collect()
}

fn profile_choices(s: &BoardSettings) -> BTreeMap<&'static str, Value> {
    let d = &s.drc_settings;
    let r = &s.design_rules;
    BTreeMap::from([
        ("inner_layers", json!(s.inner_layer_count)),
        ("pcb_thickness", json!(mm(s.pcb_thickness))),
        ("solder_resist", json!(color_id(s.solder_resist))),
        ("silkscreen_color", json!(s.silkscreen_color.id())),
        (
            "silkscreen_layers_top",
            json!(layer_ids(&s.silkscreen_layers_top)),
        ),
        (
            "silkscreen_layers_bottom",
            json!(layer_ids(&s.silkscreen_layers_bot)),
        ),
        ("min_board_size", json!(SizeMm::from(d.min_board_size()))),
        (
            "max_board_size_double_sided",
            json!(SizeMm::from(d.max_board_size_double_sided())),
        ),
        (
            "max_board_size_multi_layer",
            json!(SizeMm::from(d.max_board_size_multi_layer())),
        ),
        (
            "allowed_pcb_thicknesses",
            json!(
                d.pcb_thickness()
                    .iter()
                    .copied()
                    .map(mm)
                    .collect::<Vec<_>>()
            ),
        ),
        ("max_layer_count", json!(d.max_layer_count())),
        (
            "allowed_solder_resist",
            json!(
                d.solder_resist()
                    .iter()
                    .copied()
                    .map(color_id)
                    .collect::<BTreeSet<_>>()
            ),
        ),
        (
            "allowed_silkscreen",
            json!(
                d.silkscreen()
                    .iter()
                    .copied()
                    .map(color_id)
                    .collect::<BTreeSet<_>>()
            ),
        ),
        ("allowed_npth_slots", json!(d.allowed_npth_slots())),
        ("allowed_pth_slots", json!(d.allowed_pth_slots())),
        (
            "stop_mask_clearance",
            json!(BoundedRatioMm::from(r.stop_mask_clearance())),
        ),
        (
            "solder_paste_clearance",
            json!(BoundedRatioMm::from(r.solder_paste_clearance())),
        ),
        (
            "pad_annular_ring",
            json!(BoundedRatioMm::from(r.pad_annular_ring())),
        ),
        (
            "via_annular_ring",
            json!(BoundedRatioMm::from(r.via_annular_ring())),
        ),
    ])
}

impl DesignRulesSetArgs {
    fn patch(&self, current: &BoardSettings) -> ToolResult<BoardSettings> {
        let mut next = current.clone();
        self.apply_scalars(&mut next)?;
        if let Some(count) = self.inner_layers {
            if count as usize > Layer::INNER_COPPER_COUNT {
                return Err(ToolError::invalid(format!(
                    "At most {} inner layers are supported.",
                    Layer::INNER_COPPER_COUNT
                )));
            }
            next.inner_layer_count = count;
        }
        if let Some(value) = self.pcb_thickness {
            next.pcb_thickness = positive(value, "pcb_thickness")?;
        }
        if let Some(value) = &self.solder_resist {
            next.solder_resist = optional_color(value)?;
        }
        if let Some(value) = &self.silkscreen_color {
            next.silkscreen_color = color(value)?;
        }
        if let Some(value) = &self.silkscreen_layers_top {
            next.silkscreen_layers_top = silkscreen_layers(value)?;
        }
        if let Some(value) = &self.silkscreen_layers_bottom {
            next.silkscreen_layers_bot = silkscreen_layers(value)?;
        }
        let d = &mut next.drc_settings;
        if let Some(value) = self.min_board_size {
            d.set_min_board_size(value.parse("min_board_size")?);
        }
        if let Some(value) = self.max_board_size_double_sided {
            d.set_max_board_size_double_sided(value.parse("max_board_size_double_sided")?);
        }
        if let Some(value) = self.max_board_size_multi_layer {
            d.set_max_board_size_multi_layer(value.parse("max_board_size_multi_layer")?);
        }
        if let Some(values) = &self.allowed_pcb_thicknesses {
            d.set_pcb_thickness(
                values
                    .iter()
                    .map(|&value| positive(value, "allowed_pcb_thicknesses"))
                    .collect::<ToolResult<_>>()?,
            );
        }
        if let Some(value) = self.max_layer_count {
            d.set_max_layer_count(value);
        }
        if let Some(values) = &self.allowed_solder_resist {
            d.set_solder_resist(
                values
                    .iter()
                    .map(|value| optional_color(value))
                    .collect::<ToolResult<_>>()?,
            );
        }
        if let Some(values) = &self.allowed_silkscreen {
            d.set_silkscreen(
                values
                    .iter()
                    .map(|value| optional_color(value))
                    .collect::<ToolResult<_>>()?,
            );
        }
        if let Some(value) = self.allowed_npth_slots {
            d.set_allowed_npth_slots(value.into());
        }
        if let Some(value) = self.allowed_pth_slots {
            d.set_allowed_pth_slots(value.into());
        }
        let r = &mut next.design_rules;
        if let Some(value) = self.stop_mask_clearance {
            r.set_stop_mask_clearance(value.parse("stop_mask_clearance")?);
        }
        if let Some(value) = self.solder_paste_clearance {
            r.set_solder_paste_clearance(value.parse("solder_paste_clearance")?);
        }
        if let Some(value) = self.pad_annular_ring {
            r.set_pad_annular_ring(value.parse("pad_annular_ring")?);
        }
        if let Some(value) = self.via_annular_ring {
            r.set_via_annular_ring(value.parse("via_annular_ring")?);
        }
        Ok(next)
    }
}

fn readback(settings: &BoardSettings) -> Value {
    let values = profile(settings);
    let mut report = json!({
        "copper_layers": settings.inner_layer_count + 2,
        "profile": values,
        "sources": settings.drc_settings.sources(),
        "options": settings.drc_settings.options(),
    });
    // Preserve the original flat result fields for existing callers.
    for field in [
        "default_trace_width",
        "default_via_drill",
        "min_copper_clearance",
        "min_copper_board_clearance",
        "min_copper_width",
        "min_annular_ring",
        "min_drill_diameter",
        "min_drill_clearance",
    ] {
        report[field] = values[field].clone();
    }
    report
}

pub fn get(session: &Session, args: BoardArgs) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    let (_, id, board) = resolve::board(project, args.board.as_deref())?;
    let mut report = readback(board.settings());
    report["board"] = json!(id);
    report["revision"] = json!(project.revision());
    ToolOutput::new("Read the board manufacturing profile.", report)
}

fn command(board: BoardId, settings: BoardSettings) -> EditBoardSettings {
    EditBoardSettings {
        board: Some(board),
        settings: Some(Box::new(settings)),
        design_rules: None,
        drc_settings: None,
        inner_layer_count: None,
    }
}

pub fn set(session: &mut Session, args: DesignRulesSetArgs) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    check_revision(project, args.expected_revision)?;
    let (_, id, board) = resolve::board(project, args.board.as_deref())?;
    let current = board.settings();
    let next = args.patch(current)?;
    let changed = next != *current;
    let done = write(session, "design_rules_set", args.expected_revision, |tx| {
        if changed {
            tx.run(command(id, next))?;
        }
        Ok(())
    })?;
    let open = session.project()?;
    let board = open
        .project()
        .board(id)
        .ok_or_else(|| ToolError::not_found("Updated board was not found."))?;
    let mut result = readback(board.settings());
    result["board"] = json!(id);
    done.output(
        open,
        "Read back board manufacturing settings after apply.",
        result,
    )
}

pub(crate) struct PreviewJob {
    project: Project,
    board: BoardId,
    revision: u64,
    next: BoardSettings,
    path: Option<librepcb_core::fileio::FilePath>,
}

pub(crate) struct PreviewResult {
    project: Uuid,
    path: Option<librepcb_core::fileio::FilePath>,
    saved_at: String,
    revision: u64,
    report: ToolOutput,
}

pub(crate) fn prepare_preview(
    session: &Session,
    args: DesignRulesSetArgs,
) -> ToolResult<PreviewJob> {
    let project = session.project()?.project();
    check_revision(project, args.expected_revision)?;
    let (_, board, layout) = resolve::board(project, args.board.as_deref())?;
    let next = args.patch(layout.settings())?;
    Ok(PreviewJob {
        project: project.snapshot()?,
        board,
        revision: project.revision(),
        next,
        path: project.file_path(),
    })
}

type Findings = BTreeMap<(SExpression, Severity), (DrcMessageKind, usize)>;

fn findings(result: &DrcResult, approved: &BTreeSet<SExpression>) -> Findings {
    let mut findings = BTreeMap::new();
    for m in result
        .messages
        .iter()
        .filter(|m| !approved.contains(m.approval()))
    {
        let entry = findings
            .entry((m.approval().clone(), m.message().severity()))
            .or_insert((m.kind(), 0));
        entry.1 += 1;
    }
    findings
}

fn counts(findings: &Findings) -> Value {
    let (mut errors, mut warnings, mut hints) = (0, 0, 0);
    for ((_, severity), (_, count)) in findings {
        match severity {
            Severity::Error => errors += count,
            Severity::Warning => warnings += count,
            Severity::Hint => hints += count,
        }
    }
    json!({"errors":errors,"warnings":warnings,"hints":hints})
}

fn delta(source: &Findings, other: &Findings) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (key, (kind, count)) in source {
        let diff = count.saturating_sub(other.get(key).map_or(0, |(_, count)| *count));
        if diff != 0 {
            *counts.entry(format!("{kind:?}")).or_default() += diff;
        }
    }
    counts
}

pub(crate) fn run_preview(job: PreviewJob) -> ToolResult<PreviewResult> {
    let PreviewJob {
        mut project,
        board,
        revision,
        next,
        path,
    } = job;
    let layout = project
        .board(board)
        .ok_or_else(|| ToolError::not_found("Preview board was not found."))?;
    let before = profile(layout.settings());
    let after = profile(&next);
    let approvals = layout.drc_approvals().clone();
    let changes: BTreeMap<_, _> = after
        .iter()
        .filter(|(field, value)| before[*field] != **value)
        .map(|(field, value)| (*field, json!({"before": before[field], "after":value})))
        .collect();
    let check = |project: &mut Project| {
        project
            .run_drc(board, None, false, &|_| {})
            .map_err(|error| ToolError::internal(format!("The DRC could not run: {error}")))
    };
    let checked_before = check(&mut project)?;
    let mut editor = ProjectEditor::new(project);
    let checked_after = if changes.is_empty() {
        None
    } else {
        editor.execute(command(board, next))?;
        Some(editor.update_derived_data(check)??)
    };
    let checked_after = checked_after.as_ref().unwrap_or(&checked_before);
    let before_findings = findings(&checked_before, &approvals);
    let after_findings = findings(checked_after, &approvals);
    let checks_complete = checked_before.is_complete() && checked_after.is_complete();
    let mut report = ToolOutput::new(
        "Previewed board manufacturing settings and checked the board.",
        json!({
            "board": board,
            "base_revision": revision,
            "profile_changes": changes,
            "profile": after,
            "drc": {
                "complete": checks_complete,
                "before": counts(&before_findings), "after": counts(&after_findings),
                "introduced_by_kind": checks_complete.then(|| delta(&after_findings, &before_findings)),
                "resolved_by_kind": checks_complete.then(|| delta(&before_findings, &after_findings)),
                "before_check_errors": checked_before.errors,
                "after_check_errors": checked_after.errors,
            },
        }),
    )?;
    if !checks_complete {
        report = report
            .partial()
            .warn("Some DRC checks failed. Finding deltas are unavailable.");
    }
    let project = editor.project();
    Ok(PreviewResult {
        project: project.uuid(),
        path,
        saved_at: project.date_time().to_rfc3339(),
        revision,
        report,
    })
}

pub(crate) fn finish_preview(session: &Session, preview: PreviewResult) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    if project.uuid() != preview.project
        || project.file_path() != preview.path
        || project.date_time().to_rfc3339() != preview.saved_at
    {
        return Err(ToolError::new(
            ErrorKind::StaleRevision,
            "The open project or save context changed during the preview.",
        ));
    }
    check_revision(project, Some(preview.revision))?;
    Ok(preview.report)
}
