//! Port of tests/unittests/core/export/gerberattributetest.cpp.

use foilfern_core::export::{
    ApertureFunction as F, BoardSide, CopperSide, GerberAttribute as A, MountType, Polarity,
};
use foilfern_core::types::{Angle, Uuid};

use super::{date_utc_plus_1, deg};

#[test]
fn test_unset() {
    assert_eq!("G04 #@! TD*\n", A::unset("").to_gerber_string());
    assert_eq!("G04 #@! TD.Foo*\n", A::unset(".Foo").to_gerber_string());
}

#[test]
fn test_file_generation_software() {
    assert_eq!(
        "G04 #@! TF.GenerationSoftware,Foo|Bar?!aou,Foo Bar,v1.0*\n",
        A::file_generation_software("Foo,|Bar%?!\\äöü", "Foo Bar", "v1.0").to_gerber_string()
    );
}

#[test]
fn test_file_creation_date() {
    assert_eq!(
        "G04 #@! TF.CreationDate,2000-02-01T01:02:03+01:00*\n",
        A::file_creation_date(&date_utc_plus_1(2000, 2, 1, 1, 2, 3, 4)).to_gerber_string()
    );
}

#[test]
fn test_file_project_id() {
    let uuid: Uuid = "bdf7bea5-b88e-41b2-be85-c1604e8ddfca".parse().unwrap();
    assert_eq!(
        "G04 #@! TF.ProjectId,Project Name,bdf7bea5-b88e-41b2-be85-c1604e8ddfca,rev-1.0*\n",
        A::file_project_id("Project Name", &uuid, "rev-1.0").to_gerber_string()
    );
}

#[test]
fn test_file_part_single() {
    assert_eq!(
        "G04 #@! TF.Part,Single*\n",
        A::file_part_single().to_gerber_string()
    );
}

#[test]
fn test_file_same_coordinates() {
    assert_eq!(
        "G04 #@! TF.SameCoordinates*\n",
        A::file_same_coordinates("").to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.SameCoordinates,asdf*\n",
        A::file_same_coordinates("asdf").to_gerber_string()
    );
}

#[test]
fn test_file_function_profile() {
    assert_eq!(
        "G04 #@! TF.FileFunction,Profile,P*\n",
        A::file_function_profile(true).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.FileFunction,Profile,NP*\n",
        A::file_function_profile(false).to_gerber_string()
    );
}

#[test]
fn test_file_function_copper() {
    assert_eq!(
        "G04 #@! TF.FileFunction,Copper,L1,Top*\n",
        A::file_function_copper(1, CopperSide::Top).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.FileFunction,Copper,L5,Inr*\n",
        A::file_function_copper(5, CopperSide::Inner).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.FileFunction,Copper,L42,Bot*\n",
        A::file_function_copper(42, CopperSide::Bottom).to_gerber_string()
    );
}

#[test]
fn test_file_function_solder_mask() {
    assert_eq!(
        "G04 #@! TF.FileFunction,Soldermask,Top*\n",
        A::file_function_solder_mask(BoardSide::Top).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.FileFunction,Soldermask,Bot*\n",
        A::file_function_solder_mask(BoardSide::Bottom).to_gerber_string()
    );
}

#[test]
fn test_file_function_legend() {
    assert_eq!(
        "G04 #@! TF.FileFunction,Legend,Top*\n",
        A::file_function_legend(BoardSide::Top).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.FileFunction,Legend,Bot*\n",
        A::file_function_legend(BoardSide::Bottom).to_gerber_string()
    );
}

#[test]
fn test_file_function_paste() {
    assert_eq!(
        "G04 #@! TF.FileFunction,Paste,Top*\n",
        A::file_function_paste(BoardSide::Top).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.FileFunction,Paste,Bot*\n",
        A::file_function_paste(BoardSide::Bottom).to_gerber_string()
    );
}

#[test]
fn test_file_function_glue() {
    assert_eq!(
        "G04 #@! TF.FileFunction,Glue,Top*\n",
        A::file_function_glue(BoardSide::Top).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.FileFunction,Glue,Bot*\n",
        A::file_function_glue(BoardSide::Bottom).to_gerber_string()
    );
}

#[test]
fn test_file_function_plated_through_hole_excellon() {
    assert_eq!(
        "; #@! TF.FileFunction,Plated,2,5,PTH\n",
        A::file_function_plated_through_hole(2, 5).to_excellon_string()
    );
}

#[test]
fn test_file_function_non_plated_through_hole_excellon() {
    assert_eq!(
        "; #@! TF.FileFunction,NonPlated,2,5,NPTH\n",
        A::file_function_non_plated_through_hole(2, 5).to_excellon_string()
    );
}

#[test]
fn test_file_function_mixed_plating_excellon() {
    assert_eq!(
        "; #@! TF.FileFunction,MixedPlating,2,5\n",
        A::file_function_mixed_plating(2, 5).to_excellon_string()
    );
}

#[test]
fn test_file_polarity() {
    assert_eq!(
        "G04 #@! TF.FilePolarity,Positive*\n",
        A::file_polarity(Polarity::Positive).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TF.FilePolarity,Negative*\n",
        A::file_polarity(Polarity::Negative).to_gerber_string()
    );
}

#[test]
fn test_file_md5() {
    assert_eq!(
        "G04 #@! TF.MD5,ASDF*\n",
        A::file_md5("ASDF").to_gerber_string()
    );
}

#[test]
fn test_aperture_function() {
    let data = [
        ("Profile", F::Profile),
        ("Conductor", F::Conductor),
        ("NonConductor", F::NonConductor),
        ("ComponentPad", F::ComponentPad),
        ("SMDPad,CuDef", F::SmdPadCopperDefined),
        ("SMDPad,SMDef", F::SmdPadSolderMaskDefined),
        ("BGAPad,CuDef", F::BgaPadCopperDefined),
        ("BGAPad,SMDef", F::BgaPadSolderMaskDefined),
        ("ConnectorPad", F::ConnectorPad),
        ("HeatsinkPad", F::HeatsinkPad),
        ("ViaPad", F::ViaPad),
        ("TestPad", F::TestPad),
        ("FiducialPad,Local", F::FiducialPadLocal),
        ("FiducialPad,Global", F::FiducialPadGlobal),
    ];
    for (expected, function) in data {
        assert_eq!(
            format!("G04 #@! TA.AperFunction,{expected}*\n"),
            A::aperture_function(function).to_gerber_string()
        );
    }
}

#[test]
fn test_aperture_function_excellon() {
    let data = [
        ("ViaDrill", F::ViaDrill),
        ("ComponentDrill", F::ComponentDrill),
        ("ComponentDrill,PressFit", F::ComponentDrillPressFit),
        ("MechanicalDrill", F::MechanicalDrill),
    ];
    for (expected, function) in data {
        assert_eq!(
            format!("; #@! TA.AperFunction,{expected}\n"),
            A::aperture_function(function).to_excellon_string()
        );
    }
}

#[test]
fn test_aperture_function_mixed_plating_drill_excellon() {
    let data = [
        ("NonPlated,NPTH,ViaDrill", false, F::ViaDrill),
        ("NonPlated,NPTH,ComponentDrill", false, F::ComponentDrill),
        (
            "NonPlated,NPTH,ComponentDrill,PressFit",
            false,
            F::ComponentDrillPressFit,
        ),
        ("NonPlated,NPTH,MechanicalDrill", false, F::MechanicalDrill),
        ("Plated,PTH,ViaDrill", true, F::ViaDrill),
        ("Plated,PTH,ComponentDrill", true, F::ComponentDrill),
        (
            "Plated,PTH,ComponentDrill,PressFit",
            true,
            F::ComponentDrillPressFit,
        ),
        ("Plated,PTH,MechanicalDrill", true, F::MechanicalDrill),
    ];
    for (expected, plated, function) in data {
        assert_eq!(
            format!("; #@! TA.AperFunction,{expected}\n"),
            A::aperture_function_mixed_plating_drill(plated, function).to_excellon_string()
        );
    }
}

#[test]
fn test_object_net() {
    assert_eq!("G04 #@! TO.N,*\n", A::object_net("").to_gerber_string());
    assert_eq!(
        "G04 #@! TO.N,N/C*\n",
        A::object_net("N/C").to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TO.N,Foo Bar*\n",
        A::object_net("Foo Bar").to_gerber_string()
    );
}

#[test]
fn test_object_component() {
    assert_eq!(
        "G04 #@! TO.C,C7*\n",
        A::object_component("C7").to_gerber_string()
    );
}

#[test]
fn test_object_pin() {
    assert_eq!(
        "G04 #@! TO.P,C7,42*\n",
        A::object_pin("C7", "42", "").to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TO.P,C7,42,VCC*\n",
        A::object_pin("C7", "42", "VCC").to_gerber_string()
    );
}

#[test]
fn test_component_rotation() {
    assert_eq!(
        "G04 #@! TO.CRot,-90.0*\n",
        A::component_rotation(-Angle::DEG90).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TO.CRot,0.123456*\n",
        A::component_rotation(deg(123456)).to_gerber_string()
    );
}

const ESCAPED: &str = "Foo \u{00E4} \\u005C \\u0025 \\u002A \\u002C*\n";
const UNESCAPED: &str = "Foo\n\u{00E4}\r\n\\ % * ,";

#[test]
fn test_component_manufacturer() {
    assert_eq!(
        format!("G04 #@! TO.CMfr,{ESCAPED}"),
        A::component_manufacturer(UNESCAPED).to_gerber_string()
    );
}

#[test]
fn test_component_mpn() {
    assert_eq!(
        format!("G04 #@! TO.CMPN,{ESCAPED}"),
        A::component_mpn(UNESCAPED).to_gerber_string()
    );
}

#[test]
fn test_component_value() {
    assert_eq!(
        format!("G04 #@! TO.CVal,{ESCAPED}"),
        A::component_value(UNESCAPED).to_gerber_string()
    );
}

#[test]
fn test_component_mount_type() {
    assert_eq!(
        "G04 #@! TO.CMnt,TH*\n",
        A::component_mount_type(MountType::Tht).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TO.CMnt,SMD*\n",
        A::component_mount_type(MountType::Smt).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TO.CMnt,Fiducial*\n",
        A::component_mount_type(MountType::Fiducial).to_gerber_string()
    );
    assert_eq!(
        "G04 #@! TO.CMnt,Other*\n",
        A::component_mount_type(MountType::Other).to_gerber_string()
    );
}

#[test]
fn test_component_footprint() {
    assert_eq!(
        format!("G04 #@! TO.CFtp,{ESCAPED}"),
        A::component_footprint(UNESCAPED).to_gerber_string()
    );
}
