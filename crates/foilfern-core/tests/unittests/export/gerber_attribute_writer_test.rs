//! Port of tests/unittests/core/export/gerberattributewritertest.cpp.

use foilfern_core::export::{ApertureFunction, GerberAttribute, GerberAttributeWriter};

fn function_conductor() -> GerberAttribute {
    GerberAttribute::aperture_function(ApertureFunction::Conductor)
}

fn function_smd_pad_copper_defined() -> GerberAttribute {
    GerberAttribute::aperture_function(ApertureFunction::SmdPadCopperDefined)
}

fn component_u1() -> GerberAttribute {
    GerberAttribute::object_component("U1")
}

fn component_u2() -> GerberAttribute {
    GerberAttribute::object_component("U2")
}

#[test]
fn test_empty_dict_empty_attributes() {
    let mut w = GerberAttributeWriter::new();
    assert_eq!("", w.set_attributes(&[]));
}

#[test]
fn test_empty_dict_non_empty_attributes() {
    let mut w = GerberAttributeWriter::new();
    let expected = "G04 #@! TA.AperFunction,Conductor*\n\
                    G04 #@! TO.C,U1*\n";
    assert_eq!(
        expected,
        w.set_attributes(&[function_conductor(), component_u1()])
    );
}

#[test]
fn test_non_empty_dict_empty_attributes() {
    let mut w = GerberAttributeWriter::new();
    w.set_attributes(&[function_conductor(), component_u1()]);
    assert_eq!("G04 #@! TD*\n", w.set_attributes(&[]));
}

#[test]
fn test_non_empty_dict_same_attributes() {
    let mut w = GerberAttributeWriter::new();
    w.set_attributes(&[function_conductor(), component_u1()]);
    assert_eq!(
        "",
        w.set_attributes(&[function_conductor(), component_u1()])
    );
}

#[test]
fn test_non_empty_dict_partly_different_attributes() {
    let mut w = GerberAttributeWriter::new();
    w.set_attributes(&[function_conductor(), component_u1()]);
    assert_eq!(
        "G04 #@! TO.C,U2*\n",
        w.set_attributes(&[function_conductor(), component_u2()])
    );
}

#[test]
fn test_non_empty_dict_fully_different_attributes() {
    let mut w = GerberAttributeWriter::new();
    w.set_attributes(&[function_conductor(), component_u1()]);
    let expected = "G04 #@! TA.AperFunction,SMDPad,CuDef*\n\
                    G04 #@! TO.C,U2*\n";
    assert_eq!(
        expected,
        w.set_attributes(&[function_smd_pad_copper_defined(), component_u2()])
    );
}

#[test]
fn test_more_attributes() {
    let mut w = GerberAttributeWriter::new();
    w.set_attributes(&[function_conductor()]);
    assert_eq!(
        "G04 #@! TO.C,U1*\n",
        w.set_attributes(&[component_u1(), function_conductor()])
    );
}

#[test]
fn test_less_attributes() {
    let mut w = GerberAttributeWriter::new();
    w.set_attributes(&[function_conductor(), component_u1()]);
    assert_eq!(
        "G04 #@! TD.AperFunction*\n",
        w.set_attributes(&[component_u1()])
    );
}
