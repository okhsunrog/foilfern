//! Port of tests/unittests/core/library/pkg/packagetest.cpp, plus tests of
//! the other `Package` functionality.

use std::collections::BTreeSet;

use foilfern_core::fileio::{FileSystem, TransactionalDirectory};
use foilfern_core::geometry::{
    ComponentSide, NonEmptyPath, Pad, PadFunction, PadHole, PadHoleList, PadShape, Path,
};
use foilfern_core::library::pkg::{
    AlternativeName, AssemblyType, Footprint, FootprintPad, Package, PackageModel, PackagePad,
};
use foilfern_core::library::{BaseMetadata, LibraryBaseElement};
use foilfern_core::serialization::{FromSExpression, Mode, SExpression, ToSExpression};
use foilfern_core::types::{
    Angle, CircuitIdentifier, ElementName, Length, MaskConfig, Point, PositiveLength, Ratio,
    SimpleString, UnsignedLength, UnsignedLimitedRatio, Uuid,
};

use crate::library::assert_upgrade_v01;

const UUID: &str = "da9e3bd5-7c56-4d6c-987c-603220599356";

fn new_package(assembly_type: AssemblyType) -> Package {
    Package::new(
        BaseMetadata::new(
            Uuid::new_random(),
            "0.1".parse().unwrap(),
            "me",
            chrono::Utc::now(),
            ElementName::new("Package").unwrap(),
            "",
            "",
        ),
        assembly_type,
    )
    .unwrap()
}

fn pad(pkg_pad: Option<Uuid>, function: PadFunction, tht: bool) -> FootprintPad {
    let holes = if tht {
        PadHoleList::from(vec![PadHole::new(
            Uuid::new_random(),
            PositiveLength::new(Length::new(500_000)).unwrap(),
            NonEmptyPath::from_point(Point::ORIGIN),
        )])
    } else {
        PadHoleList::new()
    };
    let size = PositiveLength::new(Length::new(1_000_000)).unwrap();
    FootprintPad::new(
        Pad::new(
            Uuid::new_random(),
            Point::ORIGIN,
            Angle::DEG0,
            PadShape::RoundedRect,
            size,
            size,
            UnsignedLimitedRatio::new(Ratio::from_percent(0)).unwrap(),
            Path::default(),
            MaskConfig::Automatic,
            MaskConfig::Automatic,
            UnsignedLength::ZERO,
            ComponentSide::Top,
            function,
            holes,
        ),
        pkg_pad,
    )
}

fn footprint(pads: Vec<FootprintPad>) -> Footprint {
    let mut footprint = Footprint::new(
        Uuid::new_random(),
        ElementName::new("default").unwrap(),
        String::new(),
    );
    footprint.pads_mut().extend(pads);
    footprint
}

#[test]
fn test_upgrade_v01() {
    assert_upgrade_v01::<Package>(&format!("libraries/v0.1.lplib/pkg/{UUID}"), UUID);
}

#[test]
fn test_assembly_type_serialization() {
    let items = [
        (AssemblyType::None, "none"),
        (AssemblyType::Tht, "tht"),
        (AssemblyType::Smt, "smt"),
        (AssemblyType::Mixed, "mixed"),
        (AssemblyType::Other, "other"),
        (AssemblyType::Auto, "auto"),
    ];
    for (value, token) in items {
        assert_eq!(value.to_sexpression().value().unwrap(), token);
        assert_eq!(
            AssemblyType::from_sexpression(&SExpression::token(token)).unwrap(),
            value
        );
    }
    assert!(AssemblyType::from_sexpression(&SExpression::token("foo")).is_err());
}

#[test]
fn test_guess_assembly_type() {
    let mut package = new_package(AssemblyType::Auto);
    // No package pads: nothing to mount.
    assert_eq!(package.guess_assembly_type(), AssemblyType::None);
    let pkg_pad = Uuid::new_random();
    package.pads_mut().push(PackagePad::new(
        pkg_pad,
        CircuitIdentifier::new("1").unwrap(),
    ));
    assert_eq!(package.guess_assembly_type(), AssemblyType::None);

    // Only the first footprint is considered.
    package.footprints_mut().push(footprint(vec![pad(
        Some(pkg_pad),
        PadFunction::StandardPad,
        false,
    )]));
    package.footprints_mut().push(footprint(vec![pad(
        Some(pkg_pad),
        PadFunction::StandardPad,
        true,
    )]));
    assert_eq!(package.guess_assembly_type(), AssemblyType::Smt);
    assert_eq!(package.resolved_assembly_type(), AssemblyType::Smt);
    assert_eq!(package.assembly_type(), AssemblyType::Auto);

    package.footprints_mut()[0]
        .pads_mut()
        .push(pad(None, PadFunction::StandardPad, true));
    assert_eq!(package.guess_assembly_type(), AssemblyType::Mixed);
    package.footprints_mut()[0].pads_mut().take(0);
    assert_eq!(package.guess_assembly_type(), AssemblyType::Tht);

    // Pads which are not soldered are ignored.
    package.footprints_mut()[0] = footprint(vec![pad(None, PadFunction::TestPad, false)]);
    assert_eq!(package.guess_assembly_type(), AssemblyType::None);

    assert!(package.set_assembly_type(AssemblyType::Other));
    assert_eq!(package.resolved_assembly_type(), AssemblyType::Other);
}

#[test]
fn test_models_for_footprint() {
    let mut package = new_package(AssemblyType::Smt);
    let models: Vec<PackageModel> = (0..3)
        .map(|i| {
            PackageModel::new(
                Uuid::new_random(),
                ElementName::new(format!("Model {i}")).unwrap(),
            )
        })
        .collect();
    package.models_mut().extend(models.iter().cloned());
    let mut fpt = footprint(Vec::new());
    fpt.set_models(BTreeSet::from([
        models[2].uuid(),
        models[0].uuid(),
        Uuid::new_random(),
    ]));
    let fpt_uuid = fpt.uuid();
    package.footprints_mut().push(fpt);
    assert_eq!(
        package.models_for_footprint(&fpt_uuid),
        [&models[0], &models[2]]
    );
    assert!(package.models_for_footprint(&Uuid::new_random()).is_empty());
}

#[test]
fn test_serialize_alternative_names() {
    let mut package = new_package(AssemblyType::Tht);
    package.set_alternative_names(vec![
        AlternativeName {
            name: ElementName::new("TO-220").unwrap(),
            reference: SimpleString::new("JEDEC").unwrap(),
        },
        AlternativeName {
            name: ElementName::new("SC-46").unwrap(),
            reference: SimpleString::new("").unwrap(),
        },
    ]);
    package.save().unwrap();
    let content = String::from_utf8(package.directory().read("package.lp").unwrap()).unwrap();
    assert!(
        content.contains(
            " (alternative_name \"TO-220\" (reference \"JEDEC\"))\n \
             (alternative_name \"SC-46\" (reference \"\"))\n \
             (assembly_type tht)\n \
             (grid_interval 2.54)\n \
             (min_copper_clearance 0.2)\n"
        ),
        "{content}"
    );

    // Loading gives the same content again.
    let reloaded = Package::load(
        TransactionalDirectory::new_temporary().unwrap(),
        &SExpression::parse(content.as_bytes(), None, Mode::LibrePcb).unwrap(),
    )
    .unwrap();
    assert_eq!(reloaded.alternative_names(), package.alternative_names());
    assert_eq!(
        reloaded
            .to_sexpression()
            .to_byte_array(Mode::LibrePcb)
            .unwrap(),
        content.as_bytes()
    );
}

#[test]
fn test_duplicate_from() {
    let mut original = new_package(AssemblyType::Smt);
    original.set_min_copper_clearance(UnsignedLength::new(Length::new(150_000)).unwrap());
    let pkg_pad = Uuid::new_random();
    original.pads_mut().push(PackagePad::new(
        pkg_pad,
        CircuitIdentifier::new("1").unwrap(),
    ));
    let model = PackageModel::new(Uuid::new_random(), ElementName::new("Model").unwrap());
    original
        .directory_mut()
        .write(&model.file_name(), b"STEP")
        .unwrap();
    original.models_mut().push(model.clone());
    let mut fpt = footprint(vec![
        pad(Some(pkg_pad), PadFunction::StandardPad, false),
        pad(None, PadFunction::StandardPad, false),
    ]);
    fpt.set_models(BTreeSet::from([model.uuid()]));
    fpt.names_mut()
        .insert("de_DE", ElementName::new("Standard").unwrap());
    original.footprints_mut().push(fpt);

    let mut copy = new_package(AssemblyType::None);
    let copy_uuid = copy.metadata().uuid();
    copy.duplicate_from(&original).unwrap();
    assert_eq!(copy.metadata().uuid(), copy_uuid);
    assert_eq!(copy.assembly_type(), AssemblyType::Smt);
    assert_eq!(copy.min_copper_clearance(), original.min_copper_clearance());

    // New UUIDs everywhere, references translated.
    let new_pad = copy.pads()[0].uuid();
    assert_ne!(new_pad, pkg_pad);
    assert_eq!(copy.pads()[0].name(), original.pads()[0].name());
    let new_model = &copy.models()[0];
    assert_ne!(new_model.uuid(), model.uuid());
    assert_eq!(
        copy.directory().read(&new_model.file_name()).unwrap(),
        b"STEP"
    );
    let new_fpt = &copy.footprints()[0];
    assert_ne!(new_fpt.uuid(), original.footprints()[0].uuid());
    assert_eq!(new_fpt.models(), &BTreeSet::from([new_model.uuid()]));
    assert_eq!(new_fpt.pads()[0].package_pad_uuid(), Some(new_pad));
    assert_eq!(new_fpt.pads()[1].package_pad_uuid(), None);
    assert_ne!(
        new_fpt.pads()[0].uuid(),
        original.footprints()[0].pads()[0].uuid()
    );
    // Translations are not copied (like upstream).
    assert!(new_fpt.names().get("de_DE").is_none());
}
