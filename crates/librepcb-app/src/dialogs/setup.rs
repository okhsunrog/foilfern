//! The board setup and project setup dialogs.
//!
//! Ports of libs/librepcb/editor/project/board/boardsetupdialog.{ui,cpp}
//! and libs/librepcb/editor/project/projectsetupdialog.{ui,cpp}.
//!
//! Differences to upstream: the DRC settings cannot be loaded from the
//! PCB design rules of organizations in the workspace libraries yet (only
//! "Reset to Default Settings" and "Remove Link to Imported Settings");
//! the inner copper layer count is a combo box; the project locales are
//! listed and added by their code (e.g. `de_CH`) instead of a localized
//! language name; net classes are renamed by selecting them and editing
//! the name below the list; assembly variants are edited in the dialog and
//! applied with the other settings (upstream applies them immediately).

use librepcb_core::library::org::{AllowedSlots, BoardDesignRuleCheckSettings};
use librepcb_core::project::board::BoardDesignRules;
use librepcb_core::project::circuit::AssemblyVariant;
use librepcb_core::project::{AssemblyVariantId, BoardId, Mutation, NetClassId};
use librepcb_core::types::{
    BoundedUnsignedRatio, ElementName, FileProofName, Layer, Length, LengthUnit, PcbColor,
    PositiveLength, UnsignedLength, UnsignedRatio, Uuid,
};
use librepcb_editor::commands::{
    AddNetClass, EditBoardSettings, EditNetClass, EditProjectMetadata, EditProjectSettings,
    RemoveNetClass, RenameBoard,
};
use librepcb_i18n::tr;
use std::cell::RefCell;
use std::rc::Rc;

use super::attributes::AttributeEditor;
use super::{
    Applied, DialogContext, DialogOptions, FieldEvent, Form, FormDialog, ListAction, ListButtons,
    ListItem, transaction,
};
use crate::length_edit::steps;
use crate::project::AppProject;

/// Context of the board setup dialog's strings.
const BSD: &str = "librepcb::editor::BoardSetupDialog";

/// Context of the project setup dialog's strings.
const PSD: &str = "librepcb::editor::ProjectSetupDialog";

fn positive(form: &Form, id: &str) -> Result<PositiveLength, String> {
    PositiveLength::new(form.get_length(id)).map_err(|e| e.to_string())
}

fn unsigned(form: &Form, id: &str) -> Result<UnsignedLength, String> {
    UnsignedLength::new(form.get_length(id)).map_err(|e| e.to_string())
}

/// Upstream appends `*` to the labels of settings which are not
/// necessarily taken into account by the PCB manufacturer.
fn starred(label: &str) -> String {
    format!("{}*:", label.replace(':', ""))
}

// --- Board setup ---------------------------------------------------------------

const SLOTS: [AllowedSlots; 4] = [
    AllowedSlots::None,
    AllowedSlots::SingleSegmentStraight,
    AllowedSlots::MultiSegmentStraight,
    AllowedSlots::Any,
];

/// The silkscreen layer check boxes (id, layer).
const SILKSCREEN_LAYERS: [(&str, Layer); 6] = [
    ("silk_top_legend", Layer::TOP_LEGEND),
    ("silk_top_names", Layer::TOP_NAMES),
    ("silk_top_values", Layer::TOP_VALUES),
    ("silk_bot_legend", Layer::BOT_LEGEND),
    ("silk_bot_names", Layer::BOT_NAMES),
    ("silk_bot_values", Layer::BOT_VALUES),
];

/// The DRC length settings: id, label, steps, getter, setter.
type DrcLength = (
    &'static str,
    &'static str,
    &'static [Length],
    fn(&BoardDesignRuleCheckSettings) -> UnsignedLength,
    fn(&mut BoardDesignRuleCheckSettings, UnsignedLength) -> bool,
);

const DRC_CLEARANCES: [DrcLength; 6] = [
    (
        "drc_copper_copper",
        "Copper ↔ Copper:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_copper_copper_clearance,
        BoardDesignRuleCheckSettings::set_min_copper_copper_clearance,
    ),
    (
        "drc_copper_board",
        "Copper ↔ Board Edge:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_copper_board_clearance,
        BoardDesignRuleCheckSettings::set_min_copper_board_clearance,
    ),
    (
        "drc_copper_npth",
        "Copper ↔ Holes:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_copper_npth_clearance,
        BoardDesignRuleCheckSettings::set_min_copper_npth_clearance,
    ),
    (
        "drc_drill_drill",
        "Drill ↔ Drill:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_drill_drill_clearance,
        BoardDesignRuleCheckSettings::set_min_drill_drill_clearance,
    ),
    (
        "drc_drill_board",
        "Drill ↔ Board Edge:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_drill_board_clearance,
        BoardDesignRuleCheckSettings::set_min_drill_board_clearance,
    ),
    (
        "drc_silkscreen_stopmask",
        "Silkscreen ↔ Stopmask:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_silkscreen_stopmask_clearance,
        BoardDesignRuleCheckSettings::set_min_silkscreen_stopmask_clearance,
    ),
];

const DRC_MINIMUMS: [DrcLength; 9] = [
    (
        "drc_copper_width",
        "Copper Width:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_copper_width,
        BoardDesignRuleCheckSettings::set_min_copper_width,
    ),
    (
        "drc_pth_annular_ring",
        "PTH Annular Ring:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_pth_annular_ring,
        BoardDesignRuleCheckSettings::set_min_pth_annular_ring,
    ),
    (
        "drc_npth_drill",
        "NPTH Drill Diameter:",
        steps::DRILL_DIAMETER,
        BoardDesignRuleCheckSettings::min_npth_drill_diameter,
        BoardDesignRuleCheckSettings::set_min_npth_drill_diameter,
    ),
    (
        "drc_npth_slot",
        "NPTH Slot Width:",
        steps::DRILL_DIAMETER,
        BoardDesignRuleCheckSettings::min_npth_slot_width,
        BoardDesignRuleCheckSettings::set_min_npth_slot_width,
    ),
    (
        "drc_pth_drill",
        "PTH Drill Diameter:",
        steps::DRILL_DIAMETER,
        BoardDesignRuleCheckSettings::min_pth_drill_diameter,
        BoardDesignRuleCheckSettings::set_min_pth_drill_diameter,
    ),
    (
        "drc_pth_slot",
        "PTH Slot Width:",
        steps::DRILL_DIAMETER,
        BoardDesignRuleCheckSettings::min_pth_slot_width,
        BoardDesignRuleCheckSettings::set_min_pth_slot_width,
    ),
    (
        "drc_silkscreen_width",
        "Silkscreen Width:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_silkscreen_width,
        BoardDesignRuleCheckSettings::set_min_silkscreen_width,
    ),
    (
        "drc_silkscreen_text_height",
        "Silkscreen Text Height:",
        steps::GENERIC,
        BoardDesignRuleCheckSettings::min_silkscreen_text_height,
        BoardDesignRuleCheckSettings::set_min_silkscreen_text_height,
    ),
    (
        "drc_outline_tool",
        "Outline Tool Diameter:",
        steps::DRILL_DIAMETER,
        BoardDesignRuleCheckSettings::min_outline_tool_diameter,
        BoardDesignRuleCheckSettings::set_min_outline_tool_diameter,
    ),
];

/// The board setup dialog (upstream `BoardSetupDialog`).
pub struct BoardSetupDialog {
    form: Form,
    board: BoardId,
    solder_resist: Vec<Option<PcbColor>>,
    silkscreen: Vec<PcbColor>,
    drc: BoardDesignRuleCheckSettings,
}

fn bounded_fields(form: &mut Form, id: &str, label: &str, value: BoundedUnsignedRatio) {
    form.ratio(&format!("{id}_ratio"), label, *value.ratio(), 0, i32::MAX);
    form.length(
        &format!("{id}_min"),
        tr!(BSD, "Minimum"),
        *value.min_value(),
        Length::new(0),
    );
    form.length(
        &format!("{id}_max"),
        tr!(BSD, "Maximum"),
        *value.max_value(),
        Length::new(0),
    );
}

fn chosen_bounded(form: &Form, id: &str) -> Result<BoundedUnsignedRatio, String> {
    let ratio =
        UnsignedRatio::new(form.get_ratio(&format!("{id}_ratio"))).map_err(|e| e.to_string())?;
    BoundedUnsignedRatio::new(
        ratio,
        unsigned(form, &format!("{id}_min"))?,
        unsigned(form, &format!("{id}_max"))?,
    )
    .map_err(|e| e.to_string())
}

impl BoardSetupDialog {
    /// Opens the dialog for a board; `None` if it does not exist.
    pub fn new(project: &AppProject, board: BoardId) -> Option<Self> {
        let p = project.shared().lock();
        let brd = p.project().board(board)?;
        let settings = brd.settings().clone();
        let name = brd.name().to_string();
        drop(p);
        let unit = settings.grid_unit;
        let mut form = Form::new(unit);

        // Tab: General.
        form.page(tr!(BSD, "General"));
        form.text("name", tr!(BSD, "Name:"), &name);
        let layer_counts: Vec<String> = (0..=Layer::INNER_COPPER_COUNT)
            .map(|n| n.to_string())
            .collect();
        form.choice(
            "inner_layers",
            starred(&tr!(BSD, "Inner Copper Layers:")),
            &layer_counts,
            Some(settings.inner_layer_count as usize),
        );
        form.length(
            "pcb_thickness",
            starred(&tr!(BSD, "Total PCB Thickness:")),
            *settings.pcb_thickness,
            Length::new(1),
        );
        form.set_hint("pcb_thickness", format!("{} 1.6 mm", tr!(BSD, "Default:")));
        let default_suffix = format!(" ({})", tr!(BSD, "default"));
        let mut solder_resist = vec![None];
        let mut solder_resist_names = vec![tr!(BSD, "None (fully exposed copper)")];
        let mut silkscreen = Vec::new();
        let mut silkscreen_names = Vec::new();
        for color in PcbColor::all() {
            if color.is_available_for_solder_resist() {
                let mut text = color.name_tr();
                if color == PcbColor::Green {
                    text += &default_suffix;
                }
                solder_resist.push(Some(color));
                solder_resist_names.push(text);
            }
            if color.is_available_for_silkscreen() {
                let mut text = color.name_tr();
                if color == PcbColor::White {
                    text += &default_suffix;
                }
                silkscreen.push(color);
                silkscreen_names.push(text);
            }
        }
        form.choice(
            "solder_resist",
            starred(&tr!(BSD, "Solder Resist:")),
            &solder_resist_names,
            solder_resist
                .iter()
                .position(|c| *c == settings.solder_resist),
        );
        form.choice(
            "silkscreen_color",
            starred(&tr!(BSD, "Silkscreen Color:")),
            &silkscreen_names,
            silkscreen
                .iter()
                .position(|c| *c == settings.silkscreen_color),
        );
        for (i, (id, layer)) in SILKSCREEN_LAYERS.iter().enumerate() {
            let enabled = if i < 3 {
                settings.silkscreen_layers_top.contains(layer)
            } else {
                settings.silkscreen_layers_bot.contains(layer)
            };
            let label = if i == 0 {
                starred(&tr!(BSD, "Silkscreen Layers:"))
            } else {
                String::new()
            };
            form.checkbox(id, label, layer.name_tr(), enabled);
        }
        form.note(
            "note_handover",
            format!(
                "*) {}",
                tr!(
                    BSD,
                    "These settings might not be supported and/or automatically taken into account by the PCB manufacturer. Always check/specify these manufacturing properties manually when ordering the PCB."
                )
            ),
        );

        // Tab: Design Rules.
        let r = &settings.design_rules;
        form.page(tr!(BSD, "Design Rules"));
        form.length(
            "default_trace_width",
            tr!(BSD, "Default Trace Width:"),
            *r.default_trace_width(),
            Length::new(1),
        );
        form.length_with_steps(
            "default_via_drill",
            tr!(BSD, "Default Via Drill Diameter:"),
            *r.default_via_drill_diameter(),
            Length::new(1),
            steps::DRILL_DIAMETER,
        );
        form.header(tr!(BSD, "Stop Mask Clearance:"));
        bounded_fields(
            &mut form,
            "stop_mask",
            &tr!(BSD, "Ratio (% of Diameter)"),
            r.stop_mask_clearance(),
        );
        form.length(
            "tented_vias",
            tr!(BSD, "Tented Vias Diameter:"),
            *r.stop_mask_max_via_drill_diameter(),
            Length::new(0),
        );
        form.set_hint(
            "tented_vias",
            tr!(
                BSD,
                "Vias with a drill diameter up to this diameter will be covered with solder resist (if not manually overridden). For larger vias, a stop mask opening is added."
            ),
        );
        form.header(tr!(BSD, "Solder Paste Clearance:"));
        bounded_fields(
            &mut form,
            "solder_paste",
            &tr!(BSD, "Ratio (% of Diameter)"),
            r.solder_paste_clearance(),
        );
        let pad_options = [tr!(BSD, "Full Shape"), tr!(BSD, "Automatic Annular Ring")];
        form.header(tr!(BSD, "Autom. Pads Annular Ring:"));
        form.radio(
            "cmp_side_pads",
            tr!(BSD, "Component Side Pads:"),
            &pad_options,
            usize::from(r.pad_cmp_side_auto_annular_ring()),
        );
        form.radio(
            "inner_pads",
            tr!(BSD, "Inner Layer Pads:"),
            &pad_options,
            usize::from(r.pad_inner_auto_annular_ring()),
        );
        bounded_fields(
            &mut form,
            "pad_annular",
            &tr!(BSD, "Ratio (% of Diameter)"),
            r.pad_annular_ring(),
        );
        form.header(tr!(BSD, "Vias Annular Ring:"));
        bounded_fields(
            &mut form,
            "via_annular",
            &tr!(BSD, "Ratio (% of Diameter)"),
            r.via_annular_ring(),
        );
        form.note(
            "note_rules",
            tr!(
                BSD,
                "Note: These settings define the shape of board objects which are automatically generated (e.g. stop masks, where not manually overridden). They are not related to the design rule check (DRC) at all. In contrast to these settings, DRC parameters do not have any impact on the board."
            ),
        );

        // Tab: DRC Settings.
        form.page(tr!(BSD, "DRC Settings"));
        form.label("drc_sources", tr!(BSD, "Configuration Base:"), "");
        form.button("drc_defaults", "", tr!(BSD, "Reset to Default Settings"));
        form.button(
            "drc_clear_sources",
            "",
            tr!(BSD, "Remove Link to Imported Settings"),
        );
        form.header(tr!(BSD, "Clearances"));
        for (id, label, steps, _, _) in DRC_CLEARANCES {
            form.length_with_steps(id, tr!(BSD, label), Length::new(0), Length::new(0), steps);
        }
        form.header(tr!(BSD, "Minimum Sizes"));
        for (id, label, steps, _, _) in DRC_MINIMUMS {
            form.length_with_steps(id, tr!(BSD, label), Length::new(0), Length::new(0), steps);
        }
        form.header(tr!(BSD, "Allowed Features"));
        form.checkbox(
            "drc_blind_vias",
            tr!(BSD, "Via Types:"),
            tr!(BSD, "Blind Vias"),
            false,
        );
        form.checkbox("drc_buried_vias", "", tr!(BSD, "Buried Vias"), false);
        let slot_names = [
            tr!(BSD, "None"),
            tr!(BSD, "Only Simple Oblongs"),
            tr!(BSD, "Any Without Curves"),
            tr!(BSD, "Any"),
        ];
        form.choice("drc_npth_slots", tr!(BSD, "NPTH Slots:"), &slot_names, None);
        form.choice("drc_pth_slots", tr!(BSD, "PTH Slots:"), &slot_names, None);

        let mut dialog = Self {
            form,
            board,
            solder_resist,
            silkscreen,
            drc: settings.drc_settings.clone(),
        };
        dialog.load_drc_settings(&settings.drc_settings);
        Some(dialog)
    }

    /// Upstream `loadDrcSettings()` and `loadDrcSources()`.
    fn load_drc_settings(&mut self, s: &BoardDesignRuleCheckSettings) {
        for (id, _, _, get, _) in DRC_CLEARANCES.iter().chain(DRC_MINIMUMS.iter()) {
            self.form.set_length(id, *get(s));
        }
        self.form
            .set_checked("drc_blind_vias", s.blind_vias_allowed());
        self.form
            .set_checked("drc_buried_vias", s.buried_vias_allowed());
        self.form.set_index(
            "drc_npth_slots",
            SLOTS.iter().position(|x| *x == s.allowed_npth_slots()),
        );
        self.form.set_index(
            "drc_pth_slots",
            SLOTS.iter().position(|x| *x == s.allowed_pth_slots()),
        );
        self.drc.set_sources(s.sources().to_vec());
        self.update_sources();
    }

    fn update_sources(&self) {
        let names: Vec<String> = self
            .drc
            .sources()
            .iter()
            .map(|s| {
                format!(
                    "{} {}",
                    s.organization_name.as_str(),
                    s.pcb_design_rules_name.as_str()
                )
            })
            .collect();
        self.form.set_text(
            "drc_sources",
            if names.is_empty() {
                tr!("librepcb::editor::BoardPadPropertiesDialog", "Custom")
            } else {
                names.join(", ")
            },
        );
        self.form
            .set_enabled("drc_clear_sources", !self.drc.sources().is_empty());
    }

    fn design_rules(&self, mut r: BoardDesignRules) -> Result<BoardDesignRules, String> {
        let f = &self.form;
        r.set_default_trace_width(positive(f, "default_trace_width")?);
        r.set_default_via_drill_diameter(positive(f, "default_via_drill")?);
        r.set_stop_mask_clearance(chosen_bounded(f, "stop_mask")?);
        r.set_solder_paste_clearance(chosen_bounded(f, "solder_paste")?);
        r.set_pad_cmp_side_auto_annular_ring(f.get_index("cmp_side_pads") == Some(1));
        r.set_pad_inner_auto_annular_ring(f.get_index("inner_pads") == Some(1));
        r.set_pad_annular_ring(chosen_bounded(f, "pad_annular")?);
        r.set_via_annular_ring(chosen_bounded(f, "via_annular")?);
        r.set_stop_mask_max_via_drill_diameter(unsigned(f, "tented_vias")?);
        Ok(r)
    }

    fn drc_settings(&self) -> Result<BoardDesignRuleCheckSettings, String> {
        let f = &self.form;
        let mut s = self.drc.clone();
        for (id, _, _, _, set) in DRC_CLEARANCES.iter().chain(DRC_MINIMUMS.iter()) {
            set(&mut s, unsigned(f, id)?);
        }
        s.set_blind_vias_allowed(f.get_checked("drc_blind_vias"));
        s.set_buried_vias_allowed(f.get_checked("drc_buried_vias"));
        if let Some(slots) = f.get_index("drc_npth_slots").and_then(|i| SLOTS.get(i)) {
            s.set_allowed_npth_slots(*slots);
        }
        if let Some(slots) = f.get_index("drc_pth_slots").and_then(|i| SLOTS.get(i)) {
            s.set_allowed_pth_slots(*slots);
        }
        Ok(s)
    }
}

impl FormDialog for BoardSetupDialog {
    fn title(&self) -> String {
        tr!(BSD, "Board Setup")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            width: 620.0,
            label_width: 190.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        match (id, event) {
            ("drc_defaults", FieldEvent::Clicked) => {
                self.drc.set_sources(Vec::new());
                self.load_drc_settings(&BoardDesignRuleCheckSettings::default());
            }
            ("drc_clear_sources", FieldEvent::Clicked) => {
                self.drc.set_sources(Vec::new());
                self.update_sources();
            }
            _ => {}
        }
    }

    /// Upstream `apply()`: one `CmdBoardEdit`.
    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let name = ElementName::new(f.get_text("name").trim()).map_err(|e| e.to_string())?;
        let mut settings = {
            let p = ctx.project()?.shared().lock();
            let board = p
                .project()
                .board(self.board)
                .ok_or_else(|| "Board not found".to_owned())?;
            board.settings().clone()
        };
        if let Some(count) = f.get_index("inner_layers") {
            settings.inner_layer_count = count as u32;
        }
        settings.pcb_thickness = positive(f, "pcb_thickness")?;
        if let Some(color) = f
            .get_index("solder_resist")
            .and_then(|i| self.solder_resist.get(i))
        {
            settings.solder_resist = *color;
        }
        if let Some(color) = f
            .get_index("silkscreen_color")
            .and_then(|i| self.silkscreen.get(i))
        {
            settings.silkscreen_color = *color;
        }
        let layers = |range: std::ops::Range<usize>| -> Vec<Layer> {
            SILKSCREEN_LAYERS[range]
                .iter()
                .filter(|(id, _)| f.get_checked(id))
                .map(|(_, l)| *l)
                .collect()
        };
        settings.silkscreen_layers_top = layers(0..3);
        settings.silkscreen_layers_bot = layers(3..6);
        settings.design_rules = self.design_rules(settings.design_rules.clone())?;
        settings.drc_settings = self.drc_settings()?;
        let board = self.board;
        transaction(
            ctx.project()?,
            tr!("librepcb::editor::CmdBoardEdit", "Modify Board Setup"),
            |e| {
                let renamed = e.project().board(board).is_some_and(|b| *b.name() != name);
                if renamed {
                    e.execute(RenameBoard {
                        board,
                        name: name.clone(),
                    })?;
                }
                e.execute(EditBoardSettings {
                    board: Some(board),
                    settings: Some(Box::new(settings.clone())),
                    ..EditBoardSettings::default()
                })
            },
        )
        .map_err(|e| format!("{}\n\n{e}", tr!(BSD, "Could not apply settings")))?;
        Ok(Applied::Project)
    }
}

// --- Project setup -------------------------------------------------------------

/// A net class row of the project setup dialog.
#[derive(Debug, Clone)]
struct NetClassRow {
    id: Option<NetClassId>,
    name: String,
    used: bool,
}

/// An assembly variant row of the project setup dialog.
#[derive(Debug, Clone)]
struct VariantRow {
    id: Option<AssemblyVariantId>,
    name: String,
    description: String,
}

/// The project setup dialog (upstream `ProjectSetupDialog`).
pub struct ProjectSetupDialog {
    form: Form,
    attributes: Rc<RefCell<AttributeEditor>>,
    locales: Vec<String>,
    norms: Vec<String>,
    net_classes: Vec<NetClassRow>,
    variants: Vec<VariantRow>,
}

impl ProjectSetupDialog {
    /// Opens the dialog for a project.
    pub fn new(project: &AppProject) -> Self {
        let p = project.shared().lock();
        let prj = p.project();
        let metadata = prj.metadata().clone();
        let settings = prj.settings().clone();
        let circuit = prj.circuit();
        let mut net_classes: Vec<NetClassRow> = circuit
            .net_classes()
            .iter()
            .map(|(id, nc)| NetClassRow {
                id: Some(*id),
                name: nc.name().to_string(),
                used: circuit.net_signals().values().any(|n| n.net_class() == *id),
            })
            .collect();
        net_classes.sort_by(|a, b| super::natural_cmp(&a.name, &b.name));
        let variants: Vec<VariantRow> = circuit
            .assembly_variants()
            .iter()
            .map(|v| VariantRow {
                id: Some(AssemblyVariantId(v.uuid())),
                name: v.name().to_string(),
                description: v.description().clone(),
            })
            .collect();
        drop(p);

        let mut form = Form::new(LengthUnit::Millimeters);
        // Tab: Metadata.
        form.page(tr!(PSD, "Metadata"));
        form.text("name", tr!(PSD, "Name:"), metadata.name.as_str());
        form.text("author", tr!(PSD, "Author:"), &metadata.author);
        form.text("version", tr!(PSD, "Version:"), metadata.version.as_str());
        form.update("version", |f| {
            f.placeholder = tr!(PSD, "Mandatory, must not be empty!").into();
        });
        form.label(
            "created",
            tr!(PSD, "Created:"),
            metadata.created.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        );

        // Tab: Attributes.
        form.page(tr!(PSD, "Attributes"));
        let attributes = AttributeEditor::new(&metadata.attributes);
        form.attributes("attributes", "", attributes.borrow().model_rc());

        // Tab: Locales & Norms.
        form.page(tr!(PSD, "Locales && Norms").replace("&&", "&"));
        let order_buttons = ListButtons {
            add: true,
            remove: true,
            move_: true,
            duplicate: false,
        };
        form.list(
            "locales",
            tr!(PSD, "Preferred Languages:\n(Highest priority at top)"),
            &[],
            &[],
            5,
            order_buttons,
        );
        let locale_suggestions: Vec<String> = librepcb_i18n::available_languages()
            .iter()
            .map(|l| (*l).to_owned())
            .collect();
        form.update("locales", |f| {
            f.suggestions = crate::models::vec_model(
                locale_suggestions
                    .iter()
                    .map(|s| slint::SharedString::from(s.as_str()))
                    .collect(),
            );
        });
        form.list(
            "norms",
            tr!(PSD, "Preferred Norms:\n(Highest priority at top)"),
            &[],
            &[],
            5,
            order_buttons,
        );
        form.update("norms", |f| {
            f.suggestions = crate::models::vec_model(vec!["IEC 60617".into(), "IEEE 315".into()]);
        });

        // Tab: Net Classes.
        form.page(tr!(PSD, "Net Classes"));
        form.list(
            "net_classes",
            "",
            &[],
            &[],
            8,
            ListButtons {
                add: true,
                remove: true,
                ..ListButtons::default()
            },
        );
        form.update("net_classes", |f| {
            f.placeholder = tr!(PSD, "Type name...").into();
        });
        form.text("net_class_name", tr!(PSD, "Name:"), "");
        form.set_enabled("net_class_name", false);
        form.note(
            "net_classes_note",
            tr!(
                PSD,
                "Note: Checked net classes are in use and thus cannot be removed."
            ),
        );

        // Tab: Assembly Variants.
        form.page(tr!(PSD, "Assembly Variants"));
        form.list(
            "variants",
            "",
            &[],
            &[],
            6,
            ListButtons {
                add: true,
                remove: true,
                ..ListButtons::default()
            },
        );
        form.text("variant_name", tr!(PSD, "Name:"), "");
        form.text(
            "variant_description",
            tr!("ComponentVariantListItem", "Description"),
            "",
        );
        form.set_enabled("variant_name", false);
        form.set_enabled("variant_description", false);

        let dialog = Self {
            form,
            attributes,
            locales: settings.locale_order,
            norms: settings.norm_order,
            net_classes,
            variants,
        };
        dialog.refresh_lists(None, None, None, None);
        dialog
    }

    /// The attribute editor (tests).
    pub fn attributes(&self) -> &Rc<RefCell<AttributeEditor>> {
        &self.attributes
    }

    fn refresh_lists(
        &self,
        locale: Option<usize>,
        norm: Option<usize>,
        net_class: Option<usize>,
        variant: Option<usize>,
    ) {
        let items = |v: &[String]| v.iter().map(ListItem::text).collect::<Vec<_>>();
        self.form
            .set_items("locales", &items(&self.locales), locale);
        self.form.set_items("norms", &items(&self.norms), norm);
        let classes: Vec<ListItem> = self
            .net_classes
            .iter()
            .map(|n| ListItem::check(&n.name, n.used))
            .collect();
        self.form.set_items("net_classes", &classes, net_class);
        let variants: Vec<ListItem> = self
            .variants
            .iter()
            .map(|v| {
                ListItem::text(if v.description.is_empty() {
                    v.name.clone()
                } else {
                    format!("{} ({})", v.name, v.description)
                })
            })
            .collect();
        self.form.set_items("variants", &variants, variant);
    }

    fn select_net_class(&self, index: Option<usize>) {
        let row = index.and_then(|i| self.net_classes.get(i));
        self.form
            .set_text("net_class_name", row.map_or("", |r| r.name.as_str()));
        self.form.set_enabled("net_class_name", row.is_some());
    }

    fn select_variant(&self, index: Option<usize>) {
        let row = index.and_then(|i| self.variants.get(i));
        self.form
            .set_text("variant_name", row.map_or("", |r| r.name.as_str()));
        self.form.set_text(
            "variant_description",
            row.map_or("", |r| r.description.as_str()),
        );
        self.form.set_enabled("variant_name", row.is_some());
        self.form.set_enabled("variant_description", row.is_some());
    }
}

/// Applies an up/down/add/remove action to an ordered string list;
/// returns the new selection.
fn edit_order(list: &mut Vec<String>, action: &ListAction) -> Option<usize> {
    match action {
        ListAction::Select(i) => Some(*i),
        ListAction::Add(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| {
                list.push(text.to_owned());
                list.len() - 1
            })
        }
        ListAction::Remove(i) if *i < list.len() => {
            list.remove(*i);
            None
        }
        ListAction::MoveUp(i) if *i > 0 && *i < list.len() => {
            list.swap(*i, *i - 1);
            Some(*i - 1)
        }
        ListAction::MoveDown(i) if *i + 1 < list.len() => {
            list.swap(*i, *i + 1);
            Some(*i + 1)
        }
        _ => None,
    }
}

impl FormDialog for ProjectSetupDialog {
    fn title(&self) -> String {
        tr!(PSD, "Project Setup")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            width: 600.0,
            label_width: 170.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        match (id, event) {
            ("locales", FieldEvent::List(action)) => {
                let sel = edit_order(&mut self.locales, &action);
                let norm = self.form.get_index("norms");
                let nc = self.form.get_index("net_classes");
                let v = self.form.get_index("variants");
                self.refresh_lists(sel, norm, nc, v);
            }
            ("norms", FieldEvent::List(action)) => {
                let sel = edit_order(&mut self.norms, &action);
                let loc = self.form.get_index("locales");
                let nc = self.form.get_index("net_classes");
                let v = self.form.get_index("variants");
                self.refresh_lists(loc, sel, nc, v);
            }
            ("net_classes", FieldEvent::List(action)) => {
                let sel = match action {
                    ListAction::Select(i) => Some(i),
                    ListAction::Add(name) if !name.trim().is_empty() => {
                        self.net_classes.push(NetClassRow {
                            id: None,
                            name: name.trim().to_owned(),
                            used: false,
                        });
                        Some(self.net_classes.len() - 1)
                    }
                    ListAction::Remove(i) if self.net_classes.get(i).is_some_and(|n| !n.used) => {
                        self.net_classes.remove(i);
                        None
                    }
                    _ => self.form.get_index("net_classes"),
                };
                let loc = self.form.get_index("locales");
                let norm = self.form.get_index("norms");
                let v = self.form.get_index("variants");
                self.refresh_lists(loc, norm, sel, v);
                self.select_net_class(sel);
            }
            ("net_class_name", FieldEvent::Edited) => {
                let Some(i) = self.form.get_index("net_classes") else {
                    return;
                };
                if let Some(row) = self.net_classes.get_mut(i) {
                    row.name = self.form.get_text("net_class_name");
                }
                let loc = self.form.get_index("locales");
                let norm = self.form.get_index("norms");
                let v = self.form.get_index("variants");
                self.refresh_lists(loc, norm, Some(i), v);
            }
            ("variants", FieldEvent::List(action)) => {
                let sel = match action {
                    ListAction::Select(i) => Some(i),
                    ListAction::Add(name) if !name.trim().is_empty() => {
                        self.variants.push(VariantRow {
                            id: None,
                            name: FileProofName::clean(name.trim()),
                            description: String::new(),
                        });
                        Some(self.variants.len() - 1)
                    }
                    // Upstream: the last assembly variant cannot be removed.
                    ListAction::Remove(i) if i < self.variants.len() && self.variants.len() > 1 => {
                        self.variants.remove(i);
                        None
                    }
                    _ => self.form.get_index("variants"),
                };
                let loc = self.form.get_index("locales");
                let norm = self.form.get_index("norms");
                let nc = self.form.get_index("net_classes");
                self.refresh_lists(loc, norm, nc, sel);
                self.select_variant(sel);
            }
            ("variant_name" | "variant_description", FieldEvent::Edited) => {
                let Some(i) = self.form.get_index("variants") else {
                    return;
                };
                if let Some(row) = self.variants.get_mut(i) {
                    row.name = self.form.get_text("variant_name");
                    row.description = self.form.get_text("variant_description");
                }
                let loc = self.form.get_index("locales");
                let norm = self.form.get_index("norms");
                let nc = self.form.get_index("net_classes");
                self.refresh_lists(loc, norm, nc, Some(i));
            }
            _ => {}
        }
    }

    /// Upstream `apply()`: one undo group "Modify Project Setup".
    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let error = |e: String| format!("{}\n\n{e}", tr!(PSD, "Could not apply settings"));
        let name = ElementName::new(ElementName::clean(&f.get_text("name")))
            .map_err(|e| error(e.to_string()))?;
        let version = FileProofName::new(FileProofName::clean(&f.get_text("version")))
            .map_err(|e| error(e.to_string()))?;
        let attributes = self.attributes.borrow().list().map_err(error)?;
        let mut classes = Vec::new();
        for row in &self.net_classes {
            let name = ElementName::new(row.name.trim()).map_err(|e| error(e.to_string()))?;
            classes.push((row.id, name));
        }
        let mut variants = Vec::new();
        for row in &self.variants {
            let name = FileProofName::new(FileProofName::clean(&row.name))
                .map_err(|e| error(e.to_string()))?;
            variants.push((row.id, name, row.description.trim().to_owned()));
        }
        let author = f.get_text("author").trim().to_owned();
        let locales = self.locales.clone();
        let norms = self.norms.clone();
        transaction(ctx.project()?, tr!(PSD, "Modify Project Setup"), |e| {
            e.execute(EditProjectMetadata {
                name: Some(name),
                author: Some(author),
                version: Some(version),
                attributes: Some(attributes),
            })?;
            let settings = e.project().settings();
            if settings.locale_order != locales || settings.norm_order != norms {
                e.execute(EditProjectSettings {
                    locale_order: Some(locales),
                    norm_order: Some(norms),
                    ..EditProjectSettings::default()
                })?;
            }
            // Net classes: remove, add, rename.
            let kept: Vec<NetClassId> = classes.iter().filter_map(|(id, _)| *id).collect();
            let removed: Vec<NetClassId> = e
                .project()
                .circuit()
                .net_classes()
                .keys()
                .filter(|id| !kept.contains(id))
                .copied()
                .collect();
            for net_class in removed {
                e.execute(RemoveNetClass { net_class })?;
            }
            for (id, name) in &classes {
                match id {
                    None => {
                        e.execute(AddNetClass {
                            name: name.clone(),
                            default_trace_width: None,
                            default_via_drill: None,
                        })?;
                    }
                    Some(id) => {
                        let old = e
                            .project()
                            .circuit()
                            .net_class(*id)
                            .map(|n| n.name().clone());
                        if old.as_ref() != Some(name) {
                            e.execute(EditNetClass {
                                net_class: *id,
                                name: Some(name.clone()),
                                default_trace_width: None,
                                default_via_drill: None,
                                inherit_defaults: false,
                            })?;
                        }
                    }
                }
            }
            // Assembly variants: add, update, remove.
            let mut mutations = Vec::new();
            for (index, (id, name, description)) in variants.iter().enumerate() {
                match id {
                    None => mutations.push(Mutation::AddAssemblyVariant {
                        variant: AssemblyVariant::new(
                            Uuid::new_random(),
                            name.clone(),
                            description.clone(),
                        ),
                        index: Some(index),
                    }),
                    Some(id) => {
                        let old = e
                            .project()
                            .circuit()
                            .assembly_variants()
                            .iter()
                            .find(|v| v.uuid() == id.0)
                            .cloned();
                        if let Some(mut v) = old
                            && (v.name() != name || v.description() != description)
                        {
                            v.set_name(name.clone());
                            v.set_description(description.clone());
                            mutations.push(Mutation::UpdateAssemblyVariant(v));
                        }
                    }
                }
            }
            let kept: Vec<AssemblyVariantId> = variants.iter().filter_map(|v| v.0).collect();
            for v in e.project().circuit().assembly_variants().iter() {
                let id = AssemblyVariantId(v.uuid());
                if !kept.contains(&id) {
                    mutations.push(Mutation::RemoveAssemblyVariant(id));
                }
            }
            if !mutations.is_empty() {
                e.apply_mutations(tr!(PSD, "Modify Project Setup"), mutations)?;
            }
            Ok(())
        })
        .map_err(error)?;
        Ok(Applied::Project)
    }
}
