//! Native supplier import vectors are synthetic; live snapshots stay outside the repository.

use foilfern_core::fileio::FileSystem;
use foilfern_core::library::LibraryBaseElement;
use foilfern_import::easyeda;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn source() -> Value {
    json!({
        "dataStr":{"head":{"x":100,"y":100,"c_para":{"pre":"R?","Manufacturer Part":"TEST-123","Manufacturer":"Test maker","Supplier Part":"C123"}},
            "shape":[
                "P~show~0~1~90.25~100~180~pin1~0^^90.25~100^^M90.25,100h5~#000^^1~90~100~0~A~start",
                "P~show~0~2~110~100~0~pin2~0^^110~100^^M110,100h-5~#000^^1~110~100~0~B~start",
                "R~95~95~0~0~10~10~#000~0.5~0~none~box"
            ]},
        "packageDetail":{"dataStr":{"head":{"x":200,"y":200,"license":"Source metadata must be retained"},
            "shape":[
                "PAD~RECT~195.125~200~2~3~1~~1~0~~0~pad1~0",
                "PAD~OVAL~205~200~2~3~1~~2~0~~0~pad2~0"
            ]}}
    })
}

#[test]
fn native_mapping_preserves_fractional_geometry_and_source_identity() {
    let raw = source();
    let hash = "a".repeat(64);
    let mut part = easyeda::convert(&raw, "C123", &hash).unwrap();
    let repeated = easyeda::convert(&raw, "C123", &hash).unwrap();
    assert_eq!(
        part.device.metadata().uuid(),
        repeated.device.metadata().uuid()
    );
    assert_eq!(part.symbol.pins().len(), 2);
    assert_eq!(part.package.pads().len(), 2);
    assert_eq!(part.device.pad_signal_map().len(), 2);
    assert!(
        part.symbol
            .pins()
            .iter()
            .any(|pin| pin.position().x.to_nm() == -2_476_500)
    );
    assert!(
        part.package
            .footprints()
            .get(0)
            .unwrap()
            .pads()
            .iter()
            .any(|pad| pad.pad().position().x.to_nm() == -1_238_250)
    );
    let selected = part.device.parts().get(0).unwrap();
    assert_eq!(selected.mpn().as_str(), "TEST-123");
    assert_eq!(selected.manufacturer().as_str(), "Test maker");
    assert_eq!(
        selected.attributes().by_name("LCSC", true).unwrap().value(),
        "C123"
    );
    let saved: Value =
        serde_json::from_slice(&part.device.directory().read("easyeda-source.json").unwrap())
            .unwrap();
    assert_eq!(saved, raw);
    let temporary = super::helpers::TempDir::new();
    let library = super::helpers::create_library(&temporary);
    part.install(&library).unwrap();
    let counts = super::helpers::open_all_elements(&library);
    for kind in ["sym", "cmp", "pkg", "dev"] {
        assert_eq!(counts[kind], 1);
    }
    assert!(part.install(&library).is_err());
    super::helpers::check_library_with_upstream_cli(&library);
}

#[test]
fn malformed_geometry_and_incomplete_mapping_are_rejected() {
    for (area, index, value) in [
        (
            "symbol",
            0,
            "P~show~0~1~NaN~100~180~pin1~0^^90.25~100^^M90.25,100h5~#000^^1~90~100~0~A~start",
        ),
        (
            "symbol",
            1,
            "P~show~0~1~110~100~0~pin2~0^^110~100^^M110,100h-5~#000^^1~110~100~0~B~start",
        ),
        ("footprint", 0, "PAD~RECT~195~200~0~3~1~~1~0~~0~pad1~0"),
        ("footprint", 1, "PAD~OVAL~205~200~2~3~1~~3~0~~0~pad2~0"),
        ("footprint", 0, "UNSUPPORTED~something"),
    ] {
        let mut raw = source();
        if area == "symbol" {
            raw["dataStr"]["shape"][index] = json!(value);
        } else {
            raw["packageDetail"]["dataStr"]["shape"][index] = json!(value);
        }
        assert!(
            easyeda::convert(&raw, "C123", &"a".repeat(64)).is_err(),
            "{area} {value}"
        );
    }
}

#[test]
#[ignore = "requires an externally cached real EasyEDA source file"]
fn external_source_converts_to_native_library() {
    let path = std::env::var("FOILFERN_EASYEDA_SOURCE").expect("set FOILFERN_EASYEDA_SOURCE");
    let bytes = std::fs::read(path).unwrap();
    let envelope: Value = serde_json::from_slice(&bytes).unwrap();
    let raw = &envelope["result"];
    let lcsc = raw["dataStr"]["head"]["c_para"]["Supplier Part"]
        .as_str()
        .unwrap();
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut part = easyeda::convert(raw, lcsc, &hash).unwrap();
    if let Ok(file) = std::env::var("FOILFERN_EASYEDA_MODEL") {
        let file = foilfern_core::fileio::FilePath::new(file).unwrap();
        let bytes = std::fs::read(file.as_path()).unwrap();
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let reference = easyeda::model_reference(raw).unwrap().unwrap();
        part.attach_step(lcsc, &reference, &file, &hash).unwrap();
        assert_eq!(part.package.models().len(), 1);
    }
    let temporary = super::helpers::TempDir::new();
    let library = super::helpers::create_library(&temporary);
    part.install(&library).unwrap();
    assert_eq!(part.symbol.pins().len(), part.package.pads().len());
    eprintln!(
        "{lcsc}: {} symbol pins, {} package pads, {} mapping entries; warnings {:?}",
        part.symbol.pins().len(),
        part.package.pads().len(),
        part.device.pad_signal_map().len(),
        part.warnings
    );
    super::helpers::open_all_elements(&library);
    super::helpers::check_library_with_upstream_cli(&library);
}

#[test]
fn step_attachment_checks_geometry_hash_and_native_pose() {
    let mut raw = source();
    let node = json!({"attrs":{"uuid":"a4b96ad857dc48c08dab3d0efdf20aec","title":"Test model","c_origin":"201,202","c_rotation":"10,20,30","z":"100"}});
    raw["packageDetail"]["dataStr"]["shape"]
        .as_array_mut()
        .unwrap()
        .push(json!(format!("SVGNODE~{node}")));
    let reference = easyeda::model_reference(&raw).unwrap().unwrap();
    assert_eq!(reference.position_mm, [0.254, -0.508, 2.54]);
    assert_eq!(reference.rotation_deg, [10.0, 20.0, 30.0]);
    let mut malformed = raw.clone();
    let shapes = malformed["packageDetail"]["dataStr"]["shape"]
        .as_array_mut()
        .unwrap();
    shapes.push(shapes.last().unwrap().clone());
    assert!(easyeda::model_reference(&malformed).is_err());
    let mut malformed = raw.clone();
    let mut missing_origin = node.clone();
    missing_origin["attrs"]
        .as_object_mut()
        .unwrap()
        .remove("c_origin");
    *malformed["packageDetail"]["dataStr"]["shape"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap() = json!(format!("SVGNODE~{missing_origin}"));
    assert!(easyeda::model_reference(&malformed).is_err());
    let mut part = easyeda::convert(&raw, "C123", &"a".repeat(64)).unwrap();
    let file = foilfern_core::fileio::FilePath::new(
        super::helpers::test_data_dir().join("unittests/librepcbcommon/OccModelTest/model.step"),
    )
    .unwrap();
    let bytes = std::fs::read(file.as_path()).unwrap();
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(
        part.attach_step("C123", &reference, &file, &"0".repeat(64))
            .is_err()
    );
    assert!(part.package.models().is_empty());
    part.attach_step("C123", &reference, &file, &hash).unwrap();
    assert_eq!(part.package.models().len(), 1);
    let fp = part.package.footprints().get(0).unwrap();
    assert_eq!(fp.model_position().0.to_nm(), 254_000);
    assert_eq!(fp.model_position().1.to_nm(), -508_000);
    assert_eq!(fp.model_position().2.to_nm(), 2_540_000);
    assert_eq!(fp.model_rotation().2.to_deg(), 30.0);
    let model = part.package.models().get(0).unwrap();
    assert_eq!(
        part.package.directory().read(&model.file_name()).unwrap(),
        bytes
    );
    let temporary = super::helpers::TempDir::new();
    let library = super::helpers::create_library(&temporary);
    part.install(&library).unwrap();
    super::helpers::open_all_elements(&library);
}
