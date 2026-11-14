//! Port of tests/unittests/core/export/excellongeneratortest.cpp.

use foilfern_core::export::{ApertureFunction, Error, ExcellonGenerator, GerberFileInfo, Plating};
use foilfern_core::geometry::{NonEmptyPath, Path, Vertex};
use foilfern_core::types::{Angle, Point};
use regex::Regex;

use super::{date_utc_plus_1, pos};

fn generator() -> ExcellonGenerator {
    let info = GerberFileInfo {
        app_version: "1.2.3-test".into(),
        creation_date: date_utc_plus_1(2000, 2, 1, 1, 2, 3, 4),
        project_name: "My Project".into(),
        project_uuid: "bdf7bea5-b88e-41b2-be85-c1604e8ddfca".parse().unwrap(),
        project_revision: "1.0".into(),
    };
    ExcellonGenerator::new(&info, Plating::Mixed, 1, 4)
}

/// Replaces volatile data in exported files with well-known, constant data.
fn make_comparable(s: &str) -> String {
    let s = Regex::new(r"TF\.GenerationSoftware,LibrePCB,LibrePCB,[^\s\*]*")
        .unwrap()
        .replace_all(s, "TF.GenerationSoftware,LibrePCB,LibrePCB,0.1.2");
    Regex::new(r"TF\.CreationDate,[^\s\*]*")
        .unwrap()
        .replace_all(&s, "TF.CreationDate,2019-01-02T03:04:05")
        .into_owned()
}

const HEADER: &str = "M48\n\
                      ; #@! TF.GenerationSoftware,LibrePCB,LibrePCB,0.1.2\n\
                      ; #@! TF.CreationDate,2019-01-02T03:04:05\n\
                      ; #@! TF.ProjectId,My Project,bdf7bea5-b88e-41b2-be85-c1604e8ddfca,1.0\n\
                      ; #@! TF.Part,Single\n\
                      ; #@! TF.SameCoordinates\n\
                      ; #@! TF.FileFunction,MixedPlating,1,4\n\
                      FMAT,2\n\
                      METRIC,TZ\n";

fn slot(first_angle: Angle) -> NonEmptyPath {
    NonEmptyPath::new(Path::new(vec![
        Vertex::new(Point::from_nm(111, 222), first_angle),
        Vertex::at(Point::from_nm(333, 444)),
        Vertex::at(Point::from_nm(555, 666)),
    ]))
    .unwrap()
}

#[test]
fn test_circular_drills() {
    let mut g = generator();
    g.drill(
        Point::from_nm(111, 222),
        pos(500000),
        true,
        ApertureFunction::ComponentDrill,
    );
    g.drill(
        Point::from_nm(333, 444),
        pos(600000),
        false,
        ApertureFunction::MechanicalDrill,
    );
    g.drill_path(
        NonEmptyPath::from_point(Point::from_nm(555, 666)),
        pos(500000),
        true,
        ApertureFunction::ComponentDrill,
    );

    let expected = HEADER.to_owned()
        + "; #@! TA.AperFunction,Plated,PTH,ComponentDrill\n\
           T1C0.5\n\
           ; #@! TA.AperFunction,NonPlated,NPTH,MechanicalDrill\n\
           T2C0.6\n\
           %\n\
           G90\n\
           G05\n\
           M71\n\
           T1\n\
           X0.000555Y0.000666\n\
           X0.000111Y0.000222\n\
           T2\n\
           X0.000333Y0.000444\n\
           T0\n\
           M30\n";
    assert_eq!(expected, make_comparable(&g.generate().unwrap()));
}

#[test]
fn test_slot_rout() {
    let mut g = generator();
    g.drill_path(
        slot(Angle::DEG90),
        pos(500000),
        false,
        ApertureFunction::MechanicalDrill,
    );

    let expected = HEADER.to_owned()
        + "; #@! TA.AperFunction,NonPlated,NPTH,MechanicalDrill\n\
           T1C0.5\n\
           %\n\
           G90\n\
           G05\n\
           M71\n\
           T1\n\
           G00X0.000111Y0.000222\n\
           M15\n\
           G03X0.000333Y0.000444A0.000222\n\
           G01X0.000555Y0.000666\n\
           M16\n\
           G05\n\
           T0\n\
           M30\n";
    assert_eq!(expected, make_comparable(&g.generate().unwrap()));
}

#[test]
fn test_slot_g85() {
    let mut g = generator();
    g.set_use_g85_slots(true);
    g.drill_path(
        slot(Angle::DEG0),
        pos(500000),
        false,
        ApertureFunction::MechanicalDrill,
    );

    let expected = HEADER.to_owned()
        + "; #@! TA.AperFunction,NonPlated,NPTH,MechanicalDrill\n\
           T1C0.5\n\
           %\n\
           G90\n\
           G05\n\
           M71\n\
           T1\n\
           X0.000111Y0.000222G85X0.000333Y0.000444\n\
           X0.000333Y0.000444G85X0.000555Y0.000666\n\
           T0\n\
           M30\n";
    assert_eq!(expected, make_comparable(&g.generate().unwrap()));
}

#[test]
fn test_curved_slot_g85() {
    let mut g = generator();
    g.set_use_g85_slots(true);
    g.drill_path(
        slot(Angle::DEG90),
        pos(500000),
        false,
        ApertureFunction::MechanicalDrill,
    );
    assert!(matches!(g.generate(), Err(Error::CurvedSlotG85)));
}

// Not upstream: arcs > 180° are split into two arcs. Here: 270° clockwise
// around (-1, 1), split at 135° (relative to the center).
#[test]
fn test_slot_rout_large_arc() {
    let mut g = generator();
    g.drill_path(
        NonEmptyPath::new(Path::new(vec![
            Vertex::new(Point::from_nm(-1000000, 0), Angle::new(-270_000_000)),
            Vertex::at(Point::from_nm(0, 1000000)),
        ]))
        .unwrap(),
        pos(500000),
        true,
        ApertureFunction::ComponentDrill,
    );
    let output = g.generate().unwrap();
    assert!(
        output.contains(
            "G00X-1.0Y0.0\n\
             M15\n\
             G02X-1.707107Y1.707107A1.0\n\
             G02X0.0Y1.0A1.0\n\
             M16\n"
        ),
        "{output}"
    );
}
