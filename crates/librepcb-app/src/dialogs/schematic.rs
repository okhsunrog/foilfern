//! Properties dialogs of the schematic editor.
//!
//! Ports of libs/librepcb/editor/project/schematic/symbolinstancepropertiesdialog.{ui,cpp},
//! renamenetsegmentdialog.{ui,cpp} and the schematic uses of
//! libs/librepcb/editor/dialogs/{polygon,text}propertiesdialog.{ui,cpp}.
//!
//! Differences to upstream: the name swap question of the symbol dialog
//! ("There is already a component with the name ...") is shown in the
//! dialog; clicking "OK" again swaps the names. The assembly options are
//! listed with their assembly variants (editable for the selected option)
//! and can be removed, but not added (upstream's
//! `ComponentAssemblyOptionListEditorWidget` also adds devices and parts).
//! The library element names are plain text instead of links.

use std::collections::BTreeSet;

use librepcb_core::attribute::AttributeList;
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::circuit::ComponentAssemblyOptionList;
use librepcb_core::project::{
    AssemblyVariantId, BusSegmentId, BusSegmentRef, ComponentInstanceId, Mutation, NetSegmentId,
    NetSegmentRef, SchematicId, SchematicMutation, SymbolId, SymbolRef,
};
use librepcb_core::types::{BusName, CircuitIdentifier, Layer, Length, LengthUnit, Uuid};
use librepcb_editor::commands::{
    ChangeNetOfSchematicSegment, EditComponent, EditNet, MoveSymbol, RenameBusSegment,
};
use librepcb_i18n::tr;
use std::cell::RefCell;
use std::rc::Rc;

use super::attributes::AttributeEditor;
use super::geometry::{
    alignment_fields, chosen_alignment, chosen_layer, chosen_path, chosen_position, layer_field,
    path_fields, position_fields,
};
use super::{
    Applied, DialogContext, FieldEvent, Form, FormDialog, ListAction, ListButtons, ListItem,
    sort_numeric, transaction,
};
use crate::project::AppProject;

/// The layers of schematic polygons and texts (upstream
/// `SchematicEditor` passes the schematic geometry layers).
pub fn schematic_geometry_layers() -> Vec<Layer> {
    vec![
        Layer::SYMBOL_OUTLINES,
        Layer::SYMBOL_NAMES,
        Layer::SYMBOL_VALUES,
        Layer::SCHEMATIC_SHEET_FRAMES,
        Layer::SCHEMATIC_DOCUMENTATION,
        Layer::SCHEMATIC_COMMENTS,
        Layer::SCHEMATIC_GUIDE,
    ]
}

// --- Symbol instance ---------------------------------------------------------

/// The properties dialog of a symbol and its component instance (upstream
/// `SymbolInstancePropertiesDialog`).
pub struct SymbolPropertiesDialog {
    form: Form,
    title: String,
    symbol: SymbolId,
    component: ComponentInstanceId,
    has_name: bool,
    attributes: Rc<RefCell<AttributeEditor>>,
    options: ComponentAssemblyOptionList,
    variants: Vec<(AssemblyVariantId, String)>,
    device_names: Vec<String>,
    /// The name for which the user was asked to swap names.
    swap_confirmed: Option<String>,
}

impl SymbolPropertiesDialog {
    /// Opens the dialog for a symbol; `None` if it does not exist.
    pub fn new(project: &AppProject, symbol: SymbolId, unit: LengthUnit) -> Option<Self> {
        let p = project.shared().lock();
        let prj = p.project();
        let view = prj.view();
        let (_, sym) = prj
            .schematics()
            .iter()
            .find_map(|s| s.symbols().get(&symbol).map(|sym| (s.id(), sym)))?;
        let resolved = sym.resolve(view).ok()?;
        let cmp = resolved.component;
        let locales = &prj.settings().locale_order;
        let sym_name = sym.name(view).unwrap_or_default();
        let has_name = !cmp.is_pure_schematic_only(resolved.lib_component);
        let mut form = Form::new(unit);

        form.header(tr!(
            "librepcb::editor::SymbolInstancePropertiesDialog",
            "Component"
        ));
        if has_name {
            form.text(
                "name",
                tr!("librepcb::editor::SymbolInstancePropertiesDialog", "Name:"),
                cmp.name().as_str(),
            );
        }
        form.multiline(
            "value",
            tr!("librepcb::editor::SymbolInstancePropertiesDialog", "Value:"),
            cmp.value(),
            2,
        );
        let variant_name = resolved
            .lib_component
            .symbol_variants()
            .by_uuid(&cmp.lib_variant())
            .map(|v| v.names().value(locales).to_string())
            .unwrap_or_default();
        form.label(
            "lib_component",
            tr!(
                "librepcb::editor::SymbolInstancePropertiesDialog",
                "Component:"
            ),
            format!(
                "{} ({})",
                resolved.lib_component.metadata().names().value(locales),
                tr!(
                    "librepcb::editor::SymbolInstancePropertiesDialog",
                    "symbol variant \"{0}\"",
                    variant_name
                )
            ),
        );

        let suffix = resolved.gate.suffix().as_str().to_owned();
        form.header(if suffix.is_empty() {
            tr!("librepcb::editor::SymbolInstancePropertiesDialog", "Gate")
        } else {
            tr!(
                "librepcb::editor::SymbolInstancePropertiesDialog",
                "Gate '{0}'",
                suffix
            )
        });
        position_fields(
            &mut form,
            &tr!(
                "librepcb::editor::SymbolInstancePropertiesDialog",
                "Pos. X:"
            ),
            &tr!(
                "librepcb::editor::SymbolInstancePropertiesDialog",
                "Pos. Y:"
            ),
            sym.position(),
        );
        form.angle(
            "rotation",
            tr!(
                "librepcb::editor::SymbolInstancePropertiesDialog",
                "Rotation:"
            ),
            sym.rotation(),
        );
        form.checkbox(
            "mirror",
            tr!(
                "librepcb::editor::SymbolInstancePropertiesDialog",
                "Mirror:"
            ),
            "",
            sym.mirrored(),
        );
        form.label(
            "lib_symbol",
            tr!(
                "librepcb::editor::SymbolInstancePropertiesDialog",
                "Symbol:"
            ),
            resolved
                .lib_symbol
                .metadata()
                .names()
                .value(locales)
                .as_str(),
        );

        let attributes = AttributeEditor::new(cmp.attributes());
        form.header(tr!(
            "librepcb::editor::SymbolInstancePropertiesDialog",
            "Attributes of Component"
        ));
        form.attributes("attributes", "", attributes.borrow().model_rc());

        let variants: Vec<(AssemblyVariantId, String)> = prj
            .circuit()
            .assembly_variants()
            .iter()
            .map(|v| (AssemblyVariantId(v.uuid()), v.name().to_string()))
            .collect();
        let device_names: Vec<String> = cmp
            .assembly_options()
            .iter()
            .map(|o| {
                prj.library()
                    .device(&o.device())
                    .map(|d| d.metadata().names().value(locales).to_string())
                    .unwrap_or_else(|| o.device().to_string())
            })
            .collect();
        let options = cmp.assembly_options().clone();
        let title = tr!(
            "librepcb::editor::SymbolInstancePropertiesDialog",
            "Properties of {0}",
            sym_name
        );
        let component = cmp.id();
        drop(p);
        let mut dialog = Self {
            form,
            title,
            symbol,
            component,
            has_name,
            attributes,
            options,
            variants,
            device_names,
            swap_confirmed: None,
        };
        dialog.add_assembly_fields();
        Some(dialog)
    }

    fn option_items(&self) -> Vec<ListItem> {
        self.options
            .iter()
            .zip(&self.device_names)
            .map(|(o, name)| {
                let parts: Vec<String> = o.parts().iter().map(|p| p.mpn().to_string()).collect();
                let variants: Vec<&str> = self
                    .variants
                    .iter()
                    .filter(|(id, _)| o.assembly_variants().contains(id))
                    .map(|(_, n)| n.as_str())
                    .collect();
                ListItem::row(vec![name.clone(), parts.join(", "), variants.join(", ")])
            })
            .collect()
    }

    fn variant_items(&self, option: Option<usize>) -> Vec<ListItem> {
        let selected: BTreeSet<AssemblyVariantId> = option
            .and_then(|i| self.options.get(i))
            .map(|o| o.assembly_variants().clone())
            .unwrap_or_default();
        self.variants
            .iter()
            .map(|(id, name)| ListItem::check(name, selected.contains(id)))
            .collect()
    }

    fn add_assembly_fields(&mut self) {
        self.form.header(tr!(
            "librepcb::editor::SymbolInstancePropertiesDialog",
            "Assembly Options (BOM/PnP)"
        ));
        let columns = vec![
            tr!(
                "librepcb::editor::ComponentAssemblyOptionListEditorWidget",
                "Board Device"
            ),
            tr!(
                "librepcb::editor::ComponentAssemblyOptionListEditorWidget",
                "Part Number"
            ),
            tr!("librepcb::editor::ProjectSetupDialog", "Assembly Variants"),
        ];
        let items = self.option_items();
        self.form.list(
            "options",
            "",
            &columns,
            &items,
            3,
            ListButtons {
                remove: true,
                ..ListButtons::default()
            },
        );
        let variants = self.variant_items(None);
        self.form.list(
            "option_variants",
            tr!("librepcb::editor::ProjectSetupDialog", "Assembly Variants"),
            &[],
            &variants,
            2,
            ListButtons::default(),
        );
        self.form.set_enabled("option_variants", false);
    }

    fn refresh_options(&self, selected: Option<usize>) {
        self.form
            .set_items("options", &self.option_items(), selected);
        self.form
            .set_items("option_variants", &self.variant_items(selected), None);
        self.form.set_enabled("option_variants", selected.is_some());
    }

    /// The attribute editor (tests).
    pub fn attributes(&self) -> &Rc<RefCell<AttributeEditor>> {
        &self.attributes
    }
}

impl FormDialog for SymbolPropertiesDialog {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn form(&self) -> &Form {
        &self.form
    }

    fn form_mut(&mut self) -> &mut Form {
        &mut self.form
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        match (id, event) {
            ("options", FieldEvent::List(ListAction::Select(i))) => {
                self.refresh_options(Some(i));
            }
            ("options", FieldEvent::List(ListAction::Remove(i))) if i < self.options.len() => {
                self.options.take(i);
                self.device_names.remove(i);
                self.refresh_options(None);
            }
            ("option_variants", FieldEvent::List(ListAction::Toggle(v))) => {
                let (Some(option), Some((variant, _))) =
                    (self.form.get_index("options"), self.variants.get(v))
                else {
                    return;
                };
                let Some(o) = self.options.get_mut(option) else {
                    return;
                };
                let mut set = o.assembly_variants().clone();
                if !set.remove(variant) {
                    set.insert(*variant);
                }
                o.set_assembly_variants(set);
                self.refresh_options(Some(option));
            }
            ("name", FieldEvent::Edited) => {
                let text = self.form.get_text("name");
                let valid = CircuitIdentifier::new(CircuitIdentifier::clean(&text)).is_ok();
                self.form.set_error(
                    "name",
                    if valid {
                        String::new()
                    } else {
                        tr!("SlintHelpers", "Invalid")
                    },
                );
            }
            _ => {}
        }
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let attributes: AttributeList = self.attributes.borrow().list()?;
        let new_name = if self.has_name {
            Some(CircuitIdentifier::new(form.get_text("name").trim()).map_err(|e| e.to_string())?)
        } else {
            None
        };
        // Name already used by another component: ask to swap the names.
        let (old_name, other) = {
            let p = ctx.project()?.shared().lock();
            let circuit = p.project().circuit();
            let old = circuit
                .component_instance(self.component)
                .map(|c| c.name().clone())
                .ok_or_else(|| "Component not found".to_owned())?;
            let other = new_name.as_ref().and_then(|n| {
                circuit
                    .component_instance_by_name(n.as_str())
                    .map(|(id, _)| id)
                    .filter(|id| *id != self.component)
            });
            (old, other)
        };
        if let (Some(other), Some(new)) = (other, &new_name) {
            if self.swap_confirmed.as_deref() != Some(new.as_str()) {
                self.swap_confirmed = Some(new.as_str().to_owned());
                let tmpl = |a: &str, b: &str| {
                    tr!(
                        "librepcb::editor::SymbolInstancePropertiesDialog",
                        "{0} gets renamed to {1}",
                        a,
                        b
                    )
                };
                return Err(format!(
                    "{}\n\n • {}\n • {}\n\n{}",
                    tr!(
                        "librepcb::editor::SymbolInstancePropertiesDialog",
                        "There is already a component with the name '{0}' in the schematic. Do you want to swap their names?",
                        new.as_str()
                    ),
                    tmpl(new.as_str(), old_name.as_str()),
                    tmpl(old_name.as_str(), new.as_str()),
                    tr!(
                        "librepcb::editor::SymbolInstancePropertiesDialog",
                        "Click OK again to swap the names."
                    )
                ));
            }
            let _ = other;
        }
        let position = chosen_position(form);
        let rotation = form.get_angle("rotation");
        let mirrored = form.get_checked("mirror");
        let value = form.get_text("value");
        let options = self.options.clone();
        let component = self.component;
        let symbol = self.symbol;
        let text = tr!(
            "librepcb::editor::SymbolInstancePropertiesDialog",
            "Change properties of {0}",
            self.title.clone()
        );
        transaction(ctx.project()?, text, |e| {
            if let Some(other) = other {
                e.execute(EditComponent {
                    component: component.into(),
                    name: Some(CircuitIdentifier::new("_tmp_swap_names_")?),
                    value: None,
                    attributes: None,
                    assembly_options: None,
                    lock_assembly: None,
                })?;
                e.execute(EditComponent {
                    component: other.into(),
                    name: Some(old_name.clone()),
                    value: None,
                    attributes: None,
                    assembly_options: None,
                    lock_assembly: None,
                })?;
            }
            e.execute(EditComponent {
                component: component.into(),
                name: new_name,
                value: Some(value),
                attributes: Some(attributes),
                assembly_options: Some(options),
                lock_assembly: None,
            })?;
            e.execute(MoveSymbol {
                symbol,
                position: Some(position),
                rotation: Some(rotation),
                mirrored: Some(mirrored),
            })?;
            Ok(())
        })?;
        self.swap_confirmed = None;
        Ok(Applied::Project)
    }
}

// --- Net / bus segment rename ------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameAction {
    None,
    InvalidName,
    RenameWhole,
    Merge,
    MoveToExisting,
    MoveToNew,
}

/// The segment of a rename dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameSegment {
    Net(NetSegmentRef),
    Bus(BusSegmentRef),
}

const RENAME_NET: &str = "librepcb::editor::RenameNetSegmentDialog";
const RENAME_BUS: &str = "librepcb::editor::RenameBusSegmentDialog";

/// The rename dialogs of schematic net and bus segments (upstream
/// `RenameNetSegmentDialog` and `RenameBusSegmentDialog`, opened for net
/// and bus labels).
pub struct RenameSegmentDialog {
    form: Form,
    segment: RenameSegment,
    old_name: String,
    action: RenameAction,
}

impl RenameSegmentDialog {
    /// Opens the dialog for a net segment; `None` if it does not exist.
    pub fn for_net(project: &AppProject, schematic: SchematicId, segment: Uuid) -> Option<Self> {
        let p = project.shared().lock();
        let prj = p.project();
        let seg = prj
            .schematic(schematic)?
            .net_segments()
            .get(&NetSegmentId(segment))?;
        let net = prj.circuit().net_signal(seg.net())?;
        let old_name = net.name().to_string();
        let names: Vec<String> = prj
            .circuit()
            .net_signals()
            .values()
            .filter(|n| !n.has_auto_name())
            .map(|n| n.name().to_string())
            .collect();
        let segment_count = prj
            .schematics()
            .iter()
            .flat_map(|s| s.net_segments().values())
            .filter(|s| s.net() == seg.net())
            .count();
        drop(p);
        let segment = RenameSegment::Net(NetSegmentRef {
            schematic,
            segment: NetSegmentId(segment),
        });
        Some(Self::with(project, segment, old_name, names, segment_count))
    }

    /// Opens the dialog for a bus segment; `None` if it does not exist.
    pub fn for_bus(project: &AppProject, schematic: SchematicId, segment: Uuid) -> Option<Self> {
        let p = project.shared().lock();
        let prj = p.project();
        let seg = prj
            .schematic(schematic)?
            .bus_segments()
            .get(&BusSegmentId(segment))?;
        let old_name = prj.circuit().bus(seg.bus())?.name().to_string();
        let names: Vec<String> = prj
            .circuit()
            .buses()
            .values()
            .filter(|b| !b.has_auto_name())
            .map(|b| b.name().to_string())
            .collect();
        let segment_count = prj.bus_uses(seg.bus()).count();
        drop(p);
        let segment = RenameSegment::Bus(BusSegmentRef {
            schematic,
            segment: BusSegmentId(segment),
        });
        Some(Self::with(project, segment, old_name, names, segment_count))
    }

    fn with(
        project: &AppProject,
        segment: RenameSegment,
        old_name: String,
        mut names: Vec<String>,
        segment_count: usize,
    ) -> Self {
        sort_numeric(&mut names);
        let mut form = Form::new(LengthUnit::Millimeters);
        let (label, options) = match segment {
            RenameSegment::Net(_) => (
                tr!("librepcb::editor::RenameNetSegmentDialog", "Net name:"),
                vec![
                    tr!(
                        "librepcb::editor::RenameNetSegmentDialog",
                        "Rename only this net segment"
                    ),
                    tr!(
                        "librepcb::editor::RenameNetSegmentDialog",
                        "Rename whole net ({0} segments)",
                        segment_count
                    ),
                ],
            ),
            RenameSegment::Bus(_) => (
                tr!("librepcb::editor::RenameBusSegmentDialog", "Bus name:"),
                vec![
                    tr!(
                        "librepcb::editor::RenameBusSegmentDialog",
                        "Rename only this bus segment"
                    ),
                    tr!(
                        "librepcb::editor::RenameBusSegmentDialog",
                        "Rename whole bus ({0} segments)",
                        segment_count
                    ),
                ],
            ),
        };
        form.text_with_suggestions("name", label, &old_name, &names);
        // Upstream: "only this segment" is checked by default, the whole
        // net/bus if it has only one segment (then the choice is disabled).
        form.radio("scope", "", &options, usize::from(segment_count <= 1));
        if segment_count <= 1 {
            form.set_enabled("scope", false);
        }
        form.note("description", "");
        let mut dialog = Self {
            form,
            segment,
            old_name,
            action: RenameAction::None,
        };
        dialog.update_action(project);
        dialog
    }

    fn is_bus(&self) -> bool {
        matches!(self.segment, RenameSegment::Bus(_))
    }

    fn new_name(&self) -> String {
        let text = self.form.get_text("name");
        if self.is_bus() {
            BusName::clean(&text)
        } else {
            CircuitIdentifier::clean(&text)
        }
    }

    /// Upstream `updateAction()`.
    fn update_action(&mut self, project: &AppProject) {
        let name = self.new_name();
        let whole = self.form.get_index("scope") == Some(1);
        let bus = self.is_bus();
        let valid = if bus {
            BusName::new(name.clone()).is_ok()
        } else {
            CircuitIdentifier::new(name.clone()).is_ok()
        };
        let exists = {
            let p = project.shared().lock();
            let circuit = p.project().circuit();
            if bus {
                circuit.bus_by_name(&name).is_some()
            } else {
                circuit.net_signal_by_name(&name).is_some()
            }
        };
        let old = self.old_name.as_str();
        let n = name.as_str();
        let ctx = if bus { RENAME_BUS } else { RENAME_NET };
        let (action, desc) = if !valid {
            (RenameAction::InvalidName, tr!(ctx, "Invalid name!"))
        } else if n == old {
            (RenameAction::None, tr!(ctx, "No change is made."))
        } else if whole && exists {
            (
                RenameAction::Merge,
                if bus {
                    tr!(
                        "librepcb::editor::RenameBusSegmentDialog",
                        "The whole bus '{0}' will be merged into the bus '{1}'.",
                        old,
                        n
                    )
                } else {
                    tr!(
                        "librepcb::editor::RenameNetSegmentDialog",
                        "The whole net '{0}' will be merged into the net '{1}'.",
                        old,
                        n
                    )
                },
            )
        } else if whole {
            (
                RenameAction::RenameWhole,
                if bus {
                    tr!(
                        "librepcb::editor::RenameBusSegmentDialog",
                        "The whole bus '{0}' will be renamed to '{1}'.",
                        old,
                        n
                    )
                } else {
                    tr!(
                        "librepcb::editor::RenameNetSegmentDialog",
                        "The whole net '{0}' will be renamed to '{1}'.",
                        old,
                        n
                    )
                },
            )
        } else if exists {
            (
                RenameAction::MoveToExisting,
                if bus {
                    tr!(
                        "librepcb::editor::RenameBusSegmentDialog",
                        "The segment will be moved to the existing bus '{0}'.",
                        n
                    )
                } else {
                    tr!(
                        "librepcb::editor::RenameNetSegmentDialog",
                        "The segment will be moved to the existing net '{0}'.",
                        n
                    )
                },
            )
        } else {
            (
                RenameAction::MoveToNew,
                if bus {
                    tr!(
                        "librepcb::editor::RenameBusSegmentDialog",
                        "The segment will be moved to the new bus '{0}'.",
                        n
                    )
                } else {
                    tr!(
                        "librepcb::editor::RenameNetSegmentDialog",
                        "The segment will be moved to the new net '{0}'.",
                        n
                    )
                },
            )
        };
        self.action = action;
        self.form.set_text("description", desc);
        self.form.set_error(
            "name",
            if action == RenameAction::InvalidName {
                tr!("SlintHelpers", "Invalid")
            } else {
                String::new()
            },
        );
    }
}

impl FormDialog for RenameSegmentDialog {
    fn title(&self) -> String {
        if self.is_bus() {
            tr!(
                "librepcb::editor::RenameBusSegmentDialog",
                "Rename Bus Segment"
            )
        } else {
            tr!(
                "librepcb::editor::RenameNetSegmentDialog",
                "Rename net segment"
            )
        }
    }

    fn form(&self) -> &Form {
        &self.form
    }

    fn form_mut(&mut self) -> &mut Form {
        &mut self.form
    }

    fn options(&self) -> super::DialogOptions {
        super::DialogOptions {
            apply: false,
            width: 450.0,
            label_width: 80.0,
            ..super::DialogOptions::default()
        }
    }

    fn field_event(&mut self, ctx: &DialogContext<'_>, _id: &str, _event: FieldEvent) {
        if let Ok(p) = ctx.project() {
            self.update_action(p);
        }
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        self.update_action(ctx.project()?);
        let name = self.new_name();
        match (self.action, self.segment) {
            (RenameAction::None, _) => Ok(Applied::Nothing),
            (RenameAction::InvalidName, _) => {
                let ctx = if self.is_bus() {
                    RENAME_BUS
                } else {
                    RENAME_NET
                };
                Err(tr!(ctx, "Invalid name!"))
            }
            (action, RenameSegment::Bus(segment)) => {
                let name = BusName::new(name).map_err(|e| e.to_string())?;
                let whole_bus = matches!(action, RenameAction::RenameWhole | RenameAction::Merge);
                let text = match action {
                    RenameAction::Merge => {
                        tr!("librepcb::editor::CmdCombineBuses", "Combine Net Signals")
                    }
                    RenameAction::RenameWhole => tr!("librepcb::editor::CmdBusEdit", "Edit Bus"),
                    _ => tr!(
                        "librepcb::editor::RenameBusSegmentDialog",
                        "Change Bus of Bus Segment"
                    ),
                };
                transaction(ctx.project()?, text, |e| {
                    e.execute(RenameBusSegment {
                        segment,
                        name,
                        whole_bus,
                    })
                })?;
                Ok(Applied::Project)
            }
            (action @ (RenameAction::RenameWhole | RenameAction::Merge), RenameSegment::Net(_)) => {
                let net = CircuitIdentifier::new(name).map_err(|e| e.to_string())?;
                let merge = action == RenameAction::Merge;
                let text = if merge {
                    tr!(
                        "librepcb::editor::CmdCombineNetSignals",
                        "Combine Net Signals"
                    )
                } else {
                    tr!("librepcb::editor::CmdNetSignalEdit", "Edit netsignal")
                };
                let old = self.old_name.as_str();
                transaction(ctx.project()?, text, |e| {
                    e.execute(EditNet {
                        net: old.into(),
                        name: Some(net),
                        net_class: None,
                        merge,
                    })
                })?;
                Ok(Applied::Project)
            }
            (_, RenameSegment::Net(segment)) => {
                let net = CircuitIdentifier::new(name).map_err(|e| e.to_string())?;
                transaction(
                    ctx.project()?,
                    tr!(
                        "librepcb::editor::RenameNetSegmentDialog",
                        "Change net of net segment"
                    ),
                    |e| e.execute(ChangeNetOfSchematicSegment { segment, net }),
                )?;
                Ok(Applied::Project)
            }
        }
    }
}

// --- Polygon -------------------------------------------------------------------

/// The properties dialog of a schematic polygon (upstream
/// `PolygonPropertiesDialog` for `SI_Polygon`, without the lock option
/// which schematic polygons do not have).
pub struct SchematicPolygonDialog {
    form: Form,
    schematic: SchematicId,
    polygon: librepcb_core::geometry::Polygon,
}

impl SchematicPolygonDialog {
    /// Opens the dialog; `None` if the polygon does not exist.
    pub fn new(
        project: &AppProject,
        schematic: SchematicId,
        uuid: Uuid,
        unit: LengthUnit,
    ) -> Option<Self> {
        let polygon = project
            .shared()
            .lock()
            .project()
            .schematic(schematic)?
            .polygons()
            .get(&uuid)?
            .clone();
        let mut form = Form::new(unit);
        layer_field(
            &mut form,
            "layer",
            &tr!("librepcb::editor::PolygonPropertiesDialog", "Layer:"),
            &schematic_geometry_layers(),
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
        path_fields(&mut form, polygon.path());
        Some(Self {
            form,
            schematic,
            polygon,
        })
    }
}

impl FormDialog for SchematicPolygonDialog {
    fn title(&self) -> String {
        tr!(
            "librepcb::editor::PolygonPropertiesDialog",
            "Polygon Properties"
        )
    }

    fn form(&self) -> &Form {
        &self.form
    }

    fn form_mut(&mut self) -> &mut Form {
        &mut self.form
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let mut polygon = self.polygon.clone();
        if let Some(layer) = chosen_layer(&self.form, "layer") {
            polygon.set_layer(layer);
        }
        polygon.set_line_width(
            librepcb_core::types::UnsignedLength::new(self.form.get_length("line_width"))
                .map_err(|e| e.to_string())?,
        );
        polygon.set_is_filled(self.form.get_checked("fill"));
        polygon.set_is_grab_area(self.form.get_checked("grab_area"));
        polygon.set_path(chosen_path(&self.form, self.polygon.path()));
        let schematic = self.schematic;
        transaction(
            ctx.project()?,
            tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"),
            |e| {
                e.apply_mutations(
                    tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"),
                    vec![Mutation::Schematic(SchematicMutation::UpdatePolygon {
                        schematic,
                        polygon: polygon.clone(),
                    })],
                )
            },
        )?;
        self.polygon = polygon;
        Ok(Applied::Project)
    }
}

// --- Text ------------------------------------------------------------------------

/// The properties dialog of a schematic text (upstream
/// `TextPropertiesDialog`).
pub struct SchematicTextDialog {
    form: Form,
    owner: TextOwner,
    text: librepcb_core::geometry::Text,
}

/// Where a schematic text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextOwner {
    /// A text of the schematic.
    Schematic(SchematicId),
    /// A text of a symbol.
    Symbol(SymbolRef),
}

impl SchematicTextDialog {
    /// Opens the dialog; `None` if the text does not exist.
    pub fn new(
        project: &AppProject,
        schematic: SchematicId,
        uuid: Uuid,
        unit: LengthUnit,
    ) -> Option<Self> {
        let text = project
            .shared()
            .lock()
            .project()
            .schematic(schematic)?
            .texts()
            .get(&uuid)?
            .clone();
        Some(Self::with_text(TextOwner::Schematic(schematic), text, unit))
    }

    /// Opens the dialog for a text of a symbol; `None` if it does not
    /// exist.
    pub fn for_symbol(
        project: &AppProject,
        symbol: SymbolId,
        uuid: Uuid,
        unit: LengthUnit,
    ) -> Option<Self> {
        let p = project.shared().lock();
        let (schematic, text) = p.project().schematics().iter().find_map(|s| {
            let text = s.symbols().get(&symbol)?.texts().get(&uuid)?.clone();
            Some((s.id(), text))
        })?;
        drop(p);
        Some(Self::with_text(
            TextOwner::Symbol(SymbolRef { schematic, symbol }),
            text,
            unit,
        ))
    }

    fn with_text(owner: TextOwner, text: librepcb_core::geometry::Text, unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        form.multiline(
            "text",
            tr!("librepcb::editor::TextPropertiesDialog", "Text:"),
            text.text(),
            3,
        );
        layer_field(
            &mut form,
            "layer",
            &tr!("librepcb::editor::TextPropertiesDialog", "Layer:"),
            &schematic_geometry_layers(),
            text.layer(),
        );
        alignment_fields(
            &mut form,
            &tr!("librepcb::editor::TextPropertiesDialog", "Alignment:"),
            text.align(),
        );
        form.length_with_steps(
            "height",
            tr!("librepcb::editor::TextPropertiesDialog", "Height:"),
            *text.height(),
            Length::new(1),
            crate::length_edit::steps::TEXT_HEIGHT,
        );
        position_fields(
            &mut form,
            &tr!("librepcb::editor::TextPropertiesDialog", "Position:"),
            "",
            text.position(),
        );
        form.angle(
            "rotation",
            tr!("librepcb::editor::TextPropertiesDialog", "Rotation:"),
            text.rotation(),
        );
        form.checkbox(
            "lock",
            tr!("librepcb::editor::TextPropertiesDialog", "Options:"),
            tr!("librepcb::editor::TextPropertiesDialog", "Lock"),
            text.locked(),
        );
        Self { form, owner, text }
    }
}

impl FormDialog for SchematicTextDialog {
    fn title(&self) -> String {
        tr!("librepcb::editor::TextPropertiesDialog", "Text Properties")
    }

    fn form(&self) -> &Form {
        &self.form
    }

    fn form_mut(&mut self) -> &mut Form {
        &mut self.form
    }

    fn apply(&mut self, ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let form = &self.form;
        let mut text = self.text.clone();
        text.set_text(form.get_text("text"));
        if let Some(layer) = chosen_layer(form, "layer") {
            text.set_layer(layer);
        }
        text.set_align(chosen_alignment(form));
        text.set_height(
            librepcb_core::types::PositiveLength::new(form.get_length("height"))
                .map_err(|e| e.to_string())?,
        );
        text.set_position(chosen_position(form));
        text.set_rotation(form.get_angle("rotation"));
        text.set_locked(form.get_checked("lock"));
        let mutation = match self.owner {
            TextOwner::Schematic(schematic) => SchematicMutation::UpdateText {
                schematic,
                text: text.clone(),
            },
            TextOwner::Symbol(symbol) => SchematicMutation::UpdateSymbolText {
                symbol,
                text: text.clone(),
            },
        };
        let label = tr!("librepcb::editor::CmdTextEdit", "Edit Text");
        transaction(ctx.project()?, label.clone(), |e| {
            e.apply_mutations(label, vec![Mutation::Schematic(mutation)])
        })?;
        self.text = text;
        Ok(Applied::Project)
    }
}
