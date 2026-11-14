//! Port of tests/unittests/core/export/pickplacecsvwritertest.cpp.

use foilfern_core::export::{
    BoardSide, PickPlaceCsvWriter, PickPlaceData, PickPlaceDataItem, PickPlaceSides, PickPlaceType,
};
use foilfern_core::types::{Angle, Point};

/// Upstream's `PickPlaceDataItem` constructor.
#[allow(clippy::too_many_arguments)]
fn item(
    designator: &str,
    value: &str,
    device_name: &str,
    package_name: &str,
    position: Point,
    rotation: Angle,
    board_side: BoardSide,
    item_type: PickPlaceType,
    mount: bool,
) -> PickPlaceDataItem {
    PickPlaceDataItem {
        designator: designator.into(),
        value: value.into(),
        device_name: device_name.into(),
        package_name: package_name.into(),
        position,
        rotation,
        board_side,
        item_type,
        mount,
    }
}

fn create_data() -> PickPlaceData {
    let mut data = PickPlaceData::new("project", "version", "board");
    data.add_item(item(
        "R10",
        "",
        "device",
        "pack,\"age\"",
        Point::from_nm(-1000000, -2000000),
        -Angle::DEG45,
        BoardSide::Top,
        PickPlaceType::Tht,
        true,
    ));
    data.add_item(item(
        "U5",
        "1kΩ\r\n\r\n",
        "device",
        "package",
        Point::from_nm(1000000, 2000000),
        Angle::DEG45,
        BoardSide::Bottom,
        PickPlaceType::Smt,
        true,
    ));
    data.add_item(item(
        "R1",
        " 1kΩ\n1W\n100V ",
        "device \"foo\"",
        "pack,age",
        Point::from_nm(1000000, 2000000),
        Angle::DEG45,
        BoardSide::Top,
        PickPlaceType::Fiducial,
        true,
    ));
    data.add_item(item(
        "U1",
        "mixed",
        "mixed device",
        "mixed package",
        Point::ORIGIN,
        Angle::DEG0,
        BoardSide::Bottom,
        PickPlaceType::Mixed,
        true,
    ));
    data.add_item(item(
        "DNM",
        "dnm",
        "",
        "",
        Point::ORIGIN,
        Angle::DEG0,
        BoardSide::Top,
        PickPlaceType::Mixed,
        false,
    ));
    data
}

const HEADER: &str = "Designator,Value,Device,Package,Position X,Position Y,Rotation,Side,Type";
const R1: &str = "R1, 1kΩ 1W 100V ,\"device \"\"foo\"\"\",\"pack,age\",1.0,2.0,45.0,Top,Fiducial";
const R10: &str = "R10,,device,\"pack,\"\"age\"\"\",-1.0,-2.0,315.0,Top,THT";
const U1: &str = "U1,mixed,mixed device,mixed package,0.0,0.0,0.0,Bottom,THT+SMT";
const U5: &str = "U5,1kΩ  ,device,package,1.0,2.0,45.0,Bottom,SMT";

#[test]
fn test_empty_data() {
    let data = PickPlaceData::new("project", "version", "board");
    let mut writer = PickPlaceCsvWriter::new(&data, "0.1.2");
    writer.set_include_metadata_comment(false);
    let file = writer.generate_csv().unwrap();
    assert_eq!(format!("{HEADER}\n"), file.to_csv_string().unwrap());
}

#[test]
fn test_both_sides() {
    let data = create_data();
    let writer = PickPlaceCsvWriter::new(&data, "0.1.2");
    let file = writer.generate_csv().unwrap();
    let content = file.to_csv_string().unwrap();
    let lines: Vec<&str> = content.split('\n').collect();
    assert_eq!("# Pick&Place Position Data File", lines[0]);
    assert_eq!("#", lines[1]);
    assert_eq!("# Project Name:        project", lines[2]);
    assert_eq!("# Project Version:     version", lines[3]);
    assert_eq!("# Board Name:          board", lines[4]);
    assert_eq!("# Unit:                mm", lines[7]);
    assert_eq!("# Rotation:            Degrees CCW", lines[8]);
    assert_eq!("# Board Side:          Top + Bottom", lines[9]);
    assert_eq!(
        "# Assembly Types:      THT, SMT, THT+SMT, Fiducial, Other",
        lines[10]
    );
    assert_eq!("", lines[11]);
    assert_eq!(HEADER, lines[12]);
    assert_eq!(R1, lines[13]);
    assert_eq!(R10, lines[14]);
    assert_eq!(U1, lines[15]);
    assert_eq!(U5, lines[16]);
    assert_eq!("", lines[17]);
    assert_eq!(18, lines.len());
}

#[test]
fn test_top_side() {
    let data = create_data();
    let mut writer = PickPlaceCsvWriter::new(&data, "0.1.2");
    writer.set_include_metadata_comment(false);
    writer.set_board_sides(PickPlaceSides::Top);
    let content = writer.generate_csv().unwrap().to_csv_string().unwrap();
    assert_eq!([HEADER, R1, R10, ""].join("\n"), content);
}

#[test]
fn test_bottom_side() {
    let data = create_data();
    let mut writer = PickPlaceCsvWriter::new(&data, "0.1.2");
    writer.set_include_metadata_comment(false);
    writer.set_board_sides(PickPlaceSides::Bottom);
    let content = writer.generate_csv().unwrap().to_csv_string().unwrap();
    assert_eq!([HEADER, U1, U5, ""].join("\n"), content);
}

// Not upstream: the complete comment, the type filter and non-mounted parts.
#[test]
fn test_metadata_type_filter_and_non_mounted_parts() {
    let data = create_data();
    let mut writer = PickPlaceCsvWriter::new(&data, "1.2.3");
    writer.set_generation_date(foilfern_core::export::Timestamp::Local(
        chrono::NaiveDate::from_ymd_opt(2019, 1, 2)
            .unwrap()
            .and_hms_opt(3, 4, 5)
            .unwrap(),
    ));
    writer.set_type_filter([PickPlaceType::Mixed, PickPlaceType::Tht]);
    writer.set_include_non_mounted_parts(true);
    let content = writer.generate_csv().unwrap().to_csv_string().unwrap();
    let expected = [
        "# Pick&Place Position Data File",
        "#",
        "# Project Name:        project",
        "# Project Version:     version",
        "# Board Name:          board",
        "# Generation Software: LibrePCB 1.2.3",
        "# Generation Date:     2019-01-02T03:04:05",
        "# Unit:                mm",
        "# Rotation:            Degrees CCW",
        "# Board Side:          Top + Bottom",
        "# Assembly Types:      THT, THT+SMT",
        "",
        HEADER,
        "DNM,dnm,,,0.0,0.0,0.0,Top,THT+SMT",
        R10,
        U1,
        "",
    ];
    assert_eq!(expected.join("\n"), content);
}

fn read_expected(name: &str) -> String {
    let path = crate::helpers::test_data_dir().join(format!(
        "unittests/librepcbproject/BoardPickPlaceGeneratorTest/expected/{name}"
    ));
    std::fs::read_to_string(path).unwrap()
}

/// Reproduces the upstream pick&place export of a real board (the rows of
/// `both.csv`, added in reverse order to check the sorting).
#[test]
fn test_expected_files() {
    let both = read_expected("both.csv");
    let mut data = PickPlaceData::new("test_project", "v1", "default");
    for line in both.lines().skip(1).collect::<Vec<_>>().into_iter().rev() {
        let v: Vec<&str> = line.split(',').collect();
        data.add_item(PickPlaceDataItem {
            designator: v[0].into(),
            value: v[1].into(),
            device_name: v[2].into(),
            package_name: v[3].into(),
            position: Point::new(v[4].parse().unwrap(), v[5].parse().unwrap()),
            rotation: v[6].parse().unwrap(),
            board_side: if v[7] == "Top" {
                BoardSide::Top
            } else {
                BoardSide::Bottom
            },
            item_type: *PickPlaceType::ALL
                .iter()
                .find(|t| t.name() == v[8])
                .unwrap(),
            mount: true,
        });
    }

    let mut writer = PickPlaceCsvWriter::new(&data, "0.1.2");
    writer.set_include_metadata_comment(false);
    assert_eq!(
        both,
        writer.generate_csv().unwrap().to_csv_string().unwrap()
    );

    for (sides, name) in [
        (PickPlaceSides::Top, "top.csv"),
        (PickPlaceSides::Bottom, "bottom.csv"),
    ] {
        let mut writer = PickPlaceCsvWriter::new(&data, "0.1.2");
        writer.set_board_sides(sides);
        let actual = writer.generate_csv().unwrap().to_csv_string().unwrap();
        // Replace volatile data like upstream's test does.
        let actual: Vec<&str> = actual
            .split('\n')
            .map(|line| {
                if line.starts_with("# Generation Software:") {
                    "# Generation Software:"
                } else if line.starts_with("# Generation Date:") {
                    "# Generation Date:"
                } else {
                    line
                }
            })
            .collect();
        assert_eq!(read_expected(name), actual.join("\n"), "{name}");
    }
}
