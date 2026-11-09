//! Properties dialogs of the board editor.
//!
//! Ports of libs/librepcb/editor/project/board/{deviceinstancepropertiesdialog,
//! boardviapropertiesdialog, boardpadpropertiesdialog,
//! boardplanepropertiesdialog}.{ui,cpp}, the board uses of
//! libs/librepcb/editor/dialogs/{polygon,stroketext,hole,zone}propertiesdialog.{ui,cpp}
//! and the "Set Width" input dialog of `BoardEditorState_Select`.
//!
//! Differences to upstream: the device dialog offers the compatible
//! devices of the project library and the footprints of the package as
//! combo boxes (upstream changes them from the context menu only); the pad
//! dialog edits the pad geometry and clearances but not its holes (count
//! shown) nor the custom shape outline; path vertices are edited in place
//! (no adding/removing).

use std::collections::{BTreeSet, HashMap};

use librepcb_core::geometry::ComponentSide;
use librepcb_core::geometry::{PadFunction, PadShape, Path, ZoneRules};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::board::BoardItem;
use librepcb_core::project::board::{BoardSegmentElements, PlaneConnectStyle};
use librepcb_core::project::{
    BoardId, BoardMutation, BoardNetSegmentRef, ComponentInstanceId, Mutation, NetSegmentId,
    NetSignalId, PlaneId,
};
use librepcb_core::types::{
    Layer, Length, LengthUnit, MaskConfig, PositiveLength, Ratio, StrokeTextSpacing,
    UnsignedLength, UnsignedLimitedRatio, UnsignedRatio, Uuid,
};
use librepcb_editor::commands::{
    EditComponent, EditPlane, MoveDevice, PlaneSettings, ReplaceDevice, UpdateBoardItem,
};
use librepcb_editor::fsm::board::BoardItemRef;
use librepcb_i18n::tr;
use std::cell::RefCell;
use std::rc::Rc;

use super::attributes::AttributeEditor;
use super::geometry::{
    alignment_fields, chosen_alignment, chosen_layer, chosen_path, chosen_position, layer_field,
    path_fields, position_fields,
};
use super::{
    Applied, DialogContext, DialogOptions, FieldEvent, Form, FormDialog, ListButtons, ListItem,
    TabDialogResult, sort_numeric, transaction,
};
use crate::length_edit::steps;
use crate::project::AppProject;
use crate::tabs::DxfImportKind;

/// Layers allowed for board polygons and stroke texts (upstream
/// `BoardEditorState::getAllowedGeometryLayers()`).
pub fn board_geometry_layers(copper: &BTreeSet<Layer>) -> Vec<Layer> {
    let mut layers: BTreeSet<Layer> = [
        Layer::BOARD_SHEET_FRAMES,
        Layer::BOARD_OUTLINES,
        Layer::BOARD_CUTOUTS,
        Layer::BOARD_PLATED_CUTOUTS,
        Layer::BOARD_MEASURES,
        Layer::BOARD_ALIGNMENT,
        Layer::BOARD_DOCUMENTATION,
        Layer::BOARD_COMMENTS,
        Layer::BOARD_GUIDE,
        Layer::TOP_NAMES,
        Layer::TOP_VALUES,
        Layer::TOP_LEGEND,
        Layer::TOP_DOCUMENTATION,
        Layer::TOP_COPPER,
        Layer::TOP_GLUE,
        Layer::TOP_SOLDER_PASTE,
        Layer::TOP_STOP_MASK,
        Layer::BOT_NAMES,
        Layer::BOT_VALUES,
        Layer::BOT_LEGEND,
        Layer::BOT_DOCUMENTATION,
        Layer::BOT_COPPER,
        Layer::BOT_GLUE,
        Layer::BOT_SOLDER_PASTE,
        Layer::BOT_STOP_MASK,
    ]
    .into_iter()
    .collect();
    layers.extend(copper.iter().copied());
    layers.into_iter().collect()
}

/// Opens the properties dialog of a board item (upstream
/// `BoardEditorState_Select::openPropertiesDialog()`).
pub fn open(
    project: &AppProject,
    board: BoardId,
    item: BoardItemRef,
    unit: LengthUnit,
) -> Option<Box<dyn FormDialog>> {
    let item = {
        let p = project.shared().lock();
        item.resolved(p.project().board(board)?)?
    };
    Some(match item {
        BoardItemRef::Device(c) | BoardItemRef::FootprintPad(c, _) => {
            Box::new(DeviceDialog::new(project, board, c, unit)?)
        }
        BoardItemRef::Via(seg, uuid) => Box::new(ViaDialog::new(project, board, seg, uuid, unit)?),
        BoardItemRef::Pad(seg, uuid) => Box::new(PadDialog::new(project, board, seg, uuid, unit)?),
        BoardItemRef::Plane(plane) => Box::new(PlaneDialog::new(project, board, plane, unit)?),
        BoardItemRef::Polygon(uuid) => Box::new(PolygonDialog::new(project, board, uuid, unit)?),
        BoardItemRef::StrokeText(uuid) => {
            Box::new(StrokeTextDialog::new(project, board, None, uuid, unit)?)
        }
        BoardItemRef::DeviceStrokeText(c, uuid) => {
            Box::new(StrokeTextDialog::new(project, board, Some(c), uuid, unit)?)
        }
        BoardItemRef::Hole(uuid) => Box::new(HoleDialog::new(project, board, uuid, unit)?),
        BoardItemRef::Zone(uuid) => Box::new(ZoneDialog::new(project, board, uuid, unit)?),
        BoardItemRef::Junction(..) | BoardItemRef::Trace(..) => return None,
    })
}

/// Adds the fields of a mask configuration (upstream radio buttons "Off",
/// "From Design Rules", "Manual:" with an offset edit).
pub(crate) fn mask_config_fields(
    form: &mut Form,
    id: &str,
    label: &str,
    context: &str,
    config: MaskConfig,
) {
    let options = vec![
        tr!(context, "Off"),
        tr!(context, "From Design Rules"),
        tr!(context, "Manual:"),
    ];
    let (index, offset) = match config {
        MaskConfig::Off => (0, Length::new(100_000)),
        MaskConfig::Automatic => (1, Length::new(100_000)),
        MaskConfig::Manual(offset) => (2, offset),
    };
    form.choice(id, label, &options, Some(index));
    let offset_id = format!("{id}_offset");
    form.length(&offset_id, "", offset, Length::MIN);
    form.set_enabled(&offset_id, index == 2);
}

pub(crate) fn update_mask_config_fields(form: &Form, id: &str) {
    form.set_enabled(&format!("{id}_offset"), form.get_index(id) == Some(2));
}

pub(crate) fn chosen_mask_config(form: &Form, id: &str) -> MaskConfig {
    match form.get_index(id) {
        Some(2) => MaskConfig::Manual(form.get_length(&format!("{id}_offset"))),
        Some(1) => MaskConfig::Automatic,
        _ => MaskConfig::Off,
    }
}

pub(crate) fn positive(form: &Form, id: &str) -> Result<PositiveLength, String> {
    PositiveLength::new(form.get_length(id)).map_err(|e| e.to_string())
}

pub(crate) fn unsigned(form: &Form, id: &str) -> Result<UnsignedLength, String> {
    UnsignedLength::new(form.get_length(id)).map_err(|e| e.to_string())
}

fn net_name(project: &librepcb_core::project::Project, net: Option<NetSignalId>) -> String {
    net.and_then(|n| project.circuit().net_signal(n))
        .map_or_else(String::new, |n| n.name().to_string())
}

// --- Device -------------------------------------------------------------------

/// The properties dialog of a device (upstream
/// `DeviceInstancePropertiesDialog`).
pub struct DeviceDialog {
    form: Form,
    board: BoardId,
    component: ComponentInstanceId,
    name: String,
    attributes: Rc<RefCell<AttributeEditor>>,
    devices: Vec<Uuid>,
    footprints: Vec<Uuid>,
    current_device: Uuid,
    current_footprint: Uuid,
    locked: bool,
}

impl DeviceDialog {
    /// Opens the dialog; `None` if the device does not exist.
    pub fn new(
        project: &AppProject,
        board: BoardId,
        component: ComponentInstanceId,
        unit: LengthUnit,
    ) -> Option<Self> {
        let p = project.shared().lock();
        let prj = p.project();
        let dev = prj.board(board)?.device(component)?;
        let cmp = prj.circuit().component_instance(component)?;
        let locales = &prj.settings().locale_order;
        let lib = prj.library();
        let name_of = |uuid: &Uuid| {
            lib.device(uuid)
                .map(|d| d.metadata().names().value(locales).to_string())
                .unwrap_or_else(|| uuid.to_string())
        };
        let mut form = Form::new(unit);
        form.header(tr!(
            "librepcb::editor::DeviceInstancePropertiesDialog",
            "Component"
        ));
        form.text(
            "name",
            tr!("librepcb::editor::DeviceInstancePropertiesDialog", "Name:"),
            cmp.name().as_str(),
        );
        form.multiline(
            "value",
            tr!("librepcb::editor::DeviceInstancePropertiesDialog", "Value:"),
            cmp.value(),
            2,
        );

        form.header(tr!(
            "librepcb::editor::DeviceInstancePropertiesDialog",
            "Library Elements"
        ));
        let mut devices: Vec<Uuid> = lib
            .devices_of_component(&cmp.lib_component())
            .iter()
            .map(|d| d.metadata().uuid())
            .collect();
        if !devices.contains(&dev.lib_device()) {
            devices.push(dev.lib_device());
        }
        let device_names: Vec<String> = devices.iter().map(name_of).collect();
        let device_index = devices.iter().position(|d| *d == dev.lib_device());
        form.choice(
            "device",
            tr!(
                "librepcb::editor::DeviceInstancePropertiesDialog",
                "Device:"
            ),
            &device_names,
            device_index,
        );
        let package = lib
            .device(&dev.lib_device())
            .and_then(|d| lib.package(&d.package_uuid()));
        let (footprints, footprint_names): (Vec<Uuid>, Vec<String>) = package
            .map(|pkg| {
                pkg.footprints()
                    .iter()
                    .map(|f| (f.uuid(), f.names().value(locales).to_string()))
                    .unzip()
            })
            .unwrap_or_default();
        form.label(
            "package",
            tr!(
                "librepcb::editor::DeviceInstancePropertiesDialog",
                "Package:"
            ),
            package
                .map(|pkg| pkg.metadata().names().value(locales).to_string())
                .unwrap_or_default(),
        );
        let footprint_index = footprints.iter().position(|f| *f == dev.lib_footprint());
        form.choice(
            "footprint",
            tr!(
                "librepcb::editor::DeviceInstancePropertiesDialog",
                "Footprint"
            ),
            &footprint_names,
            footprint_index,
        );

        form.header(tr!(
            "librepcb::editor::DeviceInstancePropertiesDialog",
            "Placement"
        ));
        position_fields(
            &mut form,
            &tr!(
                "librepcb::editor::DeviceInstancePropertiesDialog",
                "Pos. X:"
            ),
            &tr!(
                "librepcb::editor::DeviceInstancePropertiesDialog",
                "Pos. Y:"
            ),
            dev.position(),
        );
        form.angle(
            "rotation",
            tr!(
                "librepcb::editor::DeviceInstancePropertiesDialog",
                "Rotation:"
            ),
            dev.rotation(),
        );
        form.checkbox(
            "mirror",
            tr!(
                "librepcb::editor::DeviceInstancePropertiesDialog",
                "Options:"
            ),
            tr!("librepcb::editor::StrokeTextPropertiesDialog", "Mirror"),
            dev.mirrored(),
        );
        form.checkbox(
            "lock",
            "",
            tr!("librepcb::editor::StrokeTextPropertiesDialog", "Lock"),
            dev.locked(),
        );

        let attributes = AttributeEditor::new(cmp.attributes());
        form.header(tr!(
            "librepcb::editor::DeviceInstancePropertiesDialog",
            "Attributes of Component"
        ));
        form.attributes("attributes", "", attributes.borrow().model_rc());

        let name = cmp.name().to_string();
        let current_device = dev.lib_device();
        let current_footprint = dev.lib_footprint();
        let locked = dev.locked();
        drop(p);
        Some(Self {
            form,
            board,
            component,
            name,
            attributes,
            devices,
            footprints,
            current_device,
            current_footprint,
            locked,
        })
    }
}

impl FormDialog for DeviceDialog {
    fn title(&self) -> String {
        tr!(
            "librepcb::editor::DeviceInstancePropertiesDialog",
            "Properties of {0}",
            self.name.as_str()
        )
    }

    form_accessors!();

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let name = librepcb_core::types::CircuitIdentifier::new(form.get_text("name").trim())
            .map_err(|e| e.to_string())?;
        let attributes = self.attributes.borrow().list()?;
        let value = form.get_text("value");
        let device = form
            .get_index("device")
            .and_then(|i| self.devices.get(i).copied());
        let footprint = form
            .get_index("footprint")
            .and_then(|i| self.footprints.get(i).copied());
        let position = chosen_position(form);
        let rotation = form.get_angle("rotation");
        let mirrored = form.get_checked("mirror");
        let locked = form.get_checked("lock");
        let (component, board) = (self.component, self.board);
        let replace = match (device, footprint) {
            (Some(d), _) if d != self.current_device => Some((d, None)),
            (_, Some(f)) if f != self.current_footprint => Some((self.current_device, Some(f))),
            _ => None,
        };
        let lock_changed = locked != self.locked;
        let text = tr!(
            "librepcb::editor::DeviceInstancePropertiesDialog",
            "Change properties of {0}",
            self.name.as_str()
        );
        transaction(ctx.project()?, text, |e| {
            e.execute(EditComponent {
                component: component.into(),
                name: Some(name),
                value: Some(value),
                attributes: Some(attributes),
                assembly_options: None,
                lock_assembly: None,
            })?;
            if let Some((device, footprint)) = replace {
                e.execute(ReplaceDevice {
                    component: component.into(),
                    board: Some(board),
                    device,
                    footprint,
                })?;
            }
            e.execute(MoveDevice {
                component: component.into(),
                board: Some(board),
                position: Some(position),
                rotation: Some(rotation),
                mirrored: Some(mirrored),
                // Do not apply to all elements if not modified (upstream).
                locked: lock_changed.then_some(locked),
            })
        })?;
        if let Some((d, f)) = replace {
            self.current_device = d;
            if let Some(f) = f {
                self.current_footprint = f;
            }
        }
        self.locked = locked;
        Ok(Applied::Project)
    }
}

// --- Via ---------------------------------------------------------------------------

/// The properties dialog of a via (upstream `BoardViaPropertiesDialog`).
pub struct ViaDialog {
    form: Form,
    segment: BoardNetSegmentRef,
    via: librepcb_core::geometry::Via,
    auto_drill: PositiveLength,
    annular_ring: librepcb_core::types::BoundedUnsignedRatio,
}

impl ViaDialog {
    /// Opens the dialog; `None` if the via does not exist.
    pub fn new(
        project: &AppProject,
        board: BoardId,
        segment: NetSegmentId,
        uuid: Uuid,
        unit: LengthUnit,
    ) -> Option<Self> {
        let p = project.shared().lock();
        let prj = p.project();
        let brd = prj.board(board)?;
        let seg = brd.net_segment(segment)?;
        let via = seg.vias().get(&uuid)?.clone();
        let mut auto = via.clone();
        auto.set_drill_and_size(None, None).ok()?;
        let auto_drill = brd
            .via_properties(&auto, seg.net(), prj.circuit())
            .drill_diameter;
        let annular_ring = brd.design_rules().via_annular_ring();
        let mut layers = brd.copper_layers();
        layers.insert(via.start_layer());
        layers.insert(via.end_layer());
        let layers: Vec<Layer> = layers.into_iter().collect();
        let mut form = Form::new(unit);
        let net = net_name(prj, seg.net());
        form.label(
            "net",
            tr!("librepcb::editor::BoardViaPropertiesDialog", "Net Signal:"),
            if net.is_empty() {
                format!(
                    "[{}]",
                    tr!("librepcb::editor::BoardPlanePropertiesDialog", "None")
                )
            } else {
                net
            },
        );
        position_fields(
            &mut form,
            &tr!("librepcb::editor::BoardViaPropertiesDialog", "Position:"),
            "",
            via.position(),
        );
        form.length_with_steps(
            "drill",
            tr!(
                "librepcb::editor::BoardViaPropertiesDialog",
                "Drill Diameter:"
            ),
            *via.drill_diameter().unwrap_or(auto_drill),
            Length::new(1),
            steps::DRILL_DIAMETER,
        );
        form.update("drill", |f| {
            f.auto_supported = true;
            f.auto_checked = via.drill_diameter().is_none();
        });
        let size = via.size().unwrap_or_else(|| {
            librepcb_core::geometry::Via::calc_size_from_rules(
                via.drill_diameter().unwrap_or(auto_drill),
                &annular_ring,
            )
        });
        form.length_auto(
            "size",
            tr!(
                "librepcb::editor::BoardViaPropertiesDialog",
                "Outer Diameter:"
            ),
            *size,
            Length::new(1),
            via.size().is_none(),
        );
        form.set_hint(
            "drill",
            tr!(
                "librepcb::editor::BoardViaPropertiesDialog",
                "From Design Rules"
            ),
        );
        form.set_hint(
            "size",
            tr!(
                "librepcb::editor::BoardViaPropertiesDialog",
                "From Design Rules"
            ),
        );
        layer_field(
            &mut form,
            "start_layer",
            &tr!("librepcb::editor::BoardViaPropertiesDialog", "Start Layer:"),
            &layers,
            via.start_layer(),
        );
        layer_field(
            &mut form,
            "end_layer",
            &tr!("librepcb::editor::BoardViaPropertiesDialog", "End Layer:"),
            &layers,
            via.end_layer(),
        );
        mask_config_fields(
            &mut form,
            "exposure",
            &tr!("librepcb::editor::BoardViaPropertiesDialog", "Exposure:"),
            "librepcb::editor::BoardViaPropertiesDialog",
            via.exposure_config(),
        );
        drop(p);
        let dialog = Self {
            form,
            segment: BoardNetSegmentRef { board, segment },
            via,
            auto_drill,
            annular_ring,
        };
        dialog.update_enabled();
        Some(dialog)
    }

    fn update_enabled(&self) {
        let f = &self.form;
        f.set_enabled("drill", !f.get_auto("drill"));
        f.set_enabled("size", !f.get_auto("size"));
        update_mask_config_fields(f, "exposure");
    }

    fn apply_size_from_rules(&mut self) {
        if let Ok(drill) = PositiveLength::new(self.form.get_length("drill")) {
            let size =
                librepcb_core::geometry::Via::calc_size_from_rules(drill, &self.annular_ring);
            self.form.set_length("size", *size);
        }
    }
}

impl FormDialog for ViaDialog {
    fn title(&self) -> String {
        tr!(
            "librepcb::editor::BoardViaPropertiesDialog",
            "Via Properties"
        )
    }

    form_accessors!();

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, _event: FieldEvent) {
        match id {
            "drill" => {
                if self.form.get_auto("drill") {
                    self.form.set_auto("size", true);
                    self.form.set_length("drill", *self.auto_drill);
                }
                if self.form.get_auto("size") {
                    self.apply_size_from_rules();
                }
            }
            "size" => {
                if self.form.get_auto("size") {
                    self.apply_size_from_rules();
                } else {
                    self.form.set_auto("drill", false);
                }
            }
            _ => {}
        }
        self.update_enabled();
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        if form.get_length("drill") > form.get_length("size") {
            return Err(tr!(
                "librepcb::editor::BoardViaPropertiesDialog",
                "The drill diameter is exceeding the outer via size. Reduce the drill diameter or increase the via size."
            ));
        }
        let mut via = self.via.clone();
        via.set_position(chosen_position(form));
        let drill = (!form.get_auto("drill"))
            .then(|| positive(form, "drill"))
            .transpose()?;
        let size = (!form.get_auto("size"))
            .then(|| positive(form, "size"))
            .transpose()?;
        via.set_drill_and_size(drill, size)
            .map_err(|e| e.to_string())?;
        let start = chosen_layer(form, "start_layer").unwrap_or(via.start_layer());
        let end = chosen_layer(form, "end_layer").unwrap_or(via.end_layer());
        via.set_layers(start, end).map_err(|e| e.to_string())?;
        via.set_exposure_config(chosen_mask_config(form, "exposure"));
        let segment = self.segment;
        let text = tr!("librepcb::editor::CmdBoardViaEdit", "Edit via");
        transaction(ctx.project()?, text.clone(), |e| {
            e.apply_mutations(
                text,
                vec![Mutation::Board(BoardMutation::UpdateNetSegmentElements {
                    segment,
                    elements: BoardSegmentElements {
                        vias: vec![via.clone()],
                        ..BoardSegmentElements::default()
                    },
                })],
            )
        })?;
        self.via = via;
        Ok(Applied::Project)
    }
}

// --- Pad -------------------------------------------------------------------------------

/// The properties dialog of a standalone board pad (upstream
/// `BoardPadPropertiesDialog`, without the holes and custom outline
/// editors).
pub struct PadDialog {
    form: Form,
    segment: BoardNetSegmentRef,
    pad: librepcb_core::project::board::BoardPadData,
}

const PAD_SHAPES: [PadShape; 3] = [
    PadShape::RoundedRect,
    PadShape::RoundedOctagon,
    PadShape::Custom,
];

impl PadDialog {
    /// Opens the dialog; `None` if the pad does not exist.
    pub fn new(
        project: &AppProject,
        board: BoardId,
        segment: NetSegmentId,
        uuid: Uuid,
        unit: LengthUnit,
    ) -> Option<Self> {
        let p = project.shared().lock();
        let prj = p.project();
        let seg = prj.board(board)?.net_segment(segment)?;
        let data = seg.pads().get(&uuid)?.clone();
        let pad = data.pad();
        let mut form = Form::new(unit);
        form.page(tr!("librepcb::editor::BoardPadPropertiesDialog", "General"));
        form.label(
            "net",
            tr!("librepcb::editor::BoardPadPropertiesDialog", "Net:"),
            net_name(prj, seg.net()),
        );
        form.choice(
            "side",
            tr!(
                "librepcb::editor::BoardPadPropertiesDialog",
                "Component Side:"
            ),
            &[
                tr!("librepcb::editor::BoardPadPropertiesDialog", "Top"),
                tr!("librepcb::editor::BoardPadPropertiesDialog", "Bottom"),
            ],
            Some(usize::from(pad.component_side() == ComponentSide::Bottom)),
        );
        let functions: Vec<String> = PadFunction::ALL
            .iter()
            .map(|f| f.description_tr())
            .collect();
        form.choice(
            "function",
            tr!("librepcb::editor::BoardPadPropertiesDialog", "Function:"),
            &functions,
            PadFunction::ALL.iter().position(|f| *f == pad.function()),
        );
        form.choice(
            "shape",
            tr!("librepcb::editor::BoardPadPropertiesDialog", "Shape:"),
            &[
                tr!(
                    "librepcb::editor::BoardPadPropertiesDialog",
                    "Rounded rectangle"
                ),
                tr!("librepcb::editor::BoardPadPropertiesDialog", "Octagon"),
                tr!("librepcb::editor::BoardPadPropertiesDialog", "Custom"),
            ],
            PAD_SHAPES.iter().position(|s| *s == pad.shape()),
        );
        form.length(
            "width",
            tr!("librepcb::editor::BoardPadPropertiesDialog", "Size:"),
            *pad.width(),
            Length::new(1),
        );
        form.length("height", "", *pad.height(), Length::new(1));
        form.ratio(
            "radius",
            tr!(
                "librepcb::editor::BoardPadPropertiesDialog",
                "Corner Radius:"
            ),
            *pad.radius(),
            0,
            1_000_000,
        );
        position_fields(
            &mut form,
            &tr!("librepcb::editor::BoardPadPropertiesDialog", "Position:"),
            "",
            pad.position(),
        );
        form.angle(
            "rotation",
            tr!("librepcb::editor::BoardPadPropertiesDialog", "Rotation:"),
            pad.rotation(),
        );
        form.label(
            "holes",
            tr!("librepcb::editor::BoardPadPropertiesDialog", "Plated Holes"),
            if pad.holes().is_empty() {
                tr!(
                    "librepcb::editor::BoardPadPropertiesDialog",
                    "Pad has no holes"
                )
            } else {
                pad.holes().len().to_string()
            },
        );
        form.checkbox(
            "lock",
            tr!("librepcb::editor::BoardPadPropertiesDialog", "Options:"),
            tr!("librepcb::editor::BoardPadPropertiesDialog", "Lock"),
            data.locked(),
        );
        form.page(tr!(
            "librepcb::editor::BoardPadPropertiesDialog",
            "Clearances"
        ));
        mask_config_fields(
            &mut form,
            "stop_mask",
            &tr!("librepcb::editor::BoardPadPropertiesDialog", "Stop Mask:"),
            "librepcb::editor::BoardPadPropertiesDialog",
            pad.stop_mask_config(),
        );
        mask_config_fields(
            &mut form,
            "solder_paste",
            &tr!(
                "librepcb::editor::BoardPadPropertiesDialog",
                "Solder Paste:"
            ),
            "librepcb::editor::BoardPadPropertiesDialog",
            pad.solder_paste_config(),
        );
        form.length(
            "copper_clearance",
            tr!(
                "librepcb::editor::BoardPadPropertiesDialog",
                "Copper Keepout:"
            ),
            *pad.copper_clearance(),
            Length::new(0),
        );
        form.set_hint(
            "copper_clearance",
            tr!(
                "librepcb::editor::BoardPadPropertiesDialog",
                "Note: Intended to keep copper away from fiducials."
            ),
        );
        drop(p);
        Some(Self {
            form,
            segment: BoardNetSegmentRef { board, segment },
            pad: data,
        })
    }
}

impl FormDialog for PadDialog {
    fn title(&self) -> String {
        tr!(
            "librepcb::editor::BoardPadPropertiesDialog",
            "Pad Properties"
        )
    }

    form_accessors!();

    fn field_event(&mut self, _ctx: &DialogContext<'_>, _id: &str, _event: FieldEvent) {
        update_mask_config_fields(&self.form, "stop_mask");
        update_mask_config_fields(&self.form, "solder_paste");
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let mut data = self.pad.clone();
        data.set_locked(form.get_checked("lock"));
        {
            let pad = data.pad_mut();
            pad.set_component_side(if form.get_index("side") == Some(1) {
                ComponentSide::Bottom
            } else {
                ComponentSide::Top
            });
            if let Some(f) = form
                .get_index("function")
                .and_then(|i| PadFunction::ALL.get(i))
            {
                pad.set_function(*f);
            }
            if let Some(s) = form.get_index("shape").and_then(|i| PAD_SHAPES.get(i)) {
                pad.set_shape(*s);
            }
            pad.set_width(positive(form, "width")?);
            pad.set_height(positive(form, "height")?);
            pad.set_radius(
                UnsignedLimitedRatio::new(form.get_ratio("radius")).map_err(|e| e.to_string())?,
            );
            pad.set_position(chosen_position(form));
            pad.set_rotation(form.get_angle("rotation"));
            pad.set_stop_mask_config(chosen_mask_config(form, "stop_mask"));
            pad.set_solder_paste_config(chosen_mask_config(form, "solder_paste"));
            pad.set_copper_clearance(unsigned(form, "copper_clearance")?);
        }
        let segment = self.segment;
        let text = tr!("librepcb::editor::CmdBoardPadEdit", "Edit Pad");
        transaction(ctx.project()?, text.clone(), |e| {
            e.apply_mutations(
                text,
                vec![Mutation::Board(BoardMutation::UpdateNetSegmentElements {
                    segment,
                    elements: BoardSegmentElements {
                        pads: vec![data.clone()],
                        ..BoardSegmentElements::default()
                    },
                })],
            )
        })?;
        self.pad = data;
        Ok(Applied::Project)
    }
}

// --- Plane ---------------------------------------------------------------------------------

/// The properties dialog of a plane (upstream `BoardPlanePropertiesDialog`).
pub struct PlaneDialog {
    form: Form,
    board: BoardId,
    plane: PlaneId,
    nets: Vec<Option<NetSignalId>>,
    layers: Vec<Layer>,
    outline: Path,
}

const CONNECT_STYLES: [PlaneConnectStyle; 3] = [
    PlaneConnectStyle::None,
    PlaneConnectStyle::ThermalRelief,
    PlaneConnectStyle::Solid,
];

impl PlaneDialog {
    /// Opens the dialog; `None` if the plane does not exist.
    pub fn new(
        project: &AppProject,
        board: BoardId,
        id: PlaneId,
        unit: LengthUnit,
    ) -> Option<Self> {
        let p = project.shared().lock();
        let prj = p.project();
        let brd = prj.board(board)?;
        let plane = brd.plane(id)?;
        let mut nets: Vec<(String, NetSignalId)> = prj
            .circuit()
            .net_signals()
            .values()
            .map(|n| {
                (
                    n.name().to_string(),
                    librepcb_core::project::NetSignalId(n.uuid()),
                )
            })
            .collect();
        let mut names: Vec<String> = nets.iter().map(|(n, _)| n.clone()).collect();
        sort_numeric(&mut names);
        nets.sort_by_key(|(n, _)| names.iter().position(|x| x == n));
        let mut net_ids: Vec<Option<NetSignalId>> = vec![None];
        net_ids.extend(nets.iter().map(|(_, id)| Some(*id)));
        let mut net_names = vec![format!(
            "[{}]",
            tr!("librepcb::editor::BoardPlanePropertiesDialog", "None")
        )];
        net_names.extend(nets.iter().map(|(n, _)| n.clone()));
        let layers: Vec<Layer> = brd.copper_layers().into_iter().collect();
        let mut form = Form::new(unit);
        form.choice(
            "net",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Net Signal:"
            ),
            &net_names,
            net_ids.iter().position(|n| *n == plane.net()),
        );
        layer_field(
            &mut form,
            "layer",
            &tr!("librepcb::editor::BoardPlanePropertiesDialog", "Layer:"),
            &layers,
            plane.layer(),
        );
        form.length(
            "min_width",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Min. Width:"
            ),
            *plane.min_width(),
            Length::new(0),
        );
        form.length(
            "min_clearance_copper",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Min. Copper Clearance:"
            ),
            *plane.min_clearance_to_copper(),
            Length::new(0),
        );
        form.length(
            "min_clearance_board",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Min. Board Clearance:"
            ),
            *plane.min_clearance_to_board(),
            Length::new(0),
        );
        form.length(
            "min_clearance_npth",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Min. Hole Clearance:"
            ),
            *plane.min_clearance_to_npth(),
            Length::new(0),
        );
        form.text(
            "priority",
            tr!("librepcb::editor::BoardPlanePropertiesDialog", "Priority:"),
            plane.priority().to_string(),
        );
        form.choice(
            "connect_style",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Connect Style:"
            ),
            &[
                tr!("librepcb::editor::BoardPlanePropertiesDialog", "None"),
                tr!(
                    "librepcb::editor::BoardPlanePropertiesDialog",
                    "Thermal Relief"
                ),
                tr!("librepcb::editor::BoardPlanePropertiesDialog", "Solid"),
            ],
            CONNECT_STYLES
                .iter()
                .position(|s| *s == plane.connect_style()),
        );
        form.length(
            "thermal_gap",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Themal Gap:"
            ),
            *plane.thermal_gap(),
            Length::new(1),
        );
        form.set_hint(
            "thermal_gap",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Clearance around thermal pads"
            ),
        );
        form.length(
            "thermal_spoke_width",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Thermal Spokes:"
            ),
            *plane.thermal_spoke_width(),
            Length::new(1),
        );
        form.set_hint(
            "thermal_spoke_width",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Width of the thermal pad spokes"
            ),
        );
        form.checkbox(
            "keep_islands",
            tr!("librepcb::editor::BoardPlanePropertiesDialog", "Options:"),
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Keep Islands"
            ),
            plane.keep_islands(),
        );
        form.set_hint(
            "keep_islands",
            tr!(
                "librepcb::editor::BoardPlanePropertiesDialog",
                "Do not delete unconnected copper areas (islands)"
            ),
        );
        form.checkbox(
            "lock",
            "",
            tr!("librepcb::editor::BoardPlanePropertiesDialog", "Lock"),
            plane.locked(),
        );
        path_fields(&mut form, plane.outline());
        let outline = plane.outline().clone();
        drop(p);
        let dialog = Self {
            form,
            board,
            plane: id,
            nets: net_ids,
            layers,
            outline,
        };
        dialog.update_enabled();
        Some(dialog)
    }

    fn update_enabled(&self) {
        let thermal = self.form.get_index("connect_style") == Some(1);
        self.form.set_enabled("thermal_gap", thermal);
        self.form.set_enabled("thermal_spoke_width", thermal);
    }
}

impl FormDialog for PlaneDialog {
    fn title(&self) -> String {
        tr!(
            "librepcb::editor::BoardPlanePropertiesDialog",
            "Plane Properties"
        )
    }

    form_accessors!();

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, _event: FieldEvent) {
        if id == "priority" {
            let valid = self.form.get_text("priority").trim().parse::<i32>().is_ok();
            self.form.set_error(
                "priority",
                if valid {
                    String::new()
                } else {
                    tr!("SlintHelpers", "Invalid")
                },
            );
        }
        self.update_enabled();
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let priority: i32 = form
            .get_text("priority")
            .trim()
            .parse()
            .map_err(|_| tr!("SlintHelpers", "Invalid"))?;
        let net = form
            .get_index("net")
            .and_then(|i| self.nets.get(i).copied())
            .unwrap_or(None);
        let layer = chosen_layer(form, "layer").filter(|l| self.layers.contains(l));
        let settings = PlaneSettings {
            min_width: Some(unsigned(form, "min_width")?),
            min_clearance_to_copper: Some(unsigned(form, "min_clearance_copper")?),
            min_clearance_to_board: Some(unsigned(form, "min_clearance_board")?),
            min_clearance_to_npth: Some(unsigned(form, "min_clearance_npth")?),
            keep_islands: Some(form.get_checked("keep_islands")),
            priority: Some(priority),
            connect_style: form
                .get_index("connect_style")
                .and_then(|i| CONNECT_STYLES.get(i).copied()),
            thermal_gap: Some(positive(form, "thermal_gap")?),
            thermal_spoke_width: Some(positive(form, "thermal_spoke_width")?),
            locked: Some(form.get_checked("lock")),
        };
        let outline = chosen_path(form, &self.outline);
        let (board, plane) = (self.board, self.plane);
        transaction(
            ctx.project()?,
            tr!("librepcb::editor::CmdBoardPlaneEdit", "Edit plane"),
            |e| {
                e.execute(EditPlane {
                    board: Some(board),
                    plane,
                    net: Some(net.map(Into::into)),
                    layer,
                    outline: Some(outline),
                    settings,
                })
            },
        )?;
        Ok(Applied::Project)
    }
}

// --- Polygon -------------------------------------------------------------------------------

/// The properties dialog of a board polygon (upstream
/// `PolygonPropertiesDialog` for `BI_Polygon`).
pub struct PolygonDialog {
    form: Form,
    board: BoardId,
    polygon: librepcb_core::project::board::BoardPolygonData,
}

impl PolygonDialog {
    /// Opens the dialog; `None` if the polygon does not exist.
    pub fn new(project: &AppProject, board: BoardId, uuid: Uuid, unit: LengthUnit) -> Option<Self> {
        let p = project.shared().lock();
        let brd = p.project().board(board)?;
        let polygon = brd.polygons().get(&uuid)?.clone();
        let layers = board_geometry_layers(&brd.copper_layers());
        drop(p);
        let mut form = Form::new(unit);
        layer_field(
            &mut form,
            "layer",
            &tr!("librepcb::editor::PolygonPropertiesDialog", "Layer:"),
            &layers,
            polygon.layer(),
        );
        form.length(
            "line_width",
            tr!("librepcb::editor::PolygonPropertiesDialog", "Line Width:"),
            *polygon.line_width(),
            Length::new(0),
        );
        form.checkbox(
            "fill",
            tr!("librepcb::editor::PolygonPropertiesDialog", "Options:"),
            tr!("librepcb::editor::PolygonPropertiesDialog", "Fill"),
            polygon.is_filled(),
        );
        form.checkbox(
            "grab_area",
            "",
            tr!("librepcb::editor::PolygonPropertiesDialog", "Grab Area"),
            polygon.is_grab_area(),
        );
        form.checkbox(
            "lock",
            "",
            tr!("librepcb::editor::PolygonPropertiesDialog", "Lock"),
            polygon.locked(),
        );
        path_fields(&mut form, polygon.path());
        Some(Self {
            form,
            board,
            polygon,
        })
    }
}

impl FormDialog for PolygonDialog {
    fn title(&self) -> String {
        tr!(
            "librepcb::editor::PolygonPropertiesDialog",
            "Polygon Properties"
        )
    }

    form_accessors!();

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let mut polygon = self.polygon.clone();
        if let Some(layer) = chosen_layer(form, "layer") {
            polygon.set_layer(layer);
        }
        polygon.set_line_width(unsigned(form, "line_width")?);
        polygon.set_is_filled(form.get_checked("fill"));
        polygon.set_is_grab_area(form.get_checked("grab_area"));
        polygon.set_locked(form.get_checked("lock"));
        polygon.set_path(chosen_path(form, self.polygon.path()));
        let board = self.board;
        let item = BoardItem::Polygon(polygon.clone());
        transaction(
            ctx.project()?,
            tr!("librepcb::editor::CmdBoardPolygonEdit", "Edit polygon"),
            |e| {
                e.execute(UpdateBoardItem {
                    board: Some(board),
                    item,
                })
            },
        )?;
        self.polygon = polygon;
        Ok(Applied::Project)
    }
}

// --- Stroke text ---------------------------------------------------------------------------

/// The properties dialog of a board or device stroke text (upstream
/// `StrokeTextPropertiesDialog` for `BI_StrokeText`).
pub struct StrokeTextDialog {
    form: Form,
    board: BoardId,
    device: Option<ComponentInstanceId>,
    text: librepcb_core::project::board::BoardStrokeTextData,
}

fn spacing_fields(form: &mut Form, id: &str, label: &str, spacing: StrokeTextSpacing) {
    let (auto, ratio) = match spacing {
        StrokeTextSpacing::Auto => (true, Ratio::from_percent(100)),
        StrokeTextSpacing::Manual(r) => (false, r),
    };
    form.checkbox(
        &format!("{id}_auto"),
        label,
        tr!("librepcb::editor::StrokeTextPropertiesDialog", "Auto"),
        auto,
    );
    form.ratio(id, "", ratio, 0, i32::MAX);
    form.set_enabled(id, !auto);
}

fn chosen_spacing(form: &Form, id: &str) -> StrokeTextSpacing {
    if form.get_checked(&format!("{id}_auto")) {
        StrokeTextSpacing::Auto
    } else {
        StrokeTextSpacing::Manual(form.get_ratio(id))
    }
}

impl StrokeTextDialog {
    /// Opens the dialog; `None` if the text does not exist.
    pub fn new(
        project: &AppProject,
        board: BoardId,
        device: Option<ComponentInstanceId>,
        uuid: Uuid,
        unit: LengthUnit,
    ) -> Option<Self> {
        let p = project.shared().lock();
        let brd = p.project().board(board)?;
        let text = match device {
            Some(c) => brd.device(c)?.stroke_texts().get(&uuid)?.clone(),
            None => brd.stroke_texts().get(&uuid)?.clone(),
        };
        let layers = board_geometry_layers(&brd.copper_layers());
        drop(p);
        let mut form = Form::new(unit);
        form.multiline(
            "text",
            tr!("librepcb::editor::StrokeTextPropertiesDialog", "Text:"),
            text.text(),
            2,
        );
        layer_field(
            &mut form,
            "layer",
            &tr!("librepcb::editor::StrokeTextPropertiesDialog", "Layer:"),
            &layers,
            text.layer(),
        );
        alignment_fields(
            &mut form,
            &tr!("librepcb::editor::StrokeTextPropertiesDialog", "Alignment:"),
            text.align(),
        );
        form.length_with_steps(
            "height",
            tr!("librepcb::editor::StrokeTextPropertiesDialog", "Height:"),
            *text.height(),
            Length::new(1),
            steps::TEXT_HEIGHT,
        );
        form.length(
            "stroke_width",
            tr!(
                "librepcb::editor::StrokeTextPropertiesDialog",
                "Stroke Width:"
            ),
            *text.stroke_width(),
            Length::new(0),
        );
        spacing_fields(
            &mut form,
            "letter_spacing",
            &tr!(
                "librepcb::editor::StrokeTextPropertiesDialog",
                "Letter Spacing:"
            ),
            text.letter_spacing(),
        );
        spacing_fields(
            &mut form,
            "line_spacing",
            &tr!(
                "librepcb::editor::StrokeTextPropertiesDialog",
                "Line Spacing:"
            ),
            text.line_spacing(),
        );
        position_fields(
            &mut form,
            &tr!("librepcb::editor::StrokeTextPropertiesDialog", "Position:"),
            "",
            text.position(),
        );
        form.angle(
            "rotation",
            tr!("librepcb::editor::StrokeTextPropertiesDialog", "Rotation:"),
            text.rotation(),
        );
        form.checkbox(
            "auto_rotate",
            "",
            tr!(
                "librepcb::editor::StrokeTextPropertiesDialog",
                "Auto-Rotate"
            ),
            text.auto_rotate(),
        );
        form.checkbox(
            "mirror",
            tr!("librepcb::editor::StrokeTextPropertiesDialog", "Options:"),
            tr!("librepcb::editor::StrokeTextPropertiesDialog", "Mirror"),
            text.mirrored(),
        );
        form.checkbox(
            "lock",
            "",
            tr!("librepcb::editor::StrokeTextPropertiesDialog", "Lock"),
            text.locked(),
        );
        Some(Self {
            form,
            board,
            device,
            text,
        })
    }
}

impl FormDialog for StrokeTextDialog {
    fn title(&self) -> String {
        tr!(
            "librepcb::editor::StrokeTextPropertiesDialog",
            "Stroke Text Properties"
        )
    }

    form_accessors!();

    fn field_event(&mut self, _ctx: &DialogContext<'_>, _id: &str, _event: FieldEvent) {
        for id in ["letter_spacing", "line_spacing"] {
            let auto = self.form.get_checked(&format!("{id}_auto"));
            self.form.set_enabled(id, !auto);
        }
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let mut text = self.text.clone();
        text.set_text(form.get_text("text"));
        if let Some(layer) = chosen_layer(form, "layer") {
            text.set_layer(layer);
        }
        text.set_align(chosen_alignment(form));
        text.set_height(positive(form, "height")?);
        text.set_stroke_width(unsigned(form, "stroke_width")?);
        text.set_letter_spacing(chosen_spacing(form, "letter_spacing"));
        text.set_line_spacing(chosen_spacing(form, "line_spacing"));
        text.set_position(chosen_position(form));
        text.set_rotation(form.get_angle("rotation"));
        text.set_auto_rotate(form.get_checked("auto_rotate"));
        text.set_mirrored(form.get_checked("mirror"));
        text.set_locked(form.get_checked("lock"));
        let board = self.board;
        let label = tr!("librepcb::editor::CmdStrokeTextEdit", "Edit stroke text");
        match self.device {
            None => {
                let item = BoardItem::StrokeText(text.clone());
                transaction(ctx.project()?, label, |e| {
                    e.execute(UpdateBoardItem {
                        board: Some(board),
                        item,
                    })
                })?;
            }
            Some(c) => {
                let mut device = ctx
                    .project()?
                    .shared()
                    .lock()
                    .project()
                    .board(board)
                    .and_then(|b| b.device(c))
                    .cloned()
                    .ok_or_else(|| "Device not found".to_owned())?;
                device.insert_stroke_text(text.clone());
                transaction(ctx.project()?, label.clone(), |e| {
                    e.apply_mutations(
                        label,
                        vec![Mutation::Board(BoardMutation::UpdateDevice {
                            board,
                            device,
                        })],
                    )
                })?;
            }
        }
        self.text = text;
        Ok(Applied::Project)
    }
}

// --- Hole ------------------------------------------------------------------------------------

/// The properties dialog of a board hole (upstream `HolePropertiesDialog`
/// for `BI_Hole`).
pub struct HoleDialog {
    form: Form,
    board: BoardId,
    hole: librepcb_core::project::board::BoardHoleData,
}

impl HoleDialog {
    /// Opens the dialog; `None` if the hole does not exist.
    pub fn new(project: &AppProject, board: BoardId, uuid: Uuid, unit: LengthUnit) -> Option<Self> {
        let hole = project
            .shared()
            .lock()
            .project()
            .board(board)?
            .holes()
            .get(&uuid)?
            .clone();
        let mut form = Form::new(unit);
        form.page(tr!("librepcb::editor::HolePropertiesDialog", "General"));
        form.length_with_steps(
            "diameter",
            tr!("librepcb::editor::HoleEditorWidget", "Diameter:"),
            *hole.diameter(),
            Length::new(1),
            steps::DRILL_DIAMETER,
        );
        form.checkbox(
            "lock",
            tr!("librepcb::editor::HoleEditorWidget", "Options:"),
            tr!("librepcb::editor::HoleEditorWidget", "Lock"),
            hole.locked(),
        );
        path_fields(&mut form, hole.path());
        form.page(tr!("librepcb::editor::HolePropertiesDialog", "Advanced"));
        mask_config_fields(
            &mut form,
            "stop_mask",
            &tr!("librepcb::editor::HolePropertiesDialog", "Stop Mask:"),
            "librepcb::editor::HolePropertiesDialog",
            hole.stop_mask_config(),
        );
        Some(Self { form, board, hole })
    }
}

impl FormDialog for HoleDialog {
    fn title(&self) -> String {
        tr!("librepcb::editor::HolePropertiesDialog", "Hole Properties")
    }

    form_accessors!();

    fn field_event(&mut self, _ctx: &DialogContext<'_>, _id: &str, _event: FieldEvent) {
        update_mask_config_fields(&self.form, "stop_mask");
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let mut hole = self.hole.clone();
        hole.set_diameter(positive(form, "diameter")?);
        hole.set_locked(form.get_checked("lock"));
        let path = chosen_path(form, hole.path());
        hole.set_path(librepcb_core::geometry::NonEmptyPath::new(path).map_err(|e| e.to_string())?);
        hole.set_stop_mask_config(chosen_mask_config(form, "stop_mask"));
        let board = self.board;
        let item = BoardItem::Hole(hole.clone());
        transaction(
            ctx.project()?,
            tr!("librepcb::editor::CmdBoardHoleEdit", "Edit hole"),
            |e| {
                e.execute(UpdateBoardItem {
                    board: Some(board),
                    item,
                })
            },
        )?;
        self.hole = hole;
        Ok(Applied::Project)
    }
}

// --- Zone --------------------------------------------------------------------------------------

/// The properties dialog of a board zone (upstream `ZonePropertiesDialog`
/// for `BI_Zone`).
pub struct ZoneDialog {
    form: Form,
    board: BoardId,
    zone: librepcb_core::project::board::BoardZoneData,
    layers: Vec<Layer>,
}

const ZONE_RULES: [ZoneRules; 4] = [
    ZoneRules::NO_COPPER,
    ZoneRules::NO_PLANES,
    ZoneRules::NO_EXPOSURE,
    ZoneRules::NO_DEVICES,
];

/// The "Rules" check boxes of the zone properties dialogs.
pub(crate) fn zone_rule_fields(form: &mut Form, rules: ZoneRules) {
    form.header(tr!("librepcb::editor::ZonePropertiesDialog", "Rules"));
    let texts = [
        (
            tr!(
                "librepcb::editor::ZonePropertiesDialog",
                "No copper (except planes)"
            ),
            tr!(
                "librepcb::editor::ZonePropertiesDialog",
                "Raise a DRC error if there are any copper objects (e.g. traces or vias) in this zone. Only planes are allowed to flood this zone without raising an error."
            ),
        ),
        (
            tr!("librepcb::editor::ZonePropertiesDialog", "No planes"),
            tr!(
                "librepcb::editor::ZonePropertiesDialog",
                "Prevent copper planes from flooding this zone."
            ),
        ),
        (
            tr!("librepcb::editor::ZonePropertiesDialog", "No exposure"),
            tr!(
                "librepcb::editor::ZonePropertiesDialog",
                "Raise a DRC error if there is any solder resist opening (possibly exposing copper) in this zone."
            ),
        ),
        (
            tr!("librepcb::editor::ZonePropertiesDialog", "No devices"),
            tr!(
                "librepcb::editor::ZonePropertiesDialog",
                "Raise a DRC error if there are any devices placed in this zone."
            ),
        ),
    ];
    for (i, (rule, (text, hint))) in ZONE_RULES.iter().zip(texts).enumerate() {
        let id = format!("rule_{i}");
        form.checkbox(&id, "", text, rules.contains(*rule));
        form.set_hint(&id, hint);
    }
}

/// The rules chosen with [`zone_rule_fields()`].
pub(crate) fn chosen_zone_rules(form: &Form) -> ZoneRules {
    let mut rules = ZoneRules::empty();
    for (i, rule) in ZONE_RULES.iter().enumerate() {
        rules.set(*rule, form.get_checked(&format!("rule_{i}")));
    }
    rules
}

impl ZoneDialog {
    /// Opens the dialog; `None` if the zone does not exist.
    pub fn new(project: &AppProject, board: BoardId, uuid: Uuid, unit: LengthUnit) -> Option<Self> {
        let p = project.shared().lock();
        let brd = p.project().board(board)?;
        let zone = brd.zones().get(&uuid)?.clone();
        let mut layers: BTreeSet<Layer> = brd.copper_layers();
        layers.extend(zone.layers().iter().copied());
        let layers: Vec<Layer> = layers.into_iter().collect();
        drop(p);
        let mut form = Form::new(unit);
        form.header(tr!("librepcb::editor::ZonePropertiesDialog", "Layers"));
        let items: Vec<ListItem> = layers
            .iter()
            .map(|l| ListItem::check(l.name_tr(), zone.layers().contains(l)))
            .collect();
        form.list("layers", "", &[], &items, 4, ListButtons::default());
        zone_rule_fields(&mut form, zone.rules());
        form.header(tr!("librepcb::editor::ZonePropertiesDialog", "Options"));
        form.checkbox(
            "lock",
            "",
            tr!("librepcb::editor::ZonePropertiesDialog", "Lock"),
            zone.locked(),
        );
        path_fields(&mut form, zone.outline());
        Some(Self {
            form,
            board,
            zone,
            layers,
        })
    }
}

impl FormDialog for ZoneDialog {
    fn title(&self) -> String {
        tr!("librepcb::editor::ZonePropertiesDialog", "Zone Properties")
    }

    form_accessors!();

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        if let ("layers", FieldEvent::List(super::ListAction::Toggle(row))) = (id, event) {
            let mut checked = self.form.get_list_checked("layers");
            if let Some(c) = checked.get_mut(row) {
                *c = !*c;
            }
            let items: Vec<ListItem> = self
                .layers
                .iter()
                .zip(checked)
                .map(|(l, c)| ListItem::check(l.name_tr(), c))
                .collect();
            self.form.set_items("layers", &items, Some(row));
        }
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let mut zone = self.zone.clone();
        let layers: BTreeSet<Layer> = self
            .layers
            .iter()
            .zip(form.get_list_checked("layers"))
            .filter(|(_, c)| *c)
            .map(|(l, _)| *l)
            .collect();
        zone.set_layers(layers).map_err(|e| e.to_string())?;
        zone.set_rules(chosen_zone_rules(form));
        zone.set_locked(form.get_checked("lock"));
        zone.set_outline(chosen_path(form, self.zone.outline()));
        let board = self.board;
        let item = BoardItem::Zone(zone.clone());
        transaction(
            ctx.project()?,
            tr!("librepcb::editor::CmdBoardZoneEdit", "Edit zone"),
            |e| {
                e.execute(UpdateBoardItem {
                    board: Some(board),
                    item,
                })
            },
        )?;
        self.zone = zone;
        Ok(Applied::Project)
    }
}

// --- Line width ------------------------------------------------------------------------------

/// The "Set Width" dialog of the board editor (upstream
/// `BoardEditorState_Select::changeLineWidth()` with `QInputDialog`); the
/// width goes back to the FSM.
pub struct LineWidthDialog {
    form: Form,
}

impl LineWidthDialog {
    /// A dialog with the current (median) width.
    pub fn new(current: UnsignedLength, unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        form.length(
            "width",
            format!(
                "{}:",
                tr!("librepcb::editor::BoardEditorState_Select", "Width")
            ),
            *current,
            Length::new(0),
        );
        Self { form }
    }
}

impl FormDialog for LineWidthDialog {
    fn title(&self) -> String {
        tr!("librepcb::editor::BoardEditorState_Select", "Set Width")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            width: 350.0,
            label_width: 80.0,
            ..DialogOptions::default()
        }
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let width = unsigned(&self.form, "width")?;
        Ok(Applied::Tab(TabDialogResult::LineWidth(width)))
    }
}

/// `UnsignedRatio` helper for other dialogs (board setup).
pub fn unsigned_ratio(r: Ratio) -> Result<UnsignedRatio, String> {
    UnsignedRatio::new(r).map_err(|e| e.to_string())
}

// --- DXF import ---------------------------------------------------------------

/// The last choices of the DXF import dialog of an editor (upstream:
/// stored in the client settings under `board_editor/dxf_import_dialog/*`,
/// `symbol_editor/...` and `package_editor/...`; here kept while the
/// application runs).
#[derive(Debug, Clone, PartialEq)]
pub struct DxfImportChoices {
    /// Layer of the imported polygons.
    pub layer: Layer,
    /// Line width.
    pub line_width: UnsignedLength,
    /// Scale factor.
    pub scale_factor: f64,
    /// Place the objects interactively (with the cursor).
    pub interactive: bool,
    /// Fixed position (if not interactive).
    pub position: librepcb_core::types::Point,
    /// Join tangent polylines.
    pub join_tangent_polylines: bool,
    /// Import circles as holes.
    pub circles_as_drills: bool,
}

impl DxfImportChoices {
    /// The defaults of an editor (upstream: the `defaultLayer` argument of
    /// `DxfImportDialog`).
    pub fn defaults(kind: DxfImportKind) -> Self {
        Self {
            layer: match kind {
                DxfImportKind::Board => Layer::BOARD_OUTLINES,
                DxfImportKind::Symbol => Layer::SYMBOL_OUTLINES,
                DxfImportKind::Package => Layer::TOP_DOCUMENTATION,
            },
            line_width: UnsignedLength::default(),
            scale_factor: 1.0,
            interactive: true,
            position: librepcb_core::types::Point::ORIGIN,
            join_tangent_polylines: true,
            circles_as_drills: false,
        }
    }
}

thread_local! {
    static DXF_CHOICES: RefCell<HashMap<DxfImportKind, DxfImportChoices>> =
        RefCell::new(HashMap::new());
}

const DXF_CTX: &str = "librepcb::editor::DxfImportDialog";

/// Port of libs/librepcb/editor/dialogs/dxfimportdialog.{ui,cpp} for the
/// board, symbol and package editors: layer, line width, scale factor,
/// interactive or fixed placement, joining tangent polylines and circles
/// as drills (not in the symbol editor). The result is the FSM's
/// [`DxfImportSettings`](librepcb_editor::fsm::board::DxfImportSettings)
/// for the tab.
pub struct DxfImportDialog {
    form: Form,
    file: librepcb_core::fileio::FilePath,
    kind: DxfImportKind,
}

impl DxfImportDialog {
    /// A dialog for the chosen file, with the allowed layers.
    pub fn new(
        file: librepcb_core::fileio::FilePath,
        layers: &[Layer],
        unit: LengthUnit,
        kind: DxfImportKind,
    ) -> Self {
        let c = DXF_CHOICES.with_borrow(|c| c.get(&kind).cloned());
        let c = c.unwrap_or_else(|| DxfImportChoices::defaults(kind));
        // Choices of another editor version may name a layer which is not
        // allowed here.
        let layer = if layers.contains(&c.layer) {
            c.layer
        } else {
            DxfImportChoices::defaults(kind).layer
        };
        let mut form = Form::new(unit);
        form.label(
            "file",
            tr!("librepcb::editor::CopyOutputJobWidget", "Input File:"),
            file.to_native(),
        );
        layer_field(&mut form, "layer", &tr!(DXF_CTX, "Layer:"), layers, layer);
        form.length(
            "line_width",
            tr!(DXF_CTX, "Line width:"),
            *c.line_width,
            Length::ZERO,
        );
        form.text(
            "scale_factor",
            tr!(DXF_CTX, "Scale factor:"),
            format!("{}", c.scale_factor),
        );
        form.checkbox(
            "interactive",
            tr!(DXF_CTX, "Position:"),
            tr!(DXF_CTX, "Interactive"),
            c.interactive,
        );
        position_fields(&mut form, "X:", "Y:", c.position);
        form.set_enabled("pos_x", !c.interactive);
        form.set_enabled("pos_y", !c.interactive);
        form.checkbox(
            "join",
            tr!(DXF_CTX, "Options:"),
            tr!(DXF_CTX, "Join tangent polylines"),
            c.join_tangent_polylines,
        );
        if kind != DxfImportKind::Symbol {
            form.checkbox(
                "circles_as_drills",
                "",
                tr!(DXF_CTX, "Import circles as drills"),
                c.circles_as_drills,
            );
        }
        Self { form, file, kind }
    }

    /// The choices as import settings.
    fn settings(&self) -> Result<librepcb_editor::fsm::board::DxfImportSettings, String> {
        let scale_text = self.form.get_text("scale_factor");
        let scale_factor = scale_text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite() && *f > 0.0)
            .ok_or_else(|| format!("{} {scale_text}", tr!(DXF_CTX, "Scale factor:")))?;
        let choices = DxfImportChoices {
            layer: chosen_layer(&self.form, "layer")
                .unwrap_or(DxfImportChoices::defaults(self.kind).layer),
            line_width: unsigned(&self.form, "line_width")?,
            scale_factor,
            interactive: self.form.get_checked("interactive"),
            position: chosen_position(&self.form),
            join_tangent_polylines: self.form.get_checked("join"),
            circles_as_drills: self.kind != DxfImportKind::Symbol
                && self.form.get_checked("circles_as_drills"),
        };
        DXF_CHOICES.with_borrow_mut(|c| c.insert(self.kind, choices.clone()));
        Ok(librepcb_editor::fsm::board::DxfImportSettings {
            file: self.file.clone(),
            layer: choices.layer,
            line_width: choices.line_width,
            scale_factor: choices.scale_factor,
            join_tangent_polylines: choices.join_tangent_polylines,
            circles_as_drills: choices.circles_as_drills,
            placement: (!choices.interactive).then_some(choices.position),
        })
    }
}

impl FormDialog for DxfImportDialog {
    fn title(&self) -> String {
        tr!(DXF_CTX, "DXF Import")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            width: 450.0,
            label_width: 110.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, _event: FieldEvent) {
        if id == "interactive" {
            let interactive = self.form.get_checked("interactive");
            self.form.set_enabled("pos_x", !interactive);
            self.form.set_enabled("pos_y", !interactive);
        }
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        Ok(Applied::Tab(TabDialogResult::ImportDxf(self.settings()?)))
    }
}
