//! Context and precedence tests for ProjectAttributeLookup (no upstream
//! unit tests exist for this class).

use super::*;
use chrono::{Local, TimeZone};
use librepcb_core::attribute::{Attribute, AttributeList, AttributeType};
use librepcb_core::library::dev::Part;
use librepcb_core::project::{Error, ProjectAttributeLookup as Lookup};
use librepcb_core::types::{Point, SimpleString};

fn attributes(values: &[(&str, &str)]) -> AttributeList {
    values
        .iter()
        .map(|(key, value)| {
            Attribute::new(key.parse().unwrap(), AttributeType::String, *value, None).unwrap()
        })
        .collect::<Vec<_>>()
        .into()
}

fn check(lookup: Lookup<'_>, values: &[(&str, &str)]) {
    for (key, expected) in values {
        assert_eq!(
            lookup.try_value(key).unwrap().as_deref(),
            Some(*expected),
            "{key}"
        );
    }
    assert_eq!(lookup.try_value("unknown").unwrap(), None);
}

#[test]
fn project_builtins_paths_and_local_dates() {
    let mut p = new_project();
    let mut metadata = p.metadata().clone();
    metadata.created = Local
        .with_ymd_and_hms(2025, 1, 2, 3, 4, 5)
        .single()
        .unwrap()
        .with_timezone(&Utc);
    metadata.author = "Author".into();
    p.apply(Mutation::SetProjectMetadata(metadata)).unwrap();
    let lookup = Lookup::for_project(&p, None);
    check(
        lookup,
        &[
            ("PROJECT", "Unnamed"),
            ("PROJECT_FILENAME", "test.lpp"),
            ("AUTHOR", "Author"),
            ("VERSION", "v1"),
            ("PAGES", "0"),
            ("CREATED_DATE", "2025-01-02"),
            ("CREATED_TIME", "03:04:05"),
            ("PAGE_X_OF_Y", "Page {{PAGE}} of {{PAGES}}"),
        ],
    );
    assert_eq!(
        lookup.try_value("PROJECT_FILEPATH").unwrap(),
        p.file_path().map(|p| p.to_native())
    );
    assert_eq!(
        lookup.try_value("PROJECT_DIRPATH").unwrap(),
        p.path().map(|p| p.to_native())
    );
    assert_eq!(
        lookup.try_value("PROJECT_BASENAME").unwrap().as_deref(),
        Some("test")
    );
    let now = p.date_time().with_timezone(&Local);
    assert_eq!(
        lookup.try_value("DATE").unwrap(),
        Some(now.format("%Y-%m-%d").to_string())
    );
    assert_eq!(
        lookup.try_value("TIME").unwrap(),
        Some(now.format("%H:%M:%S").to_string())
    );
}

#[test]
fn pages_boards_and_variant_precedence() {
    let mut p = new_project();
    let mut metadata = p.metadata().clone();
    metadata.attributes = attributes(&[
        ("VARIANT", "project"),
        ("BOARD", "project"),
        ("PAGE", "project"),
    ]);
    p.apply(Mutation::SetProjectMetadata(metadata)).unwrap();
    p.add_schematic(
        Schematic::new(
            Uuid::new_random(),
            ElementName::new("First").unwrap(),
            "first",
        ),
        None,
    )
    .unwrap();
    let second = p
        .add_schematic(
            Schematic::new(
                Uuid::new_random(),
                ElementName::new("Second").unwrap(),
                "second",
            ),
            None,
        )
        .unwrap();
    p.add_board(
        Board::new(
            Uuid::new_random(),
            ElementName::new("First Board").unwrap(),
            "board1",
        ),
        None,
    )
    .unwrap();
    let board = p
        .add_board(
            Board::new(
                Uuid::new_random(),
                ElementName::new("Second Board").unwrap(),
                "board2",
            ),
            None,
        )
        .unwrap();
    let variant = p.circuit().assembly_variants().first().unwrap();
    check(
        Lookup::for_project(&p, Some(variant)),
        &[("VARIANT", "Std"), ("VARIANT_INDEX", "0"), ("PAGES", "2")],
    );
    let lookup = Lookup::for_schematic(&p, p.schematic(second).unwrap(), Some(variant));
    check(
        lookup,
        &[("SHEET", "Second"), ("PAGE", "2"), ("VARIANT", "Std")],
    );
    assert_eq!(
        lookup.try_substitute("{{PAGE_X_OF_Y}}").unwrap(),
        "Page 2 of 2"
    );
    check(
        Lookup::for_board(&p, p.board(board).unwrap(), Some(variant)),
        &[
            ("BOARD", "Second Board"),
            ("BOARD_DIRNAME", "board2"),
            ("BOARD_INDEX", "1"),
            ("VARIANT", "Std"),
        ],
    );
    check(
        Lookup::for_project(&p, None),
        &[("VARIANT", "project"), ("BOARD", "project")],
    );
}

#[test]
fn component_device_part_precedence_and_empty_values() {
    let mut f = board::Fixture::new();
    let mut metadata = f.p.metadata().clone();
    metadata.attributes = attributes(&[("CUSTOM", "project"), ("AUTHOR", "custom author")]);
    f.p.apply(Mutation::SetProjectMetadata(metadata)).unwrap();
    let mut component = f.p.circuit().component_instance(f.r1).unwrap().clone();
    component.set_attributes(attributes(&[
        ("CUSTOM", "component"),
        ("EMPTY", ""),
        ("NAME", "custom name"),
    ]));
    component.set_value("{{MPN}}".into());
    f.p.apply(Mutation::UpdateComponentInstance(component.properties()))
        .unwrap();
    let mut device = f.device(f.r1, Point::ORIGIN);
    device.set_attributes(attributes(&[
        ("CUSTOM", "device"),
        ("EMPTY", "device"),
        ("NAME", "device name"),
    ]));
    let part = Part::new(
        SimpleString::new("Part Number").unwrap(),
        SimpleString::new("Maker").unwrap(),
        attributes(&[("CUSTOM", "part")]),
    );
    let p = &f.p;
    let board = p.board(f.board).unwrap();
    let lookup = Lookup::for_component(p, &component, Some(&device), Some(&part));
    check(
        lookup,
        &[
            ("CUSTOM", "part"),
            ("MPN", "Part Number"),
            ("MANUFACTURER", "Maker"),
            ("NAME", "custom name"),
            ("EMPTY", ""),
            ("COMPONENT", "Test Component"),
            ("DEVICE", "DEV"),
            ("PACKAGE", "PKG"),
            ("FOOTPRINT", "default"),
            ("AUTHOR", "custom author"),
        ],
    );
    assert_eq!(lookup.try_substitute("{{VALUE}}").unwrap(), "Part Number");
    assert_eq!(
        Lookup::for_component(p, &component, Some(&device), None)
            .try_value("CUSTOM")
            .unwrap()
            .as_deref(),
        Some("component")
    );
    check(
        Lookup::for_device(p, board, &device, None),
        &[
            ("CUSTOM", "device"),
            ("EMPTY", "device"),
            ("NAME", "device name"),
            ("BOARD", "default"),
        ],
    );
    check(
        Lookup::for_device(p, board, &device, Some(&part)),
        &[("CUSTOM", "part"), ("MPN", "Part Number")],
    );
    let empty_part = Part::new(
        SimpleString::new("").unwrap(),
        SimpleString::new("").unwrap(),
        attributes(&[("CUSTOM", ""), ("MPN", "override")]),
    );
    check(
        Lookup::for_component(p, &component, None, Some(&empty_part)),
        &[("CUSTOM", ""), ("MPN", "override")],
    );
}

#[test]
fn invalid_device_reference_is_fallible_and_lower_scopes_are_lazy() {
    let f = board::Fixture::new();
    let mut device = f.device(f.r1, Point::ORIGIN);
    device.set_lib_device(Uuid::new_random());
    let component = f.p.circuit().component_instance(f.r1).unwrap();
    let lookup = Lookup::for_component(&f.p, component, Some(&device), None);
    assert_eq!(lookup.try_value("NAME").unwrap().as_deref(), Some("R1"));
    assert!(matches!(
        lookup.try_value("DEVICE"),
        Err(Error::MissingLibraryDevice(_))
    ));
    assert!(matches!(
        lookup.try_substitute("{{DEVICE}}"),
        Err(Error::MissingLibraryDevice(_))
    ));
}

#[test]
fn library_names_follow_project_locale_order() {
    use librepcb_core::library::LibraryBaseElement;

    let mut p = new_project();
    let (mut lib, variant) = library_component(Uuid::new_random(), 0, Uuid::new_random());
    let mut names = lib.metadata().names().clone();
    names.insert("de_DE", ElementName::new("Bauteil").unwrap());
    lib.metadata_mut().set_names(names);
    let id = lib.metadata().uuid();
    p.add_library_component(lib).unwrap();
    let component = component_instance(&p, id, variant, "R1");
    check(
        Lookup::for_component(&p, &component, None, None),
        &[("COMPONENT", "Test Component")],
    );
    let mut settings = p.settings().clone();
    settings.locale_order = vec!["fr_FR".into(), "de_DE".into()];
    p.apply(Mutation::SetProjectSettings(settings)).unwrap();
    check(
        Lookup::for_component(&p, &component, None, None),
        &[("COMPONENT", "Bauteil")],
    );
}

#[test]
fn project_attributes_override_builtins_and_include_units() {
    let mut p = new_project();
    let mut metadata = p.metadata().clone();
    metadata.attributes = attributes(&[("PROJECT", "Custom"), ("AUTHOR", "")]);
    metadata.attributes.push(
        Attribute::new(
            "RESISTANCE".parse().unwrap(),
            AttributeType::Resistance,
            "4.7",
            AttributeType::Resistance
                .unit_from_string("kiloohm")
                .unwrap(),
        )
        .unwrap(),
    );
    p.apply(Mutation::SetProjectMetadata(metadata)).unwrap();
    let lookup = Lookup::for_project(&p, None);
    check(
        lookup,
        &[
            ("PROJECT", "Custom"),
            ("AUTHOR", ""),
            ("RESISTANCE", "4.7kΩ"),
        ],
    );
    assert_eq!(lookup.try_value("project").unwrap(), None);
    assert_eq!(
        lookup.try_substitute("{{AUTHOR or 'fallback'}}").unwrap(),
        "fallback"
    );
}
