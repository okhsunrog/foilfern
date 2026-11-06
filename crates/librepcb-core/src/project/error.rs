//! Errors of the [`project`](super) module.
//!
//! Replaces the `RuntimeError`/`LogicError` exceptions of the upstream
//! project classes and of `ProjectLoader`. Upstream's lifecycle
//! `LogicError`s (object not added, added twice, wrong parent) have no
//! counterpart: an entity exists iff it is in its map. Data level rules keep
//! the upstream messages; those translated upstream go through [`tr!`].

use std::fmt;

use librepcb_i18n::tr;

use crate::fileio::FilePath;
use crate::types::{Uuid, Version};

/// Result type of the project module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

fn native(path: &Option<FilePath>) -> String {
    path.as_ref().map(FilePath::to_native).unwrap_or_default()
}

/// Kind of a project entity, for error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EntityKind {
    /// An assembly variant.
    AssemblyVariant,
    /// A net class.
    NetClass,
    /// A net signal.
    NetSignal,
    /// A bus.
    Bus,
    /// A component instance.
    Component,
    /// A schematic page.
    Schematic,
    /// A board.
    Board,
    /// A symbol of a schematic.
    Symbol,
    /// A net segment of a schematic or board.
    NetSegment,
    /// A bus segment of a schematic.
    BusSegment,
    /// A device of a board.
    Device,
    /// A plane of a board.
    Plane,
    /// A library element of the project library.
    LibraryElement,
    /// A junction of a schematic or board net segment (upstream
    /// `SI_NetPoint`, `BI_NetPoint`).
    NetPoint,
    /// A net line of a schematic net segment.
    NetLine,
    /// A net label of a schematic net segment.
    NetLabel,
    /// A junction of a schematic bus segment.
    BusJunction,
    /// A line of a schematic bus segment.
    BusLine,
    /// A label of a schematic bus segment.
    BusLabel,
    /// A polygon of a schematic.
    SchematicPolygon,
    /// A text of a schematic or symbol.
    SchematicText,
    /// An image of a schematic.
    SchematicImage,
    /// A standalone pad of a board net segment.
    Pad,
    /// A via of a board net segment.
    Via,
    /// A trace of a board net segment (upstream `BI_NetLine`).
    Trace,
    /// A keepout zone of a board.
    Zone,
    /// A polygon of a board.
    BoardPolygon,
    /// A stroke text of a board or device.
    StrokeText,
    /// A non-plated hole of a board.
    Hole,
}

impl fmt::Display for EntityKind {
    /// The (untranslated) noun used in upstream messages, e.g. `net class`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AssemblyVariant => "assembly variant",
            Self::NetClass => "net class",
            Self::NetSignal => "net signal",
            Self::Bus => "bus",
            Self::Component => "component",
            Self::Schematic => "schematic",
            Self::Board => "board",
            Self::Symbol => "symbol",
            Self::NetSegment => "netsegment",
            Self::BusSegment => "bus segment",
            Self::Device => "device",
            Self::Plane => "plane",
            Self::LibraryElement => "library element",
            Self::NetPoint => "netpoint",
            Self::NetLine => "netline",
            Self::NetLabel => "netlabel",
            Self::BusJunction => "bus junction",
            Self::BusLine => "bus line",
            Self::BusLabel => "bus label",
            Self::SchematicPolygon => "polygon",
            Self::SchematicText => "text",
            Self::SchematicImage => "image",
            Self::Pad => "pad",
            Self::Via => "via",
            Self::Trace => "netline",
            Self::Zone => "zone",
            Self::BoardPolygon => "polygon",
            Self::StrokeText => "stroke text",
            Self::Hole => "hole",
        })
    }
}

/// Formats the translated "There is already a ... with the name" message
/// (upstream uses one string per class, each in its own context).
fn duplicate_name(kind: EntityKind, name: &str) -> String {
    match kind {
        EntityKind::AssemblyVariant => tr!(
            "librepcb::Circuit",
            "There is already an assembly variant with the name \"{0}\"!",
            name
        ),
        EntityKind::NetClass => tr!(
            "librepcb::Circuit",
            "There is already a net class with the name \"{0}\"!",
            name
        ),
        EntityKind::NetSignal => tr!(
            "librepcb::Circuit",
            "There is already a net signal with the name \"{0}\"!",
            name
        ),
        EntityKind::Bus => tr!(
            "librepcb::Circuit",
            "There is already a bus with the name \"{0}\"!",
            name
        ),
        EntityKind::Component => tr!(
            "librepcb::Circuit",
            "There is already a component with the name \"{0}\"!",
            name
        ),
        EntityKind::Schematic => tr!(
            "librepcb::Project",
            "There is already a schematic with the name \"{0}\"!",
            name
        ),
        EntityKind::Board => tr!(
            "librepcb::Project",
            "There is already a board with the name \"{0}\"!",
            name
        ),
        _ => format!("There is already a {kind} with the name \"{name}\"!"),
    }
}

/// Error returned when loading, saving or modifying a project fails.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// DRC was cancelled before returning an applicable result.
    #[error("Design rule check cancelled.")]
    DrcCancelled,
    /// A fabrication layer is outside the supported copper stack.
    #[error("Unknown inner copper layer: {0}")]
    InvalidFabricationLayer(u32),
    /// A manufacturing generator rejected the output geometry.
    #[error(transparent)]
    Export(#[from] crate::export::Error),
    /// Invalid geometric outline.
    #[error(transparent)]
    Geometry(#[from] crate::geometry::Error),
    /// Plane construction was cancelled before applying any results.
    #[error("Plane construction cancelled.")]
    PlaneBuildCancelled,
    /// Thermal pad copper and clearance contour counts are inconsistent.
    #[error("Thermal pads inconsistency, please open a bug report.")]
    ThermalPadsInconsistent,
    /// Polygon clipping or contour flattening failed.
    #[error(transparent)]
    Clipper(#[from] crate::utils::clipper_helpers::Error),
    /// Serializing an exported JSON document failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A BOM attribute exceeds the regex engine's pattern limits.
    #[error(transparent)]
    BomPattern(#[from] regex::Error),
    /// File access failed.
    #[error(transparent)]
    FileIo(#[from] crate::fileio::Error),
    /// A file could not be parsed or deserialized.
    #[error(transparent)]
    Serialization(#[from] crate::serialization::Error),
    /// A library element of the project library could not be loaded or
    /// saved.
    #[error(transparent)]
    Library(#[from] crate::library::Error),
    /// A value is invalid.
    #[error(transparent)]
    Types(#[from] crate::types::Error),
    /// A stroke font of the project could not be loaded.
    #[error(transparent)]
    Font(#[from] crate::font::Error),

    // --- Opening ---
    /// The project file (`*.lpp`) does not exist.
    #[error("{}", tr!("librepcb::ProjectLoader", "File does not exist: '{0}'", .0.to_native()))]
    ProjectFileNotFound(FilePath),
    /// The directory contains no `.librepcb-project` file.
    #[error(
        "{}",
        tr!(
            "librepcb::ProjectLoader",
            "Directory does not contain a LibrePCB project: '{0}'",
            native(.0)
        )
    )]
    NotAProjectDirectory(Option<FilePath>),
    /// The project was written by a newer application (file format).
    #[error(
        "{}",
        tr!(
            "librepcb::ProjectLoader",
            "This project was created with a newer application version.\n\
             You need at least LibrePCB {0} to open it.\n\n{1}",
            .version.to_pretty_str(3, 10),
            .path.to_native()
        )
    )]
    NewerFileFormat {
        /// File format version of the project.
        version: Version,
        /// The project file.
        path: FilePath,
    },
    /// Upgrading the project from an older file format failed.
    #[error(transparent)]
    Migration(#[from] crate::serialization::MigrationError),
    /// The project file name does not end with `.lpp`.
    #[error("{}", tr!("librepcb::Project", "The suffix of the project file must be \"lpp\"!"))]
    InvalidProjectFileSuffix,
    /// The directory already contains a project (when creating one).
    #[error(
        "{}",
        tr!(
            "librepcb::Project",
            "The directory \"{0}\" already contains a LibrePCB project.",
            native(.0)
        )
    )]
    ProjectDirectoryNotEmpty(Option<FilePath>),
    /// The circuit file contains no assembly variant.
    #[error("Project has no assembly variants.")]
    NoAssemblyVariants,

    // --- References ---
    /// A referenced entity does not exist (upstream `LogicError` when
    /// operating on an object which is not in the circuit/project).
    #[error("There is no {kind} with the UUID \"{uuid}\".")]
    NotFound {
        /// Kind of the entity.
        kind: EntityKind,
        /// The UUID.
        uuid: Uuid,
    },
    /// A net class referenced by a net signal does not exist.
    #[error("Inexistent net class: '{0}'")]
    InexistentNetClass(Uuid),
    /// A net signal referenced by a component signal or segment does not
    /// exist.
    #[error("Inexistent net signal: '{0}'")]
    InexistentNetSignal(Uuid),
    /// A bus referenced by a bus segment does not exist.
    #[error("Inexistent bus: '{0}'")]
    InexistentBus(Uuid),
    /// A component instance references a library component which is not in
    /// the project library.
    #[error("The component '{0}' does not exist in the project's library.")]
    MissingLibraryComponent(Uuid),
    /// A component instance references a signal which the library
    /// component does not have.
    #[error("Inexistent component signal: '{0}'")]
    InexistentComponentSignal(Uuid),
    /// A component signal is listed twice in a component instance.
    #[error("The signal '{0}' is defined multiple times.")]
    DuplicateComponentSignal(Uuid),
    /// The signals of a component instance do not match its library
    /// component.
    #[error(
        "The signal count of the component instance '{component}' does not match with \
         the signal count of the component '{lib_component}'."
    )]
    SignalCountMismatch {
        /// The component instance.
        component: Uuid,
        /// The library component.
        lib_component: Uuid,
    },

    // --- Uniqueness ---
    /// An entity with the same UUID already exists.
    #[error("There is already a {kind} with the UUID \"{uuid}\"!")]
    DuplicateUuid {
        /// Kind of the entity.
        kind: EntityKind,
        /// The UUID.
        uuid: Uuid,
    },
    /// An entity with the same name already exists.
    #[error("{}", duplicate_name(*.kind, .name))]
    DuplicateName {
        /// Kind of the entity.
        kind: EntityKind,
        /// The name.
        name: String,
    },
    /// A schematic or board with the same directory name already exists.
    #[error(
        "{}",
        match .kind {
            EntityKind::Board => tr!(
                "librepcb::Project",
                "There is already a board with the directory name \"{0}\"!",
                .name
            ),
            _ => tr!(
                "librepcb::Project",
                "There is already a schematic with the directory name \"{0}\"!",
                .name
            ),
        }
    )]
    DuplicateDirectoryName {
        /// Kind of the entity (schematic or board).
        kind: EntityKind,
        /// The directory name.
        name: String,
    },
    /// A library element with the same UUID is already in the project
    /// library.
    #[error("There is already an element with the same UUID in the project's library: {0}")]
    DuplicateLibraryElement(Uuid),

    // --- Removal ---
    /// A schematic can only be removed when it is empty.
    #[error(
        "{}",
        tr!("librepcb::Project", "There are still elements in the schematic \"{0}\"!", .0)
    )]
    SchematicNotEmpty(String),
    /// The last assembly variant cannot be removed.
    #[error("The last assembly variant cannot be removed!")]
    LastAssemblyVariant,
    /// A net class still has net signals.
    #[error(
        "{}",
        tr!(
            "librepcb::NetClass",
            "The net class \"{0}\" cannot be removed because it is still in use!",
            .0
        )
    )]
    NetClassInUse(String),
    /// A net signal is still referenced (component signals, segments,
    /// planes).
    #[error(
        "{}",
        tr!(
            "librepcb::NetSignal",
            "The net signal \"{0}\" cannot be removed because it is still in use!",
            .0
        )
    )]
    NetSignalInUse(String),
    /// A bus still has bus segments.
    #[error("The bus \"{0}\" cannot be removed because it is still in use!")]
    BusInUse(String),
    /// A component instance still has symbols or devices, or its signals
    /// are still connected.
    #[error(
        "{}",
        tr!(
            "librepcb::ComponentInstance",
            "The component \"{0}\" cannot be removed because it is still in use!",
            .0
        )
    )]
    ComponentInUse(String),
    /// The net of a component signal cannot change while its pins or pads
    /// are connected.
    #[error(
        "{}",
        tr!(
            "librepcb::ComponentSignalInstance",
            "The net signal of the component signal \"{0}:{1}\" cannot be changed because it \
             is still in use!",
            .component,
            .signal
        )
    )]
    ComponentSignalInUse {
        /// Name of the component instance.
        component: String,
        /// Name of the component signal.
        signal: String,
    },

    // --- Board items (see `board/`; the generic item errors `NotFound`,
    // `DuplicateUuid`, `ItemInUse`, `DegenerateLine`,
    // `NetSegmentNotCohesive` and `InexistentNetPoint` are shared with the
    // schematic items) ---
    /// The board already has a device of the component instance.
    #[error("There is already a device with the component instance \"{0}\"!")]
    DuplicateDevice(Uuid),
    /// A device references a component instance which is not in the
    /// circuit.
    #[error("The component instance '{0}' does not exist in the circuit.")]
    InexistentDeviceComponent(Uuid),
    /// A device of a schematic-only component (upstream `LogicError` in
    /// `ComponentInstance::registerDevice()`).
    #[error("The component \"{0}\" is schematic-only and cannot be added to a board.")]
    SchematicOnlyComponent(String),
    /// A device references a library device which is not in the project
    /// library.
    #[error(
        "{}",
        tr!(
            "librepcb::BI_Device",
            "No device with the UUID \"{0}\" found in the project's library.",
            .0
        )
    )]
    MissingLibraryDevice(Uuid),
    /// A library device references a package which is not in the project
    /// library.
    #[error(
        "{}",
        tr!(
            "librepcb::BI_Device",
            "No package with the UUID \"{0}\" found in the project's library.",
            .0
        )
    )]
    MissingLibraryPackage(Uuid),
    /// The library device of a board device is not a device of the
    /// component instance's library component.
    #[error("The device \"{device}\" does not match with the componentinstance \"{component}\".")]
    DeviceComponentMismatch {
        /// The library device.
        device: Uuid,
        /// The component instance.
        component: Uuid,
    },
    /// The pad-signal map of a library device references a signal the
    /// component instance does not have.
    #[error("Unknown signal \"{signal}\" found in device \"{device}\"")]
    UnknownDeviceSignal {
        /// The component signal.
        signal: Uuid,
        /// The library device.
        device: Uuid,
    },
    /// A footprint contains a pad UUID twice.
    #[error("The footprint pad UUID \"{0}\" is defined multiple times.")]
    DuplicateFootprintPad(Uuid),
    /// A footprint pad references a package pad which is not in the
    /// package.
    #[error("Pad \"{pad}\" not found in package \"{package}\".")]
    MissingPackagePad {
        /// The package pad.
        pad: Uuid,
        /// The package.
        package: Uuid,
    },
    /// A footprint pad references a package pad which is not in the
    /// pad-signal map of the device.
    #[error("Package pad \"{pad}\" not found in pad-signal-map of device \"{device}\".")]
    PadNotInPadSignalMap {
        /// The package pad.
        pad: Uuid,
        /// The library device.
        device: Uuid,
    },
    /// A trace is not on a copper layer.
    #[error("The layer of netpoint \"{trace}\" is invalid ({layer}).")]
    InvalidTraceLayer {
        /// The trace.
        trace: Uuid,
        /// Translated name of the layer.
        layer: String,
    },
    /// A trace anchor (via, standalone pad, device, footprint pad) does not
    /// exist in the board (upstream `ProjectLoader::loadBoardNetSegment()`
    /// messages; junctions: [`Error::InexistentNetPoint`]).
    #[error("{}", inexistent_trace_anchor(.anchor, *.device_missing))]
    InexistentTraceAnchor {
        /// The anchor.
        anchor: crate::geometry::TraceAnchor,
        /// For footprint pads: whether the device is missing (not only the
        /// pad).
        device_missing: bool,
    },
    /// A trace is connected to a pad of another net.
    #[error(
        "Trace of net \"{trace_net}\" is not allowed to be connected to pad \"{pad}\" of \
         device \"{device}\" ({lib_device}) since it is connected to the net \"{pad_net}\"."
    )]
    TracePadNetMismatch {
        /// Net name of the trace's segment (empty if none).
        trace_net: String,
        /// Pad name or UUID.
        pad: String,
        /// Component instance name (empty for standalone pads).
        device: String,
        /// Library device name (empty for standalone pads).
        lib_device: String,
        /// Net name of the pad (empty if none).
        pad_net: String,
    },
    /// A trace is connected to a pad which has no copper on the trace's
    /// layer.
    #[error(
        "Trace on layer \"{layer}\" cannot be connected to the pad \"{pad}\" of device \
         \"{device}\" ({lib_device}) since it is on layer \"{pad_layer}\"."
    )]
    TraceLayerMismatch {
        /// Translated name of the trace layer.
        layer: String,
        /// Pad name or UUID.
        pad: String,
        /// Component instance name (empty for standalone pads).
        device: String,
        /// Library device name (empty for standalone pads).
        lib_device: String,
        /// Translated name of the pad's solder layer.
        pad_layer: String,
    },
    /// Traces of several net segments are connected to one footprint pad.
    #[error(
        "There are traces from multiple net segments connected to the pad \"{pad}\" of \
         device \"{device}\" ({lib_device})."
    )]
    PadInMultipleSegments {
        /// Pad name or UUID.
        pad: String,
        /// Component instance name.
        device: String,
        /// Library device name.
        lib_device: String,
    },
    /// A trace is connected to a blind or buried via on a layer the via
    /// does not span.
    #[error(
        "{}",
        tr!(
            "librepcb::BI_Via",
            "Failed to connect trace to via because it's a blind- or buried via which \
             doesn't include the corresponding layer."
        )
    )]
    TraceViaLayerMismatch(Uuid),
    /// The layers of a via cannot be changed because traces are connected
    /// on other layers.
    #[error(
        "{}",
        tr!(
            "librepcb::BI_Via",
            "Could not change the vias start/end layers because there are still traces \
             connected on other layers."
        )
    )]
    ViaLayersInUse(Uuid),
    /// The traces of a junction are on different layers (upstream
    /// `LogicError` in `BI_NetPoint::registerNetLine()`).
    #[error("NetPoint already has NetLines on different layer.")]
    JunctionLayerMismatch(Uuid),

    // --- Schematic items ---
    /// A symbol references a component instance which is not in the
    /// circuit.
    #[error("The component '{0}' does not exist in the circuit.")]
    InexistentComponent(Uuid),
    /// The symbol of a gate is not in the project library.
    #[error(
        "{}",
        tr!(
            "librepcb::SI_Symbol",
            "No symbol with the UUID \"{0}\" found in the project's library.",
            .0
        )
    )]
    MissingLibrarySymbol(Uuid),
    /// A symbol references a gate (symbol variant item) which is not in the
    /// symbol variant of its component.
    #[error("Invalid symbol item in circuit: \"{0}\".")]
    InvalidSymbolItem(Uuid),
    /// A gate of a component is already placed as a symbol.
    #[error("Symbol item UUID already exists in circuit: \"{0}\".")]
    SymbolItemAlreadyPlaced(Uuid),
    /// Symbols of one component must be placed in the same schematic.
    #[error(
        "{}",
        tr!(
            "librepcb::ComponentInstance",
            "All symbols of a component must be placed in the same schematic."
        )
    )]
    SymbolsInDifferentSchematics,
    /// The pins of the library symbol do not match the pin-signal-map of
    /// the gate.
    #[error(
        "The pin count of the symbol instance \"{0}\" does not match with the \
         pin-signal-map of its component."
    )]
    PinCountMismatch(Uuid),
    /// A pin UUID of the library symbol is used several times.
    #[error("The symbol pin UUID \"{0}\" is defined multiple times.")]
    DuplicateSymbolPin(Uuid),
    /// A net line references a junction which is not in its net segment.
    #[error("Net point '{0}' does not exist in schematic.")]
    InexistentNetPoint(Uuid),
    /// A net line references a bus segment which is not in the schematic.
    #[error("Bus segment '{0}' does not exist in schematic.")]
    InexistentBusSegment(Uuid),
    /// A net or bus line references a bus junction which does not exist.
    #[error("Bus junction '{segment}:{junction}' does not exist in schematic.")]
    InexistentBusJunction {
        /// The bus segment.
        segment: Uuid,
        /// The junction.
        junction: Uuid,
    },
    /// A net line references a symbol which is not in the schematic.
    #[error("Symbol '{0}' does not exist in schematic.")]
    InexistentSymbol(Uuid),
    /// A net line references a symbol pin which does not exist (or is not
    /// connected to a component signal).
    #[error("Symbol pin '{symbol}:{pin}' does not exist in schematic.")]
    InexistentSymbolPin {
        /// The symbol.
        symbol: Uuid,
        /// The library pin.
        pin: Uuid,
    },
    /// A line is connected to a pin whose component signal is connected to
    /// another net.
    #[error(
        "Line of net \"{net}\" is not allowed to be connected to pin \"{signal}\" of \
         component \"{component}\" ({lib_component}) since it is connected to the net \
         \"{pin_net}\"."
    )]
    AnchorNetMismatch {
        /// Name of the net of the line.
        net: String,
        /// Name of the component signal of the pin.
        signal: String,
        /// Name of the component instance.
        component: String,
        /// Name of the library component.
        lib_component: String,
        /// Name of the net of the component signal (empty if none).
        pin_net: String,
    },
    /// Lines of several net segments are connected to one pin.
    #[error(
        "There are lines from multiple net segments connected to the pin \"{signal}\" of \
         component \"{component}\" ({lib_component})."
    )]
    AnchorInMultipleSegments {
        /// Name of the component signal of the pin.
        signal: String,
        /// Name of the component instance.
        component: String,
        /// Name of the library component.
        lib_component: String,
    },
    /// Both endpoints of a line are the same.
    #[error(
        "{}",
        match .kind {
            EntityKind::BusLine => "SI_BusLine: both endpoints are the same.",
            EntityKind::Trace => "BI_NetLine: both endpoints are the same.",
            _ => "SI_NetLine: both endpoints are the same.",
        }
    )]
    DegenerateLine {
        /// Kind of the line.
        kind: EntityKind,
        /// The line.
        uuid: Uuid,
    },
    /// The junctions and lines of a net segment are not all connected.
    #[error(
        "{}",
        if *.after_removal {
            format!("The netsegment with the UUID \"{uuid}\" is not cohesive!")
        } else {
            format!(
                "The netsegment with the UUID \"{uuid}\" is not cohesive! If this error \
                 occurs after opening an existing project with a newer LibrePCB version, \
                 please contact us."
            )
        }
    )]
    NetSegmentNotCohesive {
        /// The net segment.
        uuid: Uuid,
        /// Whether elements were removed (upstream uses another message).
        after_removal: bool,
    },
    /// The junctions and lines of a bus segment are not all connected.
    #[error("The bus segment with the UUID \"{0}\" is not cohesive!")]
    BusSegmentNotCohesive(Uuid),
    /// An item cannot be removed or re-targeted because other items are
    /// connected to it (upstream `LogicError`: a wired symbol pin, a
    /// junction with lines, a non-empty segment whose net/bus changes).
    #[error("The {kind} \"{uuid}\" cannot be modified because it is still in use!")]
    ItemInUse {
        /// Kind of the item.
        kind: EntityKind,
        /// The item.
        uuid: Uuid,
    },
}

/// Message of [`Error::InexistentTraceAnchor`] (upstream loader messages,
/// including "schematic" for junctions).
fn inexistent_trace_anchor(anchor: &crate::geometry::TraceAnchor, device_missing: bool) -> String {
    use crate::geometry::TraceAnchor;
    match anchor {
        TraceAnchor::Junction(uuid) => format!("Net point '{uuid}' does not exist in schematic."),
        TraceAnchor::Via(uuid) => format!("Via '{uuid}' does not exist in board."),
        TraceAnchor::Pad(uuid) => format!("Pad '{uuid}' does not exist in board."),
        TraceAnchor::FootprintPad { device, .. } if device_missing => {
            format!("Device instance '{device}' does not exist in board.")
        }
        TraceAnchor::FootprintPad { device, pad } => {
            format!("Footprint pad '{device}:{pad}' does not exist in board.")
        }
    }
}
