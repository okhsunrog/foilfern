//! Tests of the board part of the project model: the ported upstream tests
//! of `core/project/board` (design rules, fabrication output settings) and
//! the board mutations, serialization, air wires and the net segment
//! splitter (no upstream counterpart).

mod air_wires_test;
mod d356_test;
mod design_rules_test;
mod fabrication_output_settings_test;
mod io_test;
mod mutation_test;
mod net_segment_splitter_test;
mod pick_place_test;
mod serde_test;
mod via_test;

use chrono::Utc;
use librepcb_core::geometry::{
    ComponentSide, Junction, NonEmptyPath, Pad, PadFunction, PadHole, PadHoleList, PadShape, Path,
    Trace, TraceAnchor,
};
use librepcb_core::library::BaseMetadata;
use librepcb_core::library::dev::{Device, DevicePadSignalMapItem};
use librepcb_core::library::pkg::{AssemblyType, Footprint, FootprintPad, Package, PackagePad};
use librepcb_core::project::board::{Board, BoardDevice, BoardNetSegment, BoardPadData};
use librepcb_core::project::{BoardId, ComponentInstanceId, NetSignalId, Project};
use librepcb_core::types::{
    Angle, CircuitIdentifier, ElementName, Layer, Length, MaskConfig, Point, PositiveLength, Ratio,
    UnsignedLength, UnsignedLimitedRatio, Uuid, Version,
};

use super::{add_net, component_instance, library_component, new_project};

/// Millimeters to a point.
pub fn pt(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).unwrap()
}

/// Millimeters to a positive length.
pub fn pos_mm(mm: f64) -> PositiveLength {
    PositiveLength::new(Length::from_mm(mm).unwrap()).unwrap()
}

fn metadata(uuid: Uuid, name: &str) -> BaseMetadata {
    BaseMetadata::new(
        uuid,
        "0.1".parse::<Version>().unwrap(),
        "test",
        Utc::now(),
        ElementName::new(name).unwrap(),
        "",
        "",
    )
}

/// Creates a pad (1x1mm, round-ish rectangle) at `pos`; THT if `hole` is
/// set.
pub fn pad(uuid: Uuid, pos: Point, side: ComponentSide, hole: bool) -> Pad {
    let holes = if hole {
        PadHoleList::from(vec![PadHole::new(
            Uuid::new_random(),
            pos_mm(0.5),
            NonEmptyPath::from_point(Point::ORIGIN),
        )])
    } else {
        PadHoleList::new()
    };
    Pad::new(
        uuid,
        pos,
        Angle::DEG0,
        PadShape::RoundedRect,
        pos_mm(1.0),
        pos_mm(1.0),
        UnsignedLimitedRatio::new(Ratio::ZERO).unwrap(),
        Path::default(),
        MaskConfig::Automatic,
        MaskConfig::Automatic,
        UnsignedLength::ZERO,
        side,
        PadFunction::StandardPad,
        holes,
    )
}

/// A project with a board, two nets (GND, VCC) and two components R1, R2
/// of a library component with signals S1, S2 whose device has the
/// footprint pads `fpt_pads` = [SMT top (S1), SMT top (S2), THT (no
/// signal)]. R1: S1 = GND, S2 = VCC; R2: S1 = GND, S2 unconnected. The
/// devices are placed at (0, 0) and (10mm, 0) (not added by `new()`).
pub struct Fixture {
    pub p: Project,
    pub board: BoardId,
    pub lib_device: Uuid,
    pub lib_package: Uuid,
    pub footprint: Uuid,
    pub fpt_pads: [Uuid; 3],
    pub gnd: NetSignalId,
    pub vcc: NetSignalId,
    pub r1: ComponentInstanceId,
    pub r2: ComponentInstanceId,
}

impl Fixture {
    pub fn new() -> Self {
        Self::build(new_project())
    }

    /// Builds the fixture in the (new, empty) project `p`.
    pub fn build(mut p: Project) -> Self {
        let gnd = add_net(&mut p, "GND");
        let vcc = add_net(&mut p, "VCC");
        let cmp_uuid = Uuid::new_random();
        let (cmp, variant) = library_component(cmp_uuid, 2, Uuid::new_random());
        let signals = cmp.signals().uuids();
        p.add_library_component(cmp).unwrap();

        // Package with three pads, one footprint.
        let pkg_uuid = Uuid::new_random();
        let mut package = Package::new(metadata(pkg_uuid, "PKG"), AssemblyType::Smt).unwrap();
        let pkg_pads: Vec<Uuid> = (1..=3)
            .map(|i| {
                let uuid = Uuid::new_random();
                package.pads_mut().push(PackagePad::new(
                    uuid,
                    CircuitIdentifier::new(format!("{i}")).unwrap(),
                ));
                uuid
            })
            .collect();
        let footprint_uuid = Uuid::new_random();
        let mut footprint = Footprint::new(
            footprint_uuid,
            ElementName::new("default").unwrap(),
            String::new(),
        );
        let fpt_pads = [Uuid::new_random(), Uuid::new_random(), Uuid::new_random()];
        let specs = [
            (pt(-1.0, 0.0), false),
            (pt(1.0, 0.0), false),
            (pt(0.0, 2.0), true),
        ];
        for ((uuid, (pos, tht)), pkg_pad) in fpt_pads.iter().zip(specs).zip(&pkg_pads) {
            footprint.pads_mut().push(FootprintPad::new(
                pad(*uuid, pos, ComponentSide::Top, tht),
                Some(*pkg_pad),
            ));
        }
        package.footprints_mut().push(footprint);
        p.add_library_package(package).unwrap();

        // Device: P1 -> S1, P2 -> S2, P3 unconnected.
        let dev_uuid = Uuid::new_random();
        let mut device = Device::new(metadata(dev_uuid, "DEV"), cmp_uuid, pkg_uuid).unwrap();
        for (pkg_pad, signal) in pkg_pads
            .iter()
            .zip([Some(signals[0]), Some(signals[1]), None])
        {
            device
                .pad_signal_map_mut()
                .push(DevicePadSignalMapItem::new(*pkg_pad, signal, false));
        }
        p.add_library_device(device).unwrap();

        let mut r1 = component_instance(&p, cmp_uuid, variant, "R1");
        r1.set_signal_net(&signals[0], Some(gnd)).unwrap();
        r1.set_signal_net(&signals[1], Some(vcc)).unwrap();
        let r1 = p.add_component_instance(r1).unwrap();
        let mut r2 = component_instance(&p, cmp_uuid, variant, "R2");
        r2.set_signal_net(&signals[0], Some(gnd)).unwrap();
        let r2 = p.add_component_instance(r2).unwrap();

        let board = p
            .add_board(
                Board::new(
                    Uuid::new_random(),
                    ElementName::new("default").unwrap(),
                    "default",
                ),
                None,
            )
            .unwrap();
        Self {
            p,
            board,
            lib_device: dev_uuid,
            lib_package: pkg_uuid,
            footprint: footprint_uuid,
            fpt_pads,
            gnd,
            vcc,
            r1,
            r2,
        }
    }

    /// A device of `component` at `pos` (not mirrored).
    pub fn device(&self, component: ComponentInstanceId, pos: Point) -> BoardDevice {
        BoardDevice::new(
            component,
            self.lib_device,
            self.footprint,
            pos,
            Angle::DEG0,
            false,
            false,
            true,
        )
    }

    /// The fixture with the devices of R1 at (0, 0) and R2 at (10mm, 0).
    pub fn with_devices() -> Self {
        let mut f = Self::new();
        let (d1, d2) = (f.device(f.r1, pt(0.0, 0.0)), f.device(f.r2, pt(10.0, 0.0)));
        f.p.add_board_device(f.board, d1).unwrap();
        f.p.add_board_device(f.board, d2).unwrap();
        f
    }

    /// The anchor of footprint pad `index` of a component's device.
    pub fn pad_anchor(&self, component: ComponentInstanceId, index: usize) -> TraceAnchor {
        TraceAnchor::FootprintPad {
            device: component.0,
            pad: self.fpt_pads[index],
        }
    }

    /// Returns the board.
    pub fn board(&self) -> &Board {
        self.p.board(self.board).unwrap()
    }
}

/// The signal of a component connected to GND (S1).
pub fn gnd_signal(f: &Fixture, component: ComponentInstanceId) -> Uuid {
    f.p.circuit()
        .component_instance(component)
        .unwrap()
        .signals()
        .iter()
        .find(|(_, s)| s.net() == Some(f.gnd))
        .map(|(uuid, _)| *uuid)
        .unwrap()
}

/// A trace on top copper (0.5mm).
pub fn trace(a: TraceAnchor, b: TraceAnchor) -> Trace {
    trace_on(Layer::TOP_COPPER, a, b)
}

/// A trace on `layer` (0.5mm).
pub fn trace_on(layer: Layer, a: TraceAnchor, b: TraceAnchor) -> Trace {
    Trace::new(Uuid::new_random(), layer, pos_mm(0.5), a, b)
}

/// A GND segment connecting R1.S1 and R2.S1 through a junction at (5, 0).
pub fn gnd_segment(f: &Fixture) -> (BoardNetSegment, Junction) {
    let junction = Junction::new(Uuid::new_random(), pt(5.0, 0.0));
    let j = TraceAnchor::Junction(junction.uuid());
    let segment = BoardNetSegment::with_elements(
        Uuid::new_random(),
        Some(f.gnd),
        Vec::<BoardPadData>::new(),
        vec![],
        vec![junction.clone()],
        vec![
            trace(f.pad_anchor(f.r1, 0), j),
            trace(j, f.pad_anchor(f.r2, 0)),
        ],
    );
    (segment, junction)
}
