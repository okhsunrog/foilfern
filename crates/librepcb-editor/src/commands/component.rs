//! Component, symbol and device commands: port of
//! libs/librepcb/editor/project/cmd/cmdaddcomponenttocircuit,
//! cmdcomponentinstanceadd, cmdcomponentinstanceedit,
//! cmdaddsymboltoschematic, cmdsymbolinstanceeditall,
//! cmdadddevicetoboard, cmddeviceinstanceeditall and cmdreplacedevice
//! (`.{h,cpp}`), plus the gate by gate placement of
//! `SchematicEditorState_AddComponent` and the component removal of
//! `CmdRemoveSelectedSchematicItems`.

use std::collections::BTreeSet;

use librepcb_core::attribute::AttributeList;
use librepcb_core::geometry::{Junction, Trace, TraceAnchor};
use librepcb_core::library::pkg::AssemblyType;
use librepcb_core::project::board::{BoardDevice, BoardSegmentElements};
use librepcb_core::project::circuit::{
    ComponentAssemblyOption, ComponentAssemblyOptionList, ComponentInstance,
};
use librepcb_core::project::schematic::SchematicSymbol;
use librepcb_core::project::{
    AssemblyVariantId, BoardId, BoardMutation, BoardNetSegmentRef, ComponentInstanceId,
    LibraryElementKind, Mutation, SchematicId, SchematicMutation, SymbolId, SymbolRef,
};
use librepcb_core::types::{Angle, CircuitIdentifier, Layer, Length, Orientation, Point, Uuid};
use librepcb_i18n::tr;

use super::board::{BoardSelection, remove_board_items};
use super::circuit::remove_unused_nets;
use super::library::{
    LibraryElementId, ensure_element, open_element, remove_unused_library_elements,
};
use super::schematic::{SchematicSelection, remove_component_devices, remove_schematic_items};
use super::{ComponentRef, resolve};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};
use crate::undo_stack::LibraryElement;

/// Adds an element from the source if missing and records it.
fn ensure(
    tx: &mut Transaction<'_>,
    kind: LibraryElementKind,
    uuid: Uuid,
    added: &mut Vec<LibraryElementId>,
) -> Result<()> {
    if ensure_element(tx, kind, uuid)? {
        added.push(LibraryElementId { kind, uuid });
    }
    Ok(())
}

/// Returns the resolved assembly type of the package of a library device,
/// from the project library or (read-only) from the library element
/// source.
fn device_package_assembly_type(tx: &Transaction<'_>, device: Uuid) -> Result<AssemblyType> {
    let lib = tx.project().library();
    let package = match lib.device(&device) {
        Some(d) => d.package_uuid(),
        None => {
            let dir = tx
                .source()
                .element_directory(LibraryElementKind::Device, &device)
                .ok_or(Error::NotInLibrarySource {
                    kind: LibraryElementKind::Device,
                    uuid: device,
                })?;
            match open_element(LibraryElementKind::Device, &dir)? {
                LibraryElement::Device(d) => d.package_uuid(),
                _ => unreachable!("opened as device"),
            }
        }
    };
    if let Some(p) = lib.package(&package) {
        return Ok(p.resolved_assembly_type());
    }
    let dir = tx
        .source()
        .element_directory(LibraryElementKind::Package, &package)
        .ok_or(Error::NotInLibrarySource {
            kind: LibraryElementKind::Package,
            uuid: package,
        })?;
    match open_element(LibraryElementKind::Package, &dir)? {
        LibraryElement::Package(p) => Ok(p.resolved_assembly_type()),
        _ => unreachable!("opened as package"),
    }
}

/// Returns the attributes of a library device (project library or source).
fn device_attributes(tx: &Transaction<'_>, device: Uuid) -> Result<AttributeList> {
    if let Some(d) = tx.project().library().device(&device) {
        return Ok(d.attributes().clone());
    }
    let dir = tx
        .source()
        .element_directory(LibraryElementKind::Device, &device)
        .ok_or(Error::NotInLibrarySource {
            kind: LibraryElementKind::Device,
            uuid: device,
        })?;
    match open_element(LibraryElementKind::Device, &dir)? {
        LibraryElement::Device(d) => Ok(d.attributes().clone()),
        _ => unreachable!("opened as device"),
    }
}

/// Returns the assembly option for a device (upstream
/// `SchematicEditorState_AddComponent::startAddingComponent()` and
/// `CmdAddDeviceToBoard`): the device's attributes, all assembly variants
/// if the package requires assembly, no parts.
fn assembly_option(tx: &Transaction<'_>, device: Uuid) -> Result<ComponentAssemblyOption> {
    let variants: BTreeSet<AssemblyVariantId> =
        if device_package_assembly_type(tx, device)? != AssemblyType::None {
            tx.project()
                .circuit()
                .assembly_variants()
                .uuids()
                .into_iter()
                .map(AssemblyVariantId)
                .collect()
        } else {
            BTreeSet::new()
        };
    Ok(ComponentAssemblyOption::new(
        device,
        device_attributes(tx, device)?,
        variants,
        Default::default(),
    ))
}

/// Placement of the symbols of a new component.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SymbolPlacement {
    /// The schematic (default: the first one).
    #[serde(default)]
    pub schematic: Option<SchematicId>,
    /// Position of the first gate.
    pub position: Point,
    /// Rotation of all gates.
    #[serde(default)]
    pub rotation: Angle,
    /// Whether the gates are mirrored.
    #[serde(default)]
    pub mirrored: bool,
    /// Offset between consecutive gates (default: 20.32 mm downwards).
    #[serde(default)]
    pub gate_offset: Option<Point>,
}

/// Adds a component instance to the circuit (upstream
/// `CmdAddComponentToCircuit` + `CmdComponentInstanceAdd`): the library
/// component is copied into the project library if missing; the instance
/// gets an automatic name from the component prefix (by norm order), the
/// default value and attributes, and all signals unconnected. Optionally
/// an assembly option for a device is added (like the "Add Component"
/// dialog) and all gates are placed on a schematic (like
/// `SchematicEditorState_AddComponent`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddComponent {
    /// UUID of the library component.
    pub component: Uuid,
    /// UUID of the symbol variant (default: by the project's norm order,
    /// else the first one).
    #[serde(default)]
    pub symbol_variant: Option<Uuid>,
    /// Name (default: automatic, e.g. "R1").
    #[serde(default)]
    pub name: Option<CircuitIdentifier>,
    /// Value (default: the component's default value).
    #[serde(default)]
    pub value: Option<String>,
    /// Library device for an assembly option (default: none).
    #[serde(default)]
    pub device: Option<Uuid>,
    /// Where to place the symbols (default: not placed).
    #[serde(default)]
    pub place: Option<SymbolPlacement>,
}

impl AddComponent {
    /// Creates the command for a library component with all defaults.
    pub fn new(component: Uuid) -> Self {
        Self {
            component,
            symbol_variant: None,
            name: None,
            value: None,
            device: None,
            place: None,
        }
    }
}

/// Result of [`AddComponent`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddedComponent {
    /// The new component instance.
    pub component: ComponentInstanceId,
    /// Its name.
    pub name: String,
    /// The placed symbols (in gate order).
    pub symbols: Vec<SymbolId>,
    /// Library elements copied into the project library.
    pub library_elements: Vec<LibraryElementId>,
}

impl Command for AddComponent {
    type Output = AddedComponent;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdAddComponentToCircuit",
            "Add component"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<AddedComponent> {
        let mut added = Vec::new();
        ensure(
            tx,
            LibraryElementKind::Component,
            self.component,
            &mut added,
        )?;
        let mut options = ComponentAssemblyOptionList::new();
        if let Some(device) = self.device {
            options.push(assembly_option(tx, device)?);
        }
        let p = tx.project();
        let lib_component =
            p.library()
                .component(&self.component)
                .ok_or(Error::NotInProjectLibrary {
                    kind: LibraryElementKind::Component,
                    uuid: self.component,
                })?;
        let norm_order = &p.settings().norm_order;
        let variant = match self.symbol_variant {
            Some(v) => v,
            None => {
                let index = lib_component
                    .symbol_variant_index_by_norm(norm_order)
                    .unwrap_or(0);
                lib_component
                    .symbol_variants()
                    .get(index)
                    .ok_or(Error::ComponentWithoutSymbols(self.component))?
                    .uuid()
            }
        };
        let name = match self.name {
            Some(name) => name,
            None => CircuitIdentifier::new(p.circuit().generate_auto_component_instance_name(
                lib_component.prefixes().value(norm_order).as_str(),
            ))?,
        };
        let mut instance = ComponentInstance::new(
            Uuid::new_random(),
            lib_component,
            variant,
            name,
            p.settings().default_lock_component_assembly,
        )?;
        instance.set_assembly_options(options);
        if let Some(value) = self.value {
            instance.set_value(value);
        }
        let id = instance.id();
        let name = instance.name().to_string();
        tx.apply(Mutation::AddComponentInstance(instance))?;

        let mut symbols = Vec::new();
        if let Some(place) = self.place {
            let gates: Vec<Uuid> = tx
                .project()
                .library()
                .component(&self.component)
                .and_then(|c| c.symbol_variants().by_uuid(&variant))
                .map(|v| v.symbol_items().uuids())
                .unwrap_or_default();
            if gates.is_empty() {
                return Err(Error::ComponentWithoutSymbols(self.component));
            }
            let offset = place
                .gate_offset
                .unwrap_or(Point::new(Length::ZERO, Length::new(-20_320_000)));
            let mut position = place.position;
            for gate in gates {
                let (symbol, lib) = place_symbol(
                    tx,
                    id,
                    Some(gate),
                    place.schematic,
                    position,
                    place.rotation,
                    place.mirrored,
                )?;
                added.extend(lib);
                symbols.push(symbol);
                position += offset;
            }
        }
        Ok(AddedComponent {
            component: id,
            name,
            symbols,
            library_elements: added,
        })
    }
}

/// Places a gate of a component (upstream `CmdAddSymbolToSchematic`).
fn place_symbol(
    tx: &mut Transaction<'_>,
    component: ComponentInstanceId,
    gate: Option<Uuid>,
    schematic: Option<SchematicId>,
    position: Point,
    rotation: Angle,
    mirrored: bool,
) -> Result<(SymbolId, Vec<LibraryElementId>)> {
    let p = tx.project();
    let instance = p
        .circuit()
        .component_instance(component)
        .ok_or_else(|| Error::not_found("Component", component))?;
    let lib_component =
        p.library()
            .component(&instance.lib_component())
            .ok_or(Error::NotInProjectLibrary {
                kind: LibraryElementKind::Component,
                uuid: instance.lib_component(),
            })?;
    let variant = lib_component
        .symbol_variants()
        .required_by_uuid(&instance.lib_variant())
        .map_err(librepcb_core::project::Error::from)?;
    let uses = p.component_uses(component).cloned().unwrap_or_default();
    // The next unplaced gate (upstream `startAddingNextGate()`).
    let item = match gate {
        Some(gate) => variant
            .symbol_items()
            .by_uuid(&gate)
            .ok_or_else(|| Error::not_found("Gate", gate))?,
        None => variant
            .symbol_items()
            .iter()
            .find(|i| !uses.symbols.contains_key(&i.uuid()))
            .ok_or_else(|| Error::AllGatesPlaced(instance.name().to_string()))?,
    };
    let gate = item.uuid();
    let symbol_uuid = item.symbol_uuid();
    // Same schematic as the other gates (the model requires it).
    let schematic = match (schematic, uses.symbols.values().next()) {
        (Some(s), _) => s,
        (None, Some((s, _))) => *s,
        (None, None) => resolve::schematic(p, None)?.id(),
    };
    let mut added = Vec::new();
    ensure(tx, LibraryElementKind::Symbol, symbol_uuid, &mut added)?;
    let mut symbol = SchematicSymbol::new(
        Uuid::new_random(),
        component,
        gate,
        position,
        rotation,
        mirrored,
    );
    let lib_symbol =
        tx.project()
            .library()
            .symbol(&symbol_uuid)
            .ok_or(Error::NotInProjectLibrary {
                kind: LibraryElementKind::Symbol,
                uuid: symbol_uuid,
            })?;
    for text in symbol.default_texts(lib_symbol) {
        symbol.insert_text(text);
    }
    let id = symbol.id();
    tx.apply(Mutation::Schematic(SchematicMutation::AddSymbol {
        schematic,
        symbol,
    }))?;
    Ok((id, added))
}

/// Places a gate of a component on a schematic (upstream
/// `CmdAddSymbolToSchematic`): the library symbol is copied into the
/// project library if missing, the symbol gets the texts of the library
/// symbol.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlaceSymbol {
    /// The component.
    pub component: ComponentRef,
    /// The gate (symbol variant item; default: the first unplaced one).
    #[serde(default)]
    pub gate: Option<Uuid>,
    /// The schematic (default: the one of the other gates, else the first).
    #[serde(default)]
    pub schematic: Option<SchematicId>,
    /// The position.
    pub position: Point,
    /// The rotation.
    #[serde(default)]
    pub rotation: Angle,
    /// Whether the symbol is mirrored.
    #[serde(default)]
    pub mirrored: bool,
}

/// Result of [`PlaceSymbol`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlacedSymbol {
    /// The new symbol.
    pub symbol: SymbolId,
    /// Library elements copied into the project library.
    pub library_elements: Vec<LibraryElementId>,
}

impl Command for PlaceSymbol {
    type Output = PlacedSymbol;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdAddSymbolToSchematic", "Add symbol")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<PlacedSymbol> {
        let component = resolve::component(tx.project(), &self.component)?;
        let (symbol, library_elements) = place_symbol(
            tx,
            component,
            self.gate,
            self.schematic,
            self.position,
            self.rotation,
            self.mirrored,
        )?;
        Ok(PlacedSymbol {
            symbol,
            library_elements,
        })
    }
}

/// Moves, rotates and/or mirrors a symbol together with its texts
/// (upstream `CmdSymbolInstanceEditAll`); `None` fields are kept. Wires
/// stay attached to the pins.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MoveSymbol {
    /// The symbol.
    pub symbol: SymbolId,
    /// The new position.
    #[serde(default)]
    pub position: Option<Point>,
    /// The new rotation.
    #[serde(default)]
    pub rotation: Option<Angle>,
    /// The new mirror state.
    #[serde(default)]
    pub mirrored: Option<bool>,
}

impl Command for MoveSymbol {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdSymbolInstanceEditAll", "Drag Symbol")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let schematic = super::schematic::symbol_schematic(tx.project(), self.symbol)?;
        let symbol = tx
            .project()
            .schematic(schematic)
            .and_then(|s| s.symbols().get(&self.symbol))
            .ok_or_else(|| Error::not_found("Symbol", self.symbol))?
            .clone();
        let r = SymbolRef {
            schematic,
            symbol: self.symbol,
        };
        let mut position = symbol.position();
        let mut rotation = symbol.rotation();
        let mut mirrored = symbol.mirrored();
        let mut texts: Vec<_> = symbol.texts().values().cloned().collect();
        // upstream `setPosition()` / `translate()`
        if let Some(new) = self.position {
            let delta = new - position;
            for text in &mut texts {
                text.set_position(text.position() + delta);
            }
            position = new;
        }
        // upstream `setRotation()`: rotate the texts around the position
        if let Some(new) = self.rotation {
            let delta = new - rotation;
            for text in &mut texts {
                text.set_position(text.position().rotated(delta, position));
                text.set_rotation(text.rotation() + delta);
            }
            rotation = new;
        }
        // upstream `setMirrored()` → `CmdTextEdit::mirror(rotation, center)`
        if let Some(new) = self.mirrored
            && new != mirrored
        {
            for text in &mut texts {
                text.set_position(
                    text.position()
                        .rotated(-rotation, position)
                        .mirrored(Orientation::Horizontal, position)
                        .rotated(rotation, position),
                );
                text.set_align(text.align().mirrored_h());
            }
            mirrored = new;
        }
        tx.apply(Mutation::Schematic(SchematicMutation::SetSymbolPlacement {
            symbol: r,
            position,
            rotation,
            mirrored,
        }))?;
        for text in texts {
            if symbol.texts().get(&text.uuid()) != Some(&text) {
                tx.apply(Mutation::Schematic(SchematicMutation::UpdateSymbolText {
                    symbol: r,
                    text,
                }))?;
            }
        }
        Ok(())
    }
}

/// Modifies a component instance (upstream `CmdComponentInstanceEdit`);
/// `None` fields are kept.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditComponent {
    /// The component.
    pub component: ComponentRef,
    /// The new name.
    #[serde(default)]
    pub name: Option<CircuitIdentifier>,
    /// The new value.
    #[serde(default)]
    pub value: Option<String>,
    /// The new attributes.
    #[serde(default)]
    pub attributes: Option<AttributeList>,
    /// The new assembly options.
    #[serde(default)]
    pub assembly_options: Option<ComponentAssemblyOptionList>,
    /// The new assembly lock.
    #[serde(default)]
    pub lock_assembly: Option<bool>,
}

impl Command for EditComponent {
    type Output = ComponentInstanceId;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdComponentInstanceEdit",
            "Edit Component"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<ComponentInstanceId> {
        let id = resolve::component(tx.project(), &self.component)?;
        let mut properties = tx
            .project()
            .circuit()
            .component_instance(id)
            .ok_or_else(|| Error::not_found("Component", id))?
            .properties();
        if let Some(name) = self.name {
            properties.name = name;
        }
        if let Some(value) = self.value {
            properties.value = value;
        }
        if let Some(attributes) = self.attributes {
            properties.attributes = attributes;
        }
        if let Some(options) = self.assembly_options {
            properties.assembly_options = options;
        }
        if let Some(lock) = self.lock_assembly {
            properties.lock_assembly = lock;
        }
        tx.apply(Mutation::UpdateComponentInstance(properties))?;
        Ok(id)
    }
}

/// Removes a component: all its symbols with their wires (like upstream
/// `CmdRemoveSelectedSchematicItems` with all symbols of the component
/// selected), its devices on all boards (traces at their pads end in
/// junctions, upstream `CmdRemoveBoardItems`), the component instance, and
/// then unused nets and library elements.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveComponent {
    /// The component.
    pub component: ComponentRef,
}

impl Command for RemoveComponent {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdComponentInstanceRemove",
            "Remove component"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let id = resolve::component(tx.project(), &self.component)?;
        let symbols: Vec<(SchematicId, SymbolId)> = tx
            .project()
            .component_uses(id)
            .map(|u| u.symbols.values().copied().collect())
            .unwrap_or_default();
        if let Some((schematic, _)) = symbols.first() {
            let selection = SchematicSelection {
                symbols: symbols.iter().map(|(_, s)| *s).collect(),
                ..SchematicSelection::default()
            };
            remove_schematic_items(tx, *schematic, &selection)?;
        } else {
            remove_component_devices(tx, id)?;
            tx.apply(Mutation::RemoveComponentInstance(id))?;
            remove_unused_nets(tx)?;
            remove_unused_library_elements(tx)?;
        }
        Ok(())
    }
}

/// Adds a device (upstream `CmdAddDeviceToBoard`).
#[allow(clippy::too_many_arguments)]
fn add_device(
    tx: &mut Transaction<'_>,
    board: BoardId,
    component: ComponentInstanceId,
    device: Uuid,
    footprint: Option<Uuid>,
    preferred_model: Option<Uuid>,
    position: Point,
    rotation: Angle,
    mirrored: bool,
) -> Result<AddedDevice> {
    let mut added = Vec::new();
    ensure(tx, LibraryElementKind::Device, device, &mut added)?;
    let package_uuid = tx
        .project()
        .library()
        .device(&device)
        .ok_or(Error::NotInProjectLibrary {
            kind: LibraryElementKind::Device,
            uuid: device,
        })?
        .package_uuid();
    ensure(tx, LibraryElementKind::Package, package_uuid, &mut added)?;
    let p = tx.project();
    let lib_device = p
        .library()
        .device(&device)
        .ok_or(Error::NotInProjectLibrary {
            kind: LibraryElementKind::Device,
            uuid: device,
        })?;
    let package = p
        .library()
        .package(&package_uuid)
        .ok_or(Error::NotInProjectLibrary {
            kind: LibraryElementKind::Package,
            uuid: package_uuid,
        })?;
    let footprint = match footprint {
        Some(f) => f,
        None => package
            .footprints()
            .first()
            .ok_or(Error::NoFootprints(package_uuid))?
            .uuid(),
    };
    let mut dev = BoardDevice::from_library(
        component, lib_device, package, footprint, position, rotation, mirrored, false, true, true,
    )?;
    // Assign the 3D model: the preferred one if valid, else the default.
    let model = preferred_model
        .filter(|m| {
            package.models().contains_uuid(m)
                && package
                    .footprints()
                    .by_uuid(&footprint)
                    .is_some_and(|f| f.models().contains(m))
        })
        .or_else(|| dev.default_lib_model(package));
    dev.set_lib_model(model);
    // Make sure there is an assembly option for this device.
    let instance = p
        .circuit()
        .component_instance(component)
        .ok_or_else(|| Error::not_found("Component", component))?;
    if !instance.compatible_devices().contains(&device) {
        if instance.lock_assembly() {
            return Err(Error::DeviceNotCompatible);
        }
        let mut properties = instance.properties();
        let option = assembly_option(tx, device)?;
        properties.assembly_options.push(option);
        tx.apply(Mutation::UpdateComponentInstance(properties))?;
    }
    tx.apply(Mutation::Board(BoardMutation::AddDevice {
        board,
        device: dev,
    }))?;
    Ok(AddedDevice {
        board,
        component,
        device,
        footprint,
        library_elements: added,
    })
}

/// Adds the device of a component to a board (upstream
/// `CmdAddDeviceToBoard`): device and package are copied into the project
/// library if missing, the device gets the attributes and stroke texts of
/// the library, the default 3D model, and an assembly option is added to
/// the component if the device is not a compatible device yet.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddDevice {
    /// The component.
    pub component: ComponentRef,
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// UUID of the library device (default: the first device of the
    /// component's assembly options).
    #[serde(default)]
    pub device: Option<Uuid>,
    /// UUID of the footprint (default: the first footprint of the
    /// package).
    #[serde(default)]
    pub footprint: Option<Uuid>,
    /// The position.
    pub position: Point,
    /// The rotation.
    #[serde(default)]
    pub rotation: Angle,
    /// Whether the device is mirrored (bottom side).
    #[serde(default)]
    pub mirrored: bool,
}

/// Result of [`AddDevice`] and [`ReplaceDevice`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddedDevice {
    /// The board.
    pub board: BoardId,
    /// The component (identifies the device on the board).
    pub component: ComponentInstanceId,
    /// UUID of the library device.
    pub device: Uuid,
    /// UUID of the footprint.
    pub footprint: Uuid,
    /// Library elements copied into the project library.
    pub library_elements: Vec<LibraryElementId>,
}

impl Command for AddDevice {
    type Output = AddedDevice;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdAddDeviceToBoard",
            "Add device to board"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<AddedDevice> {
        let component = resolve::component(tx.project(), &self.component)?;
        let board = resolve::board(tx.project(), self.board)?.id();
        let device = match self.device {
            Some(d) => d,
            None => tx
                .project()
                .circuit()
                .component_instance(component)
                .and_then(|c| c.assembly_options().iter().next().map(|o| o.device()))
                .ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "No device specified and the component \"{}\" has no assembly options.",
                        resolve::component_name(tx.project(), component)
                    ))
                })?,
        };
        add_device(
            tx,
            board,
            component,
            device,
            self.footprint,
            None,
            self.position,
            self.rotation,
            self.mirrored,
        )
    }
}

/// Replaces the device of a component on a board by another device and/or
/// footprint at the same placement (upstream `CmdReplaceDevice`): traces
/// at its pads are removed (several traces at a pad stay connected through
/// a new junction), an unused assembly option without parts of the old
/// device is removed (unless locked), then the new device is added.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplaceDevice {
    /// The component.
    pub component: ComponentRef,
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// UUID of the new library device.
    pub device: Uuid,
    /// UUID of the new footprint (default: the first one).
    #[serde(default)]
    pub footprint: Option<Uuid>,
}

impl Command for ReplaceDevice {
    type Output = AddedDevice;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdReplaceDevice", "Change Device")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<AddedDevice> {
        let component = resolve::component(tx.project(), &self.component)?;
        let p = tx.project();
        let board = resolve::board(p, self.board)?;
        let board_id = board.id();
        let old = board
            .device(component)
            .ok_or_else(|| Error::not_found("Device", component))?
            .clone();
        // Remove all connected traces.
        let mut steps = Vec::new();
        let mut selection = BoardSelection::default();
        for pad in old.pads(p.library(), p.circuit())? {
            let Some(segment_id) = board.footprint_pad_segment(component, pad.uuid()) else {
                continue;
            };
            let Some(segment) = board.net_segment(segment_id) else {
                continue;
            };
            let anchor = pad.trace_anchor();
            let traces: Vec<&Trace> = segment.traces_at(anchor).collect();
            let mut elements = BoardSegmentElements::default();
            if traces.len() > 1 {
                let mut junctions: std::collections::BTreeMap<Layer, TraceAnchor> =
                    Default::default();
                for trace in &traces {
                    let j = *junctions.entry(trace.layer()).or_insert_with(|| {
                        let junction = Junction::new(Uuid::new_random(), pad.position());
                        let a = TraceAnchor::Junction(junction.uuid());
                        elements.junctions.push(junction);
                        a
                    });
                    let other = if trace.p1() == anchor {
                        trace.p2()
                    } else {
                        trace.p1()
                    };
                    elements.traces.push(Trace::new(
                        Uuid::new_random(),
                        trace.layer(),
                        trace.width(),
                        j,
                        other,
                    ));
                }
            }
            if !elements.is_empty() {
                steps.push(Mutation::Board(BoardMutation::AddNetSegmentElements {
                    segment: BoardNetSegmentRef {
                        board: board_id,
                        segment: segment_id,
                    },
                    elements,
                }));
            }
            for trace in traces {
                selection.traces.push((segment_id, trace.uuid()));
            }
        }
        tx.apply_all(steps)?;
        remove_board_items(tx, board_id, &selection, false)?;
        // Remove the device.
        tx.apply(Mutation::Board(BoardMutation::RemoveDevice(
            librepcb_core::project::DeviceRef {
                board: board_id,
                component,
            },
        )))?;
        // Remove the assembly option if it is no longer used.
        let p = tx.project();
        let instance = p
            .circuit()
            .component_instance(component)
            .ok_or_else(|| Error::not_found("Component", component))?;
        if !instance.lock_assembly() {
            let used_elsewhere = p
                .boards()
                .iter()
                .filter_map(|b| b.device(component))
                .any(|d| d.lib_device() == old.lib_device());
            if !used_elsewhere {
                let mut properties = instance.properties();
                let mut options = ComponentAssemblyOptionList::new();
                for option in properties.assembly_options.iter() {
                    if !(option.device() == old.lib_device() && option.parts().is_empty()) {
                        options.push(option.clone());
                    }
                }
                if options != properties.assembly_options {
                    properties.assembly_options = options;
                    tx.apply(Mutation::UpdateComponentInstance(properties))?;
                }
            }
        }
        // Add the new device.
        add_device(
            tx,
            board_id,
            component,
            self.device,
            self.footprint,
            old.lib_model(),
            old.position(),
            old.rotation(),
            old.mirrored(),
        )
    }
}

/// Moves, rotates, mirrors and/or locks a device together with its stroke
/// texts (upstream `CmdDeviceInstanceEditAll`); `None` fields are kept.
/// Traces stay attached to the pads; mirroring a device with traces is
/// rejected by the model.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MoveDevice {
    /// The component.
    pub component: ComponentRef,
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The new position.
    #[serde(default)]
    pub position: Option<Point>,
    /// The new rotation.
    #[serde(default)]
    pub rotation: Option<Angle>,
    /// The new mirror state (bottom side).
    #[serde(default)]
    pub mirrored: Option<bool>,
    /// The new lock state.
    #[serde(default)]
    pub locked: Option<bool>,
}

impl Command for MoveDevice {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdDeviceInstanceEditAll",
            "Edit device instance"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let component = resolve::component(tx.project(), &self.component)?;
        let board = resolve::board(tx.project(), self.board)?;
        let board_id = board.id();
        let inner_layers = board.settings().inner_layer_count as usize;
        let mut dev = board
            .device(component)
            .ok_or_else(|| Error::not_found("Device", component))?
            .clone();
        let mut texts: Vec<_> = dev.stroke_texts().values().cloned().collect();
        if let Some(new) = self.position {
            let delta = new - dev.position();
            for text in &mut texts {
                text.set_position(text.position() + delta);
            }
            dev.set_position(new);
        }
        if let Some(new) = self.rotation {
            let delta = new - dev.rotation();
            let center = dev.position();
            for text in &mut texts {
                text.set_position(text.position().rotated(delta, center));
                text.set_rotation(text.rotation() + delta);
            }
            dev.set_rotation(new);
        }
        if let Some(new) = self.mirrored
            && new != dev.mirrored()
        {
            let (rotation, center) = (dev.rotation(), dev.position());
            for text in &mut texts {
                // upstream `CmdBoardStrokeTextEdit::mirrorGeometry(rotation, center)`
                text.set_position(
                    text.position()
                        .rotated(-rotation, center)
                        .mirrored(Orientation::Horizontal, center)
                        .rotated(rotation, center),
                );
                text.set_rotation(rotation + Angle::DEG180 - text.rotation() + rotation);
                text.set_align(text.align().mirrored_v());
                // upstream `mirrorLayer()`
                text.set_layer(text.layer().mirrored(Some(inner_layers)));
                text.set_mirrored(!text.mirrored());
                text.set_align(text.align().mirrored_h());
            }
            dev.set_mirrored(new);
        }
        if let Some(locked) = self.locked {
            dev.set_locked(locked);
            for text in &mut texts {
                text.set_locked(locked);
            }
        }
        for text in texts {
            dev.insert_stroke_text(text);
        }
        tx.apply(Mutation::Board(BoardMutation::UpdateDevice {
            board: board_id,
            device: dev,
        }))
    }
}
