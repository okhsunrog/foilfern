//! Project-to-pick&place selection, transforms and fiducial numbering.

use super::*;
use librepcb_core::attribute::AttributeList;
use librepcb_core::export::{BoardSide, PickPlaceCsvWriter, PickPlaceType};
use librepcb_core::library::dev::Part;
use librepcb_core::project::board::BoardPickPlaceGenerator;
use librepcb_core::project::circuit::ComponentAssemblyOption;
use librepcb_core::project::{AssemblyVariantId, Mutation};

#[test]
fn assembly_selection_first_part_and_mirrored_rotation() {
    let mut f = Fixture::new();
    let variant = AssemblyVariantId(f.p.circuit().assembly_variants().first().unwrap().uuid());
    let mut properties = f.p.circuit().component_instance(f.r1).unwrap().properties();
    properties.value = "fallback value".into();
    properties.assembly_options = vec![ComponentAssemblyOption::new(
        f.lib_device,
        AttributeList::new(),
        [variant].into(),
        vec![
            Part::new(
                "First Part".parse().unwrap(),
                "Maker".parse().unwrap(),
                AttributeList::new(),
            ),
            Part::new(
                "Second Part".parse().unwrap(),
                "Maker".parse().unwrap(),
                AttributeList::new(),
            ),
        ]
        .into(),
    )]
    .into();
    f.p.apply(Mutation::UpdateComponentInstance(properties))
        .unwrap();
    let mut device = f.device(f.r1, pt(10.0, 20.0));
    device.set_rotation(Angle::DEG90);
    device.set_mirrored(true);
    f.p.add_board_device(f.board, device).unwrap();
    let device = f.device(f.r2, Point::ORIGIN);
    f.p.add_board_device(f.board, device).unwrap();
    let data = BoardPickPlaceGenerator::new(&f.p, f.board(), variant)
        .generate()
        .unwrap();
    assert_eq!(data.items().len(), 2);
    let first = &data.items()[0];
    assert_eq!(first.designator, "R1");
    assert_eq!(first.value, "First Part");
    assert_eq!(first.position, pt(10.0, 20.0));
    assert_eq!(first.rotation, -Angle::DEG90);
    assert_eq!(first.board_side, BoardSide::Bottom);
    assert_eq!(first.item_type, PickPlaceType::Smt);
    assert!(first.mount);
    assert!(!data.items()[1].mount);
    let mut writer = PickPlaceCsvWriter::new(&data, "test");
    writer.set_include_metadata_comment(false);
    let csv = writer.generate_csv().unwrap().to_csv_string().unwrap();
    assert!(
        csv.contains("R1,First Part,DEV,PKG,10.0,20.0,270.0,Bottom,SMT\n"),
        "{csv}"
    );
    assert!(!csv.contains("R2,"));
}

#[test]
fn fiducials_are_mounted_numbered_and_do_not_collide_with_device_names() {
    let mut f = Fixture::new();
    let mut properties = f.p.circuit().component_instance(f.r1).unwrap().properties();
    properties.name = "FID1".parse().unwrap();
    f.p.apply(Mutation::UpdateComponentInstance(properties))
        .unwrap();
    let mut package = f.p.remove_library_package(&f.lib_package).unwrap();
    package.set_assembly_type(AssemblyType::Auto);
    package
        .footprints_mut()
        .by_uuid_mut(&f.footprint)
        .unwrap()
        .pads_mut()
        .by_uuid_mut(&f.fpt_pads[2])
        .unwrap()
        .pad_mut()
        .set_function(PadFunction::GlobalFiducial);
    f.p.add_library_package(package).unwrap();
    let mut device = f.device(f.r1, pt(10.0, 20.0));
    device.set_rotation(Angle::DEG90);
    f.p.add_board_device(f.board, device).unwrap();
    let mut pad = pad(Uuid::new_random(), pt(30.0, 40.0), ComponentSide::Top, true);
    pad.set_function(PadFunction::GlobalFiducial);
    let mut segment = BoardNetSegment::new(Uuid::new_random(), None);
    segment.insert_pad(BoardPadData::new(pad, false));
    f.p.add_board_net_segment(f.board, segment).unwrap();
    let variant = AssemblyVariantId(f.p.circuit().assembly_variants().first().unwrap().uuid());
    let data = BoardPickPlaceGenerator::new(&f.p, f.board(), variant)
        .generate()
        .unwrap();
    let fiducials: Vec<_> = data
        .items()
        .iter()
        .filter(|item| item.item_type == PickPlaceType::Fiducial)
        .collect();
    assert_eq!(fiducials.len(), 4);
    assert!(fiducials.iter().all(|item| item.mount));
    assert_eq!(
        fiducials
            .iter()
            .map(|item| item.designator.as_str())
            .collect::<Vec<_>>(),
        ["FID1:1", "FID1:2", "FID2", "FID2"]
    );
    assert_eq!(fiducials[0].position, pt(8.0, 20.0));
    assert_eq!(fiducials[0].rotation, Angle::DEG90);
    assert_eq!(fiducials[0].board_side, BoardSide::Top);
    assert_eq!(fiducials[1].board_side, BoardSide::Bottom);
    assert_eq!(fiducials[2].position, pt(30.0, 40.0));
    let device = data
        .items()
        .iter()
        .find(|item| item.designator == "FID1")
        .unwrap();
    assert!(!device.mount);
    // Fiducials do not require soldering and are excluded from auto-detection.
    assert_eq!(device.item_type, PickPlaceType::Smt);
}
