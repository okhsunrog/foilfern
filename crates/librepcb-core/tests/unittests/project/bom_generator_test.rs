//! Project-to-BOM tests based on upstream's generation rules (no upstream
//! unit tests exist for BomGenerator).

use super::*;
use librepcb_core::attribute::{Attribute, AttributeList, AttributeType};
use librepcb_core::export::BomCsvWriter;
use librepcb_core::library::dev::{Part, PartList};
use librepcb_core::project::circuit::ComponentAssemblyOption;
use librepcb_core::project::{AssemblyVariantId, BomGenerator};
use librepcb_core::types::Point;

fn attrs(values: &[(&str, &str)]) -> AttributeList {
    values
        .iter()
        .map(|(key, value)| {
            Attribute::new(key.parse().unwrap(), AttributeType::String, *value, None).unwrap()
        })
        .collect::<Vec<_>>()
        .into()
}

fn part(mpn: &str, values: &[(&str, &str)]) -> Part {
    Part::new(
        mpn.parse().unwrap(),
        "Maker".parse().unwrap(),
        attrs(values),
    )
}

fn variant(p: &Project) -> AssemblyVariantId {
    AssemblyVariantId(p.circuit().assembly_variants().first().unwrap().uuid())
}

fn options(
    p: &mut Project,
    id: ComponentInstanceId,
    values: Vec<ComponentAssemblyOption>,
    value: &str,
) {
    let mut properties = p.circuit().component_instance(id).unwrap().properties();
    properties.assembly_options = values.into();
    properties.value = value.into();
    p.apply(Mutation::UpdateComponentInstance(properties))
        .unwrap();
}

fn option(device: Uuid, variant: AssemblyVariantId, parts: Vec<Part>) -> ComponentAssemblyOption {
    ComponentAssemblyOption::new(
        device,
        AttributeList::new(),
        BTreeSet::from([variant]),
        parts.into(),
    )
}

fn csv(generator: &BomGenerator<'_>, board: Option<&Board>, variant: AssemblyVariantId) -> String {
    let bom = generator.generate(board, variant).unwrap();
    let mut writer = BomCsvWriter::new(&bom);
    writer.set_include_non_mounted_parts(true);
    writer.generate_csv().unwrap().to_csv_string().unwrap()
}

#[test]
fn empty_bom_still_has_one_part_header() {
    let p = new_project();
    assert_eq!(
        csv(&BomGenerator::new(&p), None, variant(&p)),
        "Quantity,Designators,Package,Value,MPN,Manufacturer\n"
    );
}

#[test]
fn generic_groups_items_and_removes_redundant_value_tokens() {
    let mut f = board::Fixture::new();
    let variant = variant(&f.p);
    for id in [f.r1, f.r2] {
        options(
            &mut f.p,
            id,
            vec![option(f.lib_device, variant, vec![part("ABC+1", &[])])],
            " {{MPN}} Maker Test Component  10k ABC+1-suffix ",
        );
    }
    let bom = BomGenerator::new(&f.p).generate(None, variant).unwrap();
    assert_eq!(bom.total_assembled_parts_count(), 2);
    assert_eq!(bom.items().len(), 1);
    assert_eq!(bom.mpn_manufacturer_columns(), [(2, 3)]);
    assert_eq!(
        csv(&BomGenerator::new(&f.p), None, variant),
        "Quantity,Designators,Package,Value,MPN,Manufacturer\n2,\"R1, R2\",N/A,10k ABC+1-suffix,ABC+1,Maker\n"
    );
}

#[test]
fn board_filters_devices_and_keeps_unmounted_fallback_parts() {
    let mut f = board::Fixture::new();
    let variant = variant(&f.p);
    let other_variant = AssemblyVariantId(Uuid::new_random());
    options(
        &mut f.p,
        f.r1,
        vec![
            option(Uuid::new_random(), variant, vec![part("wrong-device", &[])]),
            option(f.lib_device, other_variant, vec![part("fallback", &[])]),
        ],
        "10k",
    );
    options(
        &mut f.p,
        f.r2,
        vec![option(f.lib_device, variant, vec![part("not-placed", &[])])],
        "22k",
    );
    let device = f.device(f.r1, Point::ORIGIN);
    assert!(
        device
            .parts(f.p.circuit(), Some(variant))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        device.parts(f.p.circuit(), None).unwrap()[0].mpn().as_str(),
        "fallback"
    );
    f.p.add_board_device(f.board, device).unwrap();
    assert_eq!(
        csv(&BomGenerator::new(&f.p), Some(f.board()), variant),
        "Quantity,Designators,Package,Value,MPN,Manufacturer\n0,R1,PKG,10k,fallback,Maker\n0,R2,,22k,not-placed,Maker\n"
    );
}

#[test]
fn custom_columns_alternative_parts_and_padding() {
    let mut f = board::Fixture::new();
    let variant = variant(&f.p);
    options(
        &mut f.p,
        f.r1,
        vec![option(
            f.lib_device,
            variant,
            vec![
                part("A", &[("SKU", "sku-{{MPN}}")]),
                part("B", &[("SKU", "sku-{{MPN}}")]),
            ],
        )],
        "{{MPN}} 10k",
    );
    options(
        &mut f.p,
        f.r2,
        vec![option(
            f.lib_device,
            variant,
            vec![part("C", &[("SKU", "sku-{{MPN}}")])],
        )],
        "22k",
    );
    let mut generator = BomGenerator::new(&f.p);
    generator.set_additional_attributes(vec!["SKU[]".into(), "NAME".into(), "UNKNOWN[]".into()]);
    let bom = generator.generate(None, variant).unwrap();
    assert_eq!(bom.mpn_manufacturer_columns(), [(3, 4), (8, 9)]);
    assert_eq!(
        csv(&generator, None, variant),
        "Quantity,Designators,Package,NAME,Value,MPN,Manufacturer,SKU,UNKNOWN,Value[2],MPN[2],Manufacturer[2],SKU[2],UNKNOWN[2]\n\
         1,R1,N/A,R1,10k,A,Maker,sku-A,,10k,B,Maker,sku-B,\n\
         1,R2,N/A,R2,22k,C,Maker,sku-C,,,,,,\n"
    );
}

#[test]
fn unmounted_alternatives_do_not_widen_the_header() {
    let mut f = board::Fixture::new();
    let variant = variant(&f.p);
    options(
        &mut f.p,
        f.r1,
        vec![option(
            f.lib_device,
            AssemblyVariantId(Uuid::new_random()),
            vec![part("A", &[]), part("B", &[])],
        )],
        "10k",
    );
    let bom = BomGenerator::new(&f.p).generate(None, variant).unwrap();
    assert_eq!(bom.columns(), ["Package", "Value", "MPN", "Manufacturer"]);
    assert_eq!(bom.items()[0].attributes(), ["N/A", "10k", "A", "Maker"]);
    assert!(!bom.items()[0].is_mount());
}

#[test]
fn empty_part_uses_option_attributes() {
    let mut f = board::Fixture::new();
    let variant = variant(&f.p);
    let option = ComponentAssemblyOption::new(
        f.lib_device,
        attrs(&[("VALUE", "option value"), ("SKU", "option sku")]),
        BTreeSet::from([variant]),
        PartList::new(),
    );
    options(&mut f.p, f.r1, vec![option], "component value");
    let device = f.device(f.r1, Point::ORIGIN);
    let parts = device.parts(f.p.circuit(), Some(variant)).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(
        parts[0].attributes().by_name("SKU", true).unwrap().value(),
        "option sku"
    );
    f.p.add_board_device(f.board, device).unwrap();
    let mut generator = BomGenerator::new(&f.p);
    generator.set_additional_attributes(vec!["SKU[]".into()]);
    assert_eq!(
        csv(&generator, Some(f.board()), variant),
        "Quantity,Designators,Package,Value,MPN,Manufacturer,SKU\n1,R1,PKG,option value,,,option sku\n0,R2,,,,,\n"
    );
}

#[test]
fn schematic_only_components_without_parts_are_skipped() {
    use librepcb_core::library::LibraryBaseElement;
    let mut p = new_project();
    let (mut lib, variant_id) = library_component(Uuid::new_random(), 0, Uuid::new_random());
    lib.set_schematic_only(true);
    let id = lib.metadata().uuid();
    p.add_library_component(lib).unwrap();
    let component = component_instance(&p, id, variant_id, "FRAME1");
    p.add_component_instance(component).unwrap();
    assert!(
        BomGenerator::new(&p)
            .generate(None, variant(&p))
            .unwrap()
            .items()
            .is_empty()
    );
}

#[test]
fn packages_without_assembly_are_skipped_only_when_unmounted() {
    use librepcb_core::library::pkg::AssemblyType;

    let mut f = board::Fixture::new();
    let mut package = f.p.remove_library_package(&f.lib_package).unwrap();
    package.set_assembly_type(AssemblyType::None);
    f.p.add_library_package(package).unwrap();
    let variant = variant(&f.p);
    let device = f.device(f.r1, Point::ORIGIN);
    f.p.add_board_device(f.board, device).unwrap();
    let bom = BomGenerator::new(&f.p)
        .generate(Some(f.board()), variant)
        .unwrap();
    assert!(
        bom.items()
            .iter()
            .all(|item| !item.designators().iter().any(|d| d == "R1"))
    );
    options(
        &mut f.p,
        f.r1,
        vec![option(f.lib_device, variant, vec![])],
        "logo",
    );
    let bom = BomGenerator::new(&f.p)
        .generate(Some(f.board()), variant)
        .unwrap();
    assert_eq!(bom.items()[0].designators(), ["R1"]);
    assert!(bom.items()[0].is_mount());
}
