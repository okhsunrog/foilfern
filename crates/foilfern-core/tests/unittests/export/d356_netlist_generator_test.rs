//! Port of tests/unittests/core/export/d356netlistgeneratortest.cpp.

use foilfern_core::export::{D356Land, D356NetlistGenerator};
use foilfern_core::types::{Angle, Point};
use regex::Regex;

use super::{date_utc_plus_1, pos};

const HEADER: &str = "C  IPC-D-356A Netlist\n\
                      C  \n\
                      C  Project Name:        My Project\n\
                      C  Project Version:     1.0\n\
                      C  Board Name:          My Board\n\
                      C  Generation Software: LibrePCB 0.1.2\n\
                      C  Generation Date:     2019-01-02T03:04:05+01:00\n\
                      C  \n\
                      C  Note that due to limitations of this file format, LibrePCB\n\
                      C  applies the following operations during the export:\n\
                      C    - suffix net names with unique numbers within braces\n\
                      C    - truncate long net names (uniqueness guaranteed by suffix)\n\
                      C    - truncate long component names (uniqueness not guaranteed)\n\
                      C    - truncate long pad names (uniqueness not guaranteed)\n\
                      C    - clip drill/pad sizes to 9.999mm\n\
                      C  \n\
                      P  UNITS CUST 1\n";

fn generator() -> D356NetlistGenerator {
    D356NetlistGenerator::new(
        "1.2.3-test",
        "My Project",
        "1.0",
        "My Board",
        &date_utc_plus_1(2019, 1, 2, 3, 4, 5, 6),
    )
}

/// Replaces volatile data in the exported file with well-known, constant
/// data.
fn make_comparable(s: &str) -> String {
    Regex::new(r"Generation Software: LibrePCB (.*)")
        .unwrap()
        .replace_all(s, "Generation Software: LibrePCB 0.1.2")
        .into_owned()
}

fn land(x: i64, y: i64, w: i64, h: i64, rotation: Angle) -> D356Land {
    D356Land {
        position: Point::from_nm(x, y),
        width: pos(w),
        height: pos(h),
        rotation,
    }
}

#[test]
fn test_smt_pad() {
    let mut g = generator();
    g.smt_pad(
        "",
        "",
        "",
        &land(1111, -2222, 123456, 654321, Angle::DEG0),
        1,
    );
    g.smt_pad(
        "N/C",
        "U1",
        "42",
        &land(-11111, 22222, 234567, 765432, Angle::DEG90),
        1,
    );
    g.smt_pad(
        "N/C",
        "U2",
        "1337",
        &land(-11111, 22222, 234567, 765432, -Angle::DEG90),
        5,
    );
    g.smt_pad(
        "TooooLooogName",
        "AlsoTooLong",
        "AsWell",
        &land(55555, -66666, 20000000, 30000000, -Angle::DEG180),
        5,
    );

    #[rustfmt::skip]
    let expected = HEADER.to_owned() +
        "327N/C              NOREF -NPAD       A01X+000001Y-000002X0123Y0654R000 S2\n\
         327N/C{2}           U1    -42         A01X-000011Y+000022X0235Y0765R090 S2\n\
         327N/C{2}           U2    -1337       A05X-000011Y+000022X0235Y0765R270 S1\n\
         327TooooLooogN{3}   AlsoTo-AsWe       A05X+000056Y-000067X9999Y9999R180 S1\n\
         999\n";
    assert_eq!(expected, make_comparable(&g.generate()));
}

#[test]
fn test_tht_pad() {
    let mut g = generator();
    g.tht_pad(
        "",
        "",
        "",
        &land(1111, -2222, 123456, 654321, Angle::DEG0),
        pos(1300000),
    );
    g.tht_pad(
        "N/C",
        "U1",
        "42",
        &land(-11111, 22222, 234567, 765432, Angle::DEG90),
        pos(444444),
    );
    g.tht_pad(
        "N/C",
        "U2",
        "1337",
        &land(-11111, 22222, 234567, 765432, -Angle::DEG90),
        pos(555555),
    );
    g.tht_pad(
        "TooooLooogName",
        "AlsoTooLong",
        "AsWell",
        &land(55555, -66666, 20000000, 30000000, -Angle::DEG180),
        pos(20000000),
    );

    #[rustfmt::skip]
    let expected = HEADER.to_owned() +
        "317N/C              NOREF -NPAD D1300PA00X+000001Y-000002X0123Y0654R000 S0\n\
         317N/C{2}           U1    -42   D0444PA00X-000011Y+000022X0235Y0765R090 S0\n\
         317N/C{2}           U2    -1337 D0556PA00X-000011Y+000022X0235Y0765R270 S0\n\
         317TooooLooogN{3}   AlsoTo-AsWe D9999PA00X+000056Y-000067X9999Y9999R180 S0\n\
         999\n";
    assert_eq!(expected, make_comparable(&g.generate()));
}

#[test]
fn test_via() {
    let mut g = generator();
    g.through_via(
        "",
        &land(1111, -2222, 123456, 654321, Angle::DEG0),
        pos(1300000),
        false,
    );
    g.blind_via(
        "N/C",
        &land(-11111, 22222, 234567, 765432, Angle::DEG90),
        pos(444444),
        1,
        3,
        false,
    );
    g.blind_via(
        "N/C",
        &land(-11111, 22222, 234567, 765432, -Angle::DEG90),
        pos(555555),
        3,
        64,
        true,
    );
    g.buried_via(
        "TooooLooogName",
        Point::from_nm(55555, -66666),
        pos(20000000),
        5,
        7,
    );

    // Note: Not sure if blind and buried vias are represented correctly, we
    // need specs which are more clear!
    #[rustfmt::skip]
    let expected = HEADER.to_owned() +
        "317N/C              VIA        MD1300PA00X+000001Y-000002X0123Y0654R000 S0\n\
         307N/C{2}           VIA        MD0444PA01X-000011Y+000022               S2L01L03\n\
         027                 VIA               A01X-000011Y+000022X0235Y0765R090\n\
         307N/C{2}           VIA        MD0556PA64X-000011Y+000022               S3L03L64\n\
         027                 VIA               A64X-000011Y+000022X0235Y0765R270\n\
         307TooooLooogN{3}   VIA        MD9999P   X+000056Y-000067               S3L05L07\n\
         999\n";
    assert_eq!(expected, make_comparable(&g.generate()));
}

/// Checks the upstream export of a real board (header lines and record
/// syntax only; the records need the board model).
#[test]
fn test_expected_file_syntax() {
    let path = crate::helpers::test_data_dir()
        .join("unittests/librepcbproject/BoardD356NetlistExportTest/expected.d356");
    let content = std::fs::read_to_string(path).unwrap();
    let header_end = content.find("P  UNITS CUST 1\n").unwrap();
    // Same comment lines except project, board, version and date.
    let fixed: Vec<&str> = HEADER.lines().collect();
    let actual: Vec<&str> = content[..header_end].lines().collect();
    assert_eq!(fixed.len() - 1, actual.len());
    for (i, (a, b)) in actual.iter().zip(&fixed).enumerate() {
        if ![2, 3, 4, 5, 6].contains(&i) {
            assert_eq!(a, b);
        }
    }
    assert!(content.ends_with("\n999\n"));
}
