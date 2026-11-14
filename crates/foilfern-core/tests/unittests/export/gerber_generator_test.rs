//! Port of tests/unittests/core/export/gerbergeneratortest.cpp, plus a test
//! of the complete file layout against an upstream export result.

use std::sync::LazyLock;

use chrono::NaiveDate;
use foilfern_core::export::{
    ApertureFunction, BoardSide, ComponentAttributes, CopperSide, GerberFileInfo, GerberGenerator,
    MountType, ObjectAttributes, Polarity, Timestamp,
};
use foilfern_core::geometry::{Path, StraightAreaPath, Vertex};
use foilfern_core::types::{Point, UnsignedLength};
use regex::Regex;

use super::{date_utc_plus_1, deg, pos, uns};

/// Generates output containing all possible features (upstream
/// `generateEverything()`). If `component_pins` is false, Gerber X3
/// component pins are not exported.
fn generate_everything(component_pins: bool) -> String {
    let mut g = GerberGenerator::new(&GerberFileInfo {
        app_version: "0.1.2".into(),
        creation_date: date_utc_plus_1(2000, 2, 1, 1, 2, 3, 4),
        project_name: "Project Name".into(),
        project_uuid: "bdf7bea5-b88e-41b2-be85-c1604e8ddfca".parse().unwrap(),
        project_revision: "rev-1.0".into(),
    });

    g.set_file_function_copper(1, CopperSide::Top, Polarity::Positive);

    g.set_layer_polarity(Polarity::Negative);
    g.set_layer_polarity(Polarity::Positive);

    let p = Point::from_nm(100, 200);
    let outline = StraightAreaPath::new(Path::new(vec![
        Vertex::at(Point::from_nm(-100, -100)),
        Vertex::at(Point::from_nm(100, -100)),
        Vertex::at(Point::from_nm(0, 100)),
        Vertex::at(Point::from_nm(-100, -100)),
    ]))
    .unwrap();
    for function in [None, Some(ApertureFunction::Conductor)] {
        for net in [None, Some(""), Some("Foo")] {
            for component in ["", "Bar"] {
                let attrs = ObjectAttributes {
                    function,
                    net,
                    component,
                    ..ObjectAttributes::default()
                };
                g.draw_line(
                    Point::from_nm(500, 600),
                    Point::from_nm(700, 800),
                    uns(100000),
                    &attrs,
                );

                let circle = Path::circle(pos(1000000));
                let rect = Path::centered_rect(pos(1000000), pos(1000000), UnsignedLength::ZERO);
                g.draw_path_outline(&circle, uns(100000), &attrs);
                g.draw_path_outline(&rect, uns(100000), &attrs);
                g.draw_path_area(&circle, &attrs);
                g.draw_path_area(&rect, &attrs);

                for pin in ["", "42"] {
                    for signal in ["", "ENABLE"] {
                        let attrs = ObjectAttributes {
                            pin,
                            signal,
                            ..attrs
                        };
                        g.flash_circle(p, pos(100000), &attrs);

                        for i in (-355..=355).step_by(5) {
                            let rot = deg(i * 1000000); // -355..+355° in 5° steps

                            for r in [0, 20000] {
                                // No rounded corners, rounded corners.
                                g.flash_rect(p, pos(100000), pos(100000), uns(r), rot, &attrs);
                                g.flash_rect(p, pos(100000), pos(200000), uns(r), rot, &attrs);
                                g.flash_rect(p, pos(200000), pos(100000), uns(r), rot, &attrs);
                            }

                            g.flash_obround(p, pos(100000), pos(100000), rot, &attrs);
                            g.flash_obround(p, pos(100000), pos(200000), rot, &attrs);
                            g.flash_obround(p, pos(200000), pos(100000), rot, &attrs);

                            for r in [0, 20000] {
                                // No rounded corners, rounded corners.
                                g.flash_octagon(p, pos(100000), pos(100000), uns(r), rot, &attrs);
                                g.flash_octagon(p, pos(100000), pos(200000), uns(r), rot, &attrs);
                                g.flash_octagon(p, pos(200000), pos(100000), uns(r), rot, &attrs);
                            }

                            g.flash_outline(p, &outline, rot, &attrs);

                            let tht = ComponentAttributes {
                                designator: component,
                                value: "",
                                mount_type: MountType::Tht,
                                manufacturer: "",
                                mpn: "",
                                footprint: "",
                            };
                            let smt = ComponentAttributes {
                                designator: component,
                                value: "value",
                                mount_type: MountType::Smt,
                                manufacturer: "manufacturer",
                                mpn: "mpn",
                                footprint: "footprint",
                            };
                            g.flash_component(p, rot, &tht);
                            g.flash_component(p, rot, &smt);

                            if component_pins {
                                g.flash_component_pin(p, rot, &tht, pin, signal, false);
                                g.flash_component_pin(p, rot, &smt, pin, signal, true);
                            }
                        }
                    }
                }
            }
        }
    }
    g.generate()
}

static EVERYTHING: LazyLock<String> = LazyLock::new(|| generate_everything(true));
static EVERYTHING_WITHOUT_PINS: LazyLock<String> = LazyLock::new(|| generate_everything(false));

// Check if there are no X2 attributes if not explicitly enabled.
#[test]
fn test_does_not_contain_x2_attributes() {
    assert!(!EVERYTHING.contains("%T"));
}

// Check if we always use multi quadrant mode (G75) and never single quadrant
// mode (G74). G74 is buggy in some renderers (see
// https://github.com/LibrePCB/LibrePCB/issues/247) and was marked as
// deprecated in the current Gerber specs.
#[test]
fn test_using_only_multi_quadrant_mode() {
    assert!(!EVERYTHING.contains("G74"));
    assert!(EVERYTHING.contains("G75"));
}

// Check if there are no zero-sized apertures used. Such apertures are
// generally allowed, but not recommended by the Gerber specs. Since only
// circles are allowed to have a size of zero, only circles are checked.
#[test]
fn test_does_not_contain_zero_size_apertures() {
    let re = Regex::new(r"^%ADD\d+C,(.+)\*%$").unwrap();
    let mut checked_circles = 0;
    for line in EVERYTHING_WITHOUT_PINS.split('\n') {
        if let Some(captures) = re.captures(line) {
            assert!(captures[1].parse::<f64>().unwrap() > 0.0, "{line}");
            checked_circles += 1;
        }
    }
    assert!(checked_circles >= 3); // Sanity check if test works.
}

/// Reproduces tests/data/unittests/librepcbproject/BoardGerberExportTest/
/// expected/test_project_GLUE-TOP.gbr (MD5 line removed like upstream's
/// test does) to check the complete file layout.
#[test]
fn test_full_file_layout() {
    let date = NaiveDate::from_ymd_opt(2019, 1, 2)
        .unwrap()
        .and_hms_opt(3, 4, 5)
        .unwrap();
    let mut g = GerberGenerator::new(&GerberFileInfo {
        app_version: "0.1.2".into(),
        creation_date: Timestamp::Local(date),
        project_name: "test_project".into(),
        project_uuid: "7c04f9d5-7366-4c4b-9e17-852ed57b3966".parse().unwrap(),
        project_revision: "v1".into(),
    });
    g.set_file_function_glue(BoardSide::Top, Polarity::Positive);
    g.draw_line(
        Point::from_nm(-3160000, 41680000),
        Point::from_nm(-8240000, 41680000),
        uns(1800000),
        &ObjectAttributes {
            component: "D1",
            ..ObjectAttributes::default()
        },
    );
    let output = g.generate();

    let md5 = Regex::new(r"(?m)^G04 #@! TF\.MD5,([0-9a-f]{32})\*$").unwrap();
    assert!(md5.is_match(&output), "{output}");
    let actual = md5.replace(&output, "");
    let expected = std::fs::read_to_string(crate::helpers::test_data_dir().join(
        "unittests/librepcbproject/BoardGerberExportTest/expected/test_project_GLUE-TOP.gbr",
    ))
    .unwrap();
    assert_eq!(expected, actual);
}

// The MD5 checksum covers the whole output without line breaks.
#[test]
fn test_md5() {
    let g = GerberGenerator::new(&GerberFileInfo {
        app_version: String::new(),
        creation_date: Timestamp::Utc(
            NaiveDate::from_ymd_opt(2019, 1, 2)
                .unwrap()
                .and_hms_opt(3, 4, 5)
                .unwrap(),
        ),
        project_name: "P".into(),
        project_uuid: "7c04f9d5-7366-4c4b-9e17-852ed57b3966".parse().unwrap(),
        project_revision: "1".into(),
    });
    let output = g.generate();
    let (content, footer) = output.split_at(output.find("G04 #@! TF.MD5").unwrap());
    // Reference value: `printf '%s' "$content_without_newlines" | md5sum`.
    assert_eq!(
        content,
        "G04 --- HEADER BEGIN --- *\n\
         G04 #@! TF.GenerationSoftware,LibrePCB,LibrePCB*\n\
         G04 #@! TF.CreationDate,2019-01-02T03:04:05Z*\n\
         G04 #@! TF.ProjectId,P,7c04f9d5-7366-4c4b-9e17-852ed57b3966,1*\n\
         G04 #@! TF.Part,Single*\n\
         G04 #@! TF.SameCoordinates*\n\
         %FSLAX66Y66*%\n\
         %MOMM*%\n\
         G01*\n\
         G75*\n\
         G04 --- HEADER END --- *\n\
         G04 --- APERTURE LIST BEGIN --- *\n\
         G04 --- APERTURE LIST END --- *\n\
         G04 --- BOARD BEGIN --- *\n\
         G04 --- BOARD END --- *\n"
    );
    assert_eq!(
        footer,
        "G04 #@! TF.MD5,268d096e3ebd99f972b8c2f7b41f46a0*\nM02*\n"
    );
}
