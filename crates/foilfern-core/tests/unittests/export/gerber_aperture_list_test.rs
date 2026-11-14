//! Port of tests/unittests/core/export/gerberaperturelisttest.cpp.

use std::sync::LazyLock;

use foilfern_core::export::{ApertureFunction, GerberApertureList};
use foilfern_core::geometry::{Path, StraightAreaPath, Vertex};
use foilfern_core::types::{Angle, Point};
use regex::Regex;

use super::{deg, pos, uns};

/// All possible apertures (upstream `generateEverything()`).
static EVERYTHING: LazyLock<String> = LazyLock::new(|| {
    let mut l = GerberApertureList::new();
    for function in [None, Some(ApertureFunction::Conductor)] {
        l.add_circle(uns(0), function);
        l.add_circle(uns(100000), function);

        for i in -359..=359 {
            let rot = deg(i * 1000000); // -359..+359°

            l.add_obround(pos(100000), pos(100000), rot, function);
            l.add_obround(pos(100000), pos(200000), rot, function);
            l.add_obround(pos(200000), pos(100000), rot, function);

            for r in [0, 20000] {
                // No rounded corners, rounded corners.
                l.add_rect(pos(100000), pos(100000), uns(r), rot, function);
                l.add_rect(pos(100000), pos(200000), uns(r), rot, function);
                l.add_rect(pos(200000), pos(100000), uns(r), rot, function);
            }

            for r in [0, 20000] {
                // No rounded corners, rounded corners.
                l.add_octagon(pos(100000), pos(100000), uns(r), rot, function);
                l.add_octagon(pos(100000), pos(200000), uns(r), rot, function);
                l.add_octagon(pos(200000), pos(100000), uns(r), rot, function);
            }
        }
    }
    l.generate_string()
});

/// Returns all aperture macros (`%AM...%`).
fn macros() -> Vec<&'static str> {
    let re = Regex::new(r"(?s)%AM.*?%").unwrap();
    re.find_iter(&EVERYTHING).map(|m| m.as_str()).collect()
}

// Check some general syntax rules of the %ADD command.
#[test]
fn test_aperture_definition_syntax() {
    let re = Regex::new(r"^%ADD(\d+).+\*%$").unwrap();
    let mut checked_lines = 0;
    for line in EVERYTHING.split('\n').filter(|l| l.contains("%ADD")) {
        let captures = re.captures(line).expect(line);
        assert!(captures[1].parse::<u32>().unwrap() >= 10);
        checked_lines += 1;
    }
    assert!(checked_lines >= 3); // Sanity check if test works.
}

// Check some general syntax rules of the %AM command.
#[test]
fn test_aperture_macro_syntax() {
    let re = Regex::new(r"^%AM.+\*%$").unwrap();
    let mut checked_lines = 0;
    for line in EVERYTHING.split('\n').filter(|l| l.contains("%AM")) {
        assert!(re.is_match(line), "{line}");
        checked_lines += 1;
    }
    assert!(checked_lines >= 3); // Sanity check if test works.
}

// Check that regular polygon macros are never used. Some tools wrongly
// assume that the specified diameter is the *inner* diameter.
#[test]
fn test_not_using_regular_polygon() {
    // Sanity check if the regex works as expected (there will be circles).
    assert!(
        Regex::new(r"%ADD\d+C")
            .unwrap()
            .find_iter(&EVERYTHING)
            .count()
            >= 2
    );
    // Now check for absence of polygons.
    assert_eq!(
        Regex::new(r"%ADD\d+P")
            .unwrap()
            .find_iter(&EVERYTHING)
            .count(),
        0
    );
}

// Check that aperture macro variables are never used.
#[test]
fn test_not_using_macro_variables() {
    assert!(!EVERYTHING.contains("$1"));
}

// Check that arithmetic expressions are never used in aperture macros.
#[test]
fn test_not_using_arithmetic_expressions() {
    let macros = macros();
    for m in &macros {
        for c in ['(', ')', '+', 'x', 'X', '/'] {
            assert!(!m.contains(c), "{m}");
        }
    }
    // Sanity check if the test works properly.
    assert!(macros.len() >= 3);
}

// Check that circles in aperture macros do not specify the rotation.
#[test]
fn test_not_using_macro_circle_rotation() {
    let macros = macros();
    let mut found_circles = 0;
    for item in macros.iter().flat_map(|m| m.split('*')) {
        if item.starts_with("1,") {
            found_circles += 1;
            assert_eq!(item.matches(',').count(), 4, "{item}"); // No rotation!
        }
    }
    assert!(macros.len() >= 3);
    assert!(found_circles >= 3);
}

// Check that the macro primitive "Center Line, Code 21" is never used.
#[test]
fn test_not_using_macro_center_line() {
    let macros = macros();
    let center_lines = macros
        .iter()
        .flat_map(|m| m.split('*'))
        .filter(|item| item.starts_with("21,"))
        .count();
    assert!(macros.len() >= 3);
    assert_eq!(center_lines, 0);
}

#[test]
fn test_same_properties_and_attributes() {
    let mut l = GerberApertureList::new();
    assert_eq!(10, l.add_circle(uns(0), None));
    assert_eq!("%ADD10C,0.0*%\n", l.generate_string());
    assert_eq!(10, l.add_circle(uns(0), None));
    assert_eq!("%ADD10C,0.0*%\n", l.generate_string());
}

#[test]
fn test_different_properties() {
    let mut l = GerberApertureList::new();
    assert_eq!(10, l.add_circle(uns(0), None));
    assert_eq!("%ADD10C,0.0*%\n", l.generate_string());
    assert_eq!(11, l.add_circle(uns(100000), None));
    assert_eq!("%ADD10C,0.0*%\n%ADD11C,0.1*%\n", l.generate_string());
}

#[test]
fn test_different_attributes() {
    let mut l = GerberApertureList::new();
    assert_eq!(10, l.add_circle(uns(0), None));
    assert_eq!("%ADD10C,0.0*%\n", l.generate_string());

    let expected = "%ADD10C,0.0*%\n\
                    G04 #@! TA.AperFunction,Conductor*\n\
                    %ADD11C,0.0*%\n\
                    G04 #@! TD*\n";
    assert_eq!(11, l.add_circle(uns(0), Some(ApertureFunction::Conductor)));
    assert_eq!(expected, l.generate_string());
}

#[test]
fn test_different_properties_and_attributes() {
    let mut l = GerberApertureList::new();
    assert_eq!(10, l.add_circle(uns(0), None));
    assert_eq!("%ADD10C,0.0*%\n", l.generate_string());

    let expected = "%ADD10C,0.0*%\n\
                    G04 #@! TA.AperFunction,Conductor*\n\
                    %ADD11C,0.1*%\n\
                    G04 #@! TD*\n";
    assert_eq!(
        11,
        l.add_circle(uns(100000), Some(ApertureFunction::Conductor))
    );
    assert_eq!(expected, l.generate_string());
}

#[test]
fn test_attributes_get_deleted_at_end() {
    let mut l = GerberApertureList::new();

    // No attribute set -> nothing to clear.
    assert_eq!(10, l.add_circle(uns(0), None));
    assert_eq!("%ADD10C,0.0*%\n", l.generate_string());

    // Attribute set -> must be cleared at end.
    let expected = "%ADD10C,0.0*%\n\
                    G04 #@! TA.AperFunction,Conductor*\n\
                    %ADD11C,0.1*%\n\
                    G04 #@! TD*\n";
    assert_eq!(
        11,
        l.add_circle(uns(100000), Some(ApertureFunction::Conductor))
    );
    assert_eq!(expected, l.generate_string());

    // Last aperture has no attribute -> nothing to clear.
    let expected = "%ADD10C,0.0*%\n\
                    G04 #@! TA.AperFunction,Conductor*\n\
                    %ADD11C,0.1*%\n\
                    G04 #@! TD*\n\
                    %ADD12C,0.2*%\n";
    assert_eq!(12, l.add_circle(uns(200000), None));
    assert_eq!(expected, l.generate_string());
}

#[test]
fn test_circle_diameter_zero() {
    let mut l = GerberApertureList::new();
    assert_eq!(10, l.add_circle(uns(0), None));
    assert_eq!("%ADD10C,0.0*%\n", l.generate_string());
    // Set same aperture again to see if it gets reused.
    assert_eq!(10, l.add_circle(uns(0), None));
    assert_eq!("%ADD10C,0.0*%\n", l.generate_string());
    // Set another size to see if a new aperture gets created.
    assert_eq!(11, l.add_circle(uns(100000), None));
    assert_eq!("%ADD10C,0.0*%\n%ADD11C,0.1*%\n", l.generate_string());
}

#[test]
fn test_circle_diameter_non_zero() {
    let mut l = GerberApertureList::new();
    assert_eq!(10, l.add_circle(uns(1230000), None));
    assert_eq!("%ADD10C,1.23*%\n", l.generate_string());
    // Set same aperture again to see if it gets reused.
    assert_eq!(10, l.add_circle(uns(1230000), None));
    assert_eq!("%ADD10C,1.23*%\n", l.generate_string());
    // Set another size to see if a new aperture gets created.
    assert_eq!(11, l.add_circle(uns(100000), None));
    assert_eq!("%ADD10C,1.23*%\n%ADD11C,0.1*%\n", l.generate_string());
}

#[test]
fn test_obround_same_size() {
    let mut l = GerberApertureList::new();
    assert_eq!(
        10,
        l.add_obround(pos(1230000), pos(1230000), Angle::DEG0, None)
    );
    assert_eq!("%ADD10C,1.23*%\n", l.generate_string());
    // Set same aperture again to see if it gets reused.
    assert_eq!(
        10,
        l.add_obround(pos(1230000), pos(1230000), Angle::DEG90, None)
    );
    assert_eq!("%ADD10C,1.23*%\n", l.generate_string());
    // Set another size to see if a new aperture gets created.
    assert_eq!(
        11,
        l.add_obround(pos(100000), pos(100000), Angle::DEG90, None)
    );
    assert_eq!("%ADD10C,1.23*%\n%ADD11C,0.1*%\n", l.generate_string());
}

const ROT_0: [Angle; 3] = [Angle::new(-180_000_000), Angle::DEG0, Angle::DEG180];
const ROT_90: [Angle; 4] = [
    Angle::new(-270_000_000),
    Angle::new(-90_000_000),
    Angle::DEG90,
    Angle::DEG270,
];
const ROT_10: [Angle; 4] = [
    Angle::new(-350_000_000),
    Angle::new(-170_000_000),
    Angle::new(10_000_000),
    Angle::new(190_000_000),
];

/// Adds an aperture with each rotation to the same list and checks that
/// always aperture 10 with the expected definition results.
fn check_rotations(
    rotations: &[Angle],
    expected: &str,
    mut add: impl FnMut(&mut GerberApertureList, Angle) -> u32,
) {
    let mut l = GerberApertureList::new();
    for &rot in rotations {
        assert_eq!(10, add(&mut l, rot), "rotation {rot}");
        assert_eq!(expected, l.generate_string(), "rotation {rot}");
    }
}

fn obround(w: i64, h: i64) -> impl FnMut(&mut GerberApertureList, Angle) -> u32 {
    move |l, rot| l.add_obround(pos(w), pos(h), rot, None)
}

fn rect(w: i64, h: i64, r: i64) -> impl FnMut(&mut GerberApertureList, Angle) -> u32 {
    move |l, rot| l.add_rect(pos(w), pos(h), uns(r), rot, None)
}

fn octagon(w: i64, h: i64, r: i64) -> impl FnMut(&mut GerberApertureList, Angle) -> u32 {
    move |l, rot| l.add_octagon(pos(w), pos(h), uns(r), rot, None)
}

#[test]
fn test_high_obround_0deg() {
    check_rotations(&ROT_0, "%ADD10O,0.1X0.2*%\n", obround(100000, 200000));
}

#[test]
fn test_wide_obround_0deg() {
    check_rotations(&ROT_0, "%ADD10O,0.2X0.1*%\n", obround(200000, 100000));
}

#[test]
fn test_high_obround_90deg() {
    check_rotations(&ROT_90, "%ADD10O,0.2X0.1*%\n", obround(100000, 200000));
}

#[test]
fn test_wide_obround_90deg() {
    check_rotations(&ROT_90, "%ADD10O,0.1X0.2*%\n", obround(200000, 100000));
}

#[test]
fn test_high_obround_10deg() {
    // ATTENTION: The circles MUST NOT SPECIFY THE ROTATION!!! This would
    // cause troubles with some CAM software!!!
    let expected = "%AMROTATEDOBROUND10*\
                    1,1,0.1,0.004341,-0.02462*\
                    1,1,0.1,-0.004341,0.02462*\
                    20,1,0.1,0.004341,-0.02462,-0.004341,0.02462,0*%\n\
                    %ADD10ROTATEDOBROUND10*%\n";
    check_rotations(&ROT_10, expected, obround(100000, 150000));
}

#[test]
fn test_wide_obround_10deg() {
    // ATTENTION: The circles MUST NOT SPECIFY THE ROTATION!!! This would
    // cause troubles with some CAM software!!!
    let expected = "%AMROTATEDOBROUND10*\
                    1,1,0.1,-0.02462,-0.004341*\
                    1,1,0.1,0.02462,0.004341*\
                    20,1,0.1,-0.02462,-0.004341,0.02462,0.004341,0*%\n\
                    %ADD10ROTATEDOBROUND10*%\n";
    check_rotations(&ROT_10, expected, obround(150000, 100000));
}

#[test]
fn test_high_rect_0deg() {
    check_rotations(&ROT_0, "%ADD10R,0.1X0.15*%\n", rect(100000, 150000, 0));
}

#[test]
fn test_wide_rect_0deg() {
    check_rotations(&ROT_0, "%ADD10R,0.15X0.1*%\n", rect(150000, 100000, 0));
}

#[test]
fn test_high_rect_90deg() {
    check_rotations(&ROT_90, "%ADD10R,0.15X0.1*%\n", rect(100000, 150000, 0));
}

#[test]
fn test_wide_rect_90deg() {
    check_rotations(&ROT_90, "%ADD10R,0.1X0.15*%\n", rect(150000, 100000, 0));
}

#[test]
fn test_high_rect_10deg() {
    // ATTENTION: DO NOT USE THE CENTER LINE (Code 21)!!! It is buggy in some
    // CAM software!!!
    let expected = "%AMROTATEDRECT10*\
                    20,1,0.1,-0.075,0.0,0.075,0.0,100.0*%\n\
                    %ADD10ROTATEDRECT10*%\n";
    check_rotations(&ROT_10, expected, rect(100000, 150000, 0));
}

#[test]
fn test_wide_rect_10deg() {
    // ATTENTION: DO NOT USE THE CENTER LINE (Code 21)!!! It is buggy in some
    // CAM software!!!
    let expected = "%AMROTATEDRECT10*\
                    20,1,0.1,-0.075,0.0,0.075,0.0,10.0*%\n\
                    %ADD10ROTATEDRECT10*%\n";
    check_rotations(&ROT_10, expected, rect(150000, 100000, 0));
}

#[test]
fn test_high_rounded_rect_10deg() {
    // ATTENTION: DO NOT USE THE CENTER LINE (Code 21)!!! It is buggy in some
    // CAM software!!!
    let expected = "%AMROUNDEDRECT10*\
                    20,1,0.1,-0.055,0.0,0.055,0.0,100.0*\
                    20,1,0.06,-0.075,0.0,0.075,0.0,100.0*\
                    1,1,0.04,-0.019994,-0.059374*\
                    1,1,0.04,-0.039095,0.048955*\
                    1,1,0.04,0.019994,0.059374*\
                    1,1,0.04,0.039095,-0.048955*%\n\
                    %ADD10ROUNDEDRECT10*%\n";
    check_rotations(&ROT_10, expected, rect(100000, 150000, 20000));
}

#[test]
fn test_wide_rounded_rect_10deg() {
    // ATTENTION: DO NOT USE THE CENTER LINE (Code 21)!!! It is buggy in some
    // CAM software!!!
    let expected = "%AMROUNDEDRECT10*\
                    20,1,0.1,-0.055,0.0,0.055,0.0,10.0*\
                    20,1,0.06,-0.075,0.0,0.075,0.0,10.0*\
                    1,1,0.04,-0.059374,0.019994*\
                    1,1,0.04,0.048955,0.039095*\
                    1,1,0.04,0.059374,-0.019994*\
                    1,1,0.04,-0.048955,-0.039095*%\n\
                    %ADD10ROUNDEDRECT10*%\n";
    check_rotations(&ROT_10, expected, rect(150000, 100000, 20000));
}

// A rounded rect with a too large radius is converted into an obround.
#[test]
fn test_obround_rounded_rect_0deg() {
    check_rotations(&ROT_0, "%ADD10O,0.15X0.1*%\n", |l, rot| {
        let (w, h) = (pos(150000), pos(100000));
        assert_eq!(10, l.add_rect(w, h, uns(50000), rot, None));
        l.add_obround(w, h, rot, None)
    });
}

// A rounded rect with a too large radius is converted into an obround.
#[test]
fn test_obround_rounded_rect_10deg() {
    let expected = "%AMROTATEDOBROUND10*\
                    1,1,0.1,-0.02462,-0.004341*\
                    1,1,0.1,0.02462,0.004341*\
                    20,1,0.1,-0.02462,-0.004341,0.02462,0.004341,0*%\n\
                    %ADD10ROTATEDOBROUND10*%\n";
    check_rotations(&ROT_10, expected, |l, rot| {
        let (w, h) = (pos(150000), pos(100000));
        assert_eq!(10, l.add_rect(w, h, uns(60000), rot, None));
        l.add_obround(w, h, rot, None)
    });
}

/// Expected macro of a 0.5 x 0.5 mm octagon with the given rotation.
fn octagon_macro(rot: &str) -> String {
    format!(
        "%AMROTATEDOCTAGON10*\
         4,1,8,\
         0.25,0.103553,\
         0.103553,0.25,\
         -0.103553,0.25,\
         -0.25,0.103553,\
         -0.25,-0.103553,\
         -0.103553,-0.25,\
         0.103553,-0.25,\
         0.25,-0.103553,\
         0.25,0.103553,\
         {rot}*%\n\
         %ADD10ROTATEDOCTAGON10*%\n"
    )
}

/// Expected macro of a 0.9 x 0.5 mm octagon with the given rotation.
fn long_octagon_macro(rot: &str) -> String {
    format!(
        "%AMROTATEDOCTAGON10*\
         4,1,8,\
         0.45,0.103553,\
         0.303553,0.25,\
         -0.303553,0.25,\
         -0.45,0.103553,\
         -0.45,-0.103553,\
         -0.303553,-0.25,\
         0.303553,-0.25,\
         0.45,-0.103553,\
         0.45,0.103553,\
         {rot}*%\n\
         %ADD10ROTATEDOCTAGON10*%\n"
    )
}

// An octagon with height==width and rotations of a multiple of 45° is
// exported as the same aperture macro.
#[test]
fn test_regular_octagon_0deg() {
    let rotations: Vec<Angle> = (-7..=7).map(|i| deg(i * 45_000_000)).collect();
    // ATTENTION: DO NOT USE THE REGULAR POLYGON PRIMITIVE (P)!!! It is buggy
    // in some tools!
    check_rotations(
        &rotations,
        &octagon_macro("0.0"),
        octagon(500000, 500000, 0),
    );
}

// An octagon with height==width and rotations of a multiple of 45° plus 10°
// is exported as the same aperture macro.
#[test]
fn test_regular_octagon_10deg() {
    let rotations: Vec<Angle> = (-8..=7).map(|i| deg(i * 45_000_000 + 10_000_000)).collect();
    assert_eq!(rotations.first(), Some(&deg(-350_000_000)));
    assert_eq!(rotations.last(), Some(&deg(325_000_000)));
    check_rotations(
        &rotations,
        &octagon_macro("10.0"),
        octagon(500000, 500000, 0),
    );
}

#[test]
fn test_high_octagon_0deg() {
    check_rotations(
        &ROT_0,
        &long_octagon_macro("90.0"),
        octagon(500000, 900000, 0),
    );
}

#[test]
fn test_wide_octagon_0deg() {
    check_rotations(
        &ROT_0,
        &long_octagon_macro("0.0"),
        octagon(900000, 500000, 0),
    );
}

const ROT_100: [Angle; 4] = [
    Angle::new(-260_000_000),
    Angle::new(-80_000_000),
    Angle::new(100_000_000),
    Angle::new(280_000_000),
];

#[test]
fn test_high_octagon_100deg() {
    check_rotations(
        &ROT_100,
        &long_octagon_macro("10.0"),
        octagon(500000, 900000, 0),
    );
}

#[test]
fn test_wide_octagon_100deg() {
    check_rotations(
        &ROT_100,
        &long_octagon_macro("100.0"),
        octagon(900000, 500000, 0),
    );
}

// Outline apertures with rotations of -350° and 10° are exported as the same
// aperture macro.
#[test]
fn test_outline_10deg() {
    let p = StraightAreaPath::new(Path::new(vec![
        Vertex::at(Point::from_nm(-100000, 100000)),
        Vertex::at(Point::from_nm(500000, 700000)),
        Vertex::at(Point::from_nm(-500000, 700000)),
        Vertex::at(Point::from_nm(-100000, 100000)),
    ]))
    .unwrap();
    let expected = "%AMOUTLINE10*\
                    4,1,3,\
                    -0.1,0.1,\
                    0.5,0.7,\
                    -0.5,0.7,\
                    -0.1,0.1,\
                    10.0*%\n\
                    %ADD10OUTLINE10*%\n";
    check_rotations(&[deg(-350_000_000), deg(10_000_000)], expected, |l, rot| {
        l.add_outline(&p, rot, None)
    });
}

#[test]
fn test_component_main() {
    let mut l = GerberApertureList::new();
    // Note: The Gerber specs require exactly this aperture shape!!!
    let expected = "G04 #@! TA.AperFunction,ComponentMain*\n\
                    %ADD10C,0.3*%\n\
                    G04 #@! TD*\n";
    assert_eq!(10, l.add_component_main());
    assert_eq!(expected, l.generate_string());
}

#[test]
fn test_component_pin() {
    let mut l = GerberApertureList::new();
    // Note: The Gerber specs require exactly this aperture shape!!!
    let expected = "G04 #@! TA.AperFunction,ComponentPin*\n\
                    %ADD10C,0*%\n\
                    %ADD11P,0.36X4X0.0*%\n\
                    G04 #@! TD*\n";
    assert_eq!(10, l.add_component_pin(false));
    assert_eq!(11, l.add_component_pin(true));
    assert_eq!(expected, l.generate_string());
}
