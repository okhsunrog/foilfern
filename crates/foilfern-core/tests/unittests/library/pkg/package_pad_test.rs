//! Tests of libs/librepcb/core/library/pkg/packagepad.{h,cpp} and
//! packagemodel.{h,cpp} (no upstream tests exist).

use foilfern_core::library::pkg::{PackageModel, PackageModelList, PackagePad, PackagePadList};
use foilfern_core::serialization::DeserializeObject;
use foilfern_core::types::{CircuitIdentifier, ElementName, Uuid};

use crate::geometry::{assert_roundtrip, parse, serialize_str};

#[test]
fn test_package_pad_construct_from_sexpression() {
    let obj = PackagePad::deserialize(&parse(
        "(pad 7040952d-7016-49cd-8c3e-6078ecca98b9 (name \"GND\"))",
    ))
    .unwrap();
    assert_eq!(
        obj.uuid(),
        "7040952d-7016-49cd-8c3e-6078ecca98b9"
            .parse::<Uuid>()
            .unwrap()
    );
    assert_eq!(obj.name().as_str(), "GND");
}

#[test]
fn test_package_pad_invalid_name() {
    let result = PackagePad::deserialize(&parse(
        "(pad 7040952d-7016-49cd-8c3e-6078ecca98b9 (name \"\"))",
    ));
    assert!(result.is_err());
}

#[test]
fn test_package_pad_serialize() {
    let mut obj = PackagePad::new(
        "7040952d-7016-49cd-8c3e-6078ecca98b9".parse().unwrap(),
        CircuitIdentifier::new("1").unwrap(),
    );
    assert!(obj.set_name(CircuitIdentifier::new("VCC").unwrap()));
    assert!(!obj.set_name(CircuitIdentifier::new("VCC").unwrap()));
    assert_eq!(
        serialize_str(&obj, "pad"),
        "(pad 7040952d-7016-49cd-8c3e-6078ecca98b9 (name \"VCC\"))\n"
    );
    assert_roundtrip(&obj);
}

#[test]
fn test_package_pad_list_by_name() {
    let list = PackagePadList::from(vec![
        PackagePad::new(Uuid::new_random(), CircuitIdentifier::new("A").unwrap()),
        PackagePad::new(Uuid::new_random(), CircuitIdentifier::new("B").unwrap()),
    ]);
    assert_eq!(list.index_of_name("b", false), Some(1));
    assert_eq!(list.index_of_name("b", true), None);
}

#[test]
fn test_package_model_serialize() {
    let obj = PackageModel::new(
        "7040952d-7016-49cd-8c3e-6078ecca98b9".parse().unwrap(),
        ElementName::new("Model").unwrap(),
    );
    assert_eq!(obj.file_name(), "7040952d-7016-49cd-8c3e-6078ecca98b9.step");
    assert_eq!(
        serialize_str(&obj, "3d_model"),
        "(3d_model 7040952d-7016-49cd-8c3e-6078ecca98b9 (name \"Model\"))\n"
    );
    assert_roundtrip(&obj);
    let list = PackageModelList::deserialize(&parse(
        "(package (3d_model 7040952d-7016-49cd-8c3e-6078ecca98b9 (name \"Model\")))",
    ))
    .unwrap();
    assert_eq!(list.as_slice(), [obj]);
}
