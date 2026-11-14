//! Tests of `Bom` and `BomCsvWriter` (no upstream unit tests; the expected
//! output follows the upstream implementation).

use foilfern_core::export::{Bom, BomCsvWriter};

fn attrs(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}

fn create_bom() -> Bom {
    let mut bom = Bom::new(attrs(&["Value", "MPN"]), vec![(1, 1)]);
    bom.add_item("R10", attrs(&["1k", "RC0603"]), true);
    bom.add_item("C1", attrs(&["100n", "\"X7R\", 50V"]), true);
    bom.add_item("R2", attrs(&["1k", "RC0603"]), true);
    bom.add_item("r1", attrs(&["1k", "RC0603"]), true);
    bom.add_item("A1", attrs(&["DNP", ""]), false);
    bom.add_item("R3", attrs(&["1k", "RC0603"]), false);
    bom
}

#[test]
fn test_items() {
    let bom = create_bom();
    let designators: Vec<Vec<String>> = bom
        .items()
        .iter()
        .map(|item| item.designators().to_vec())
        .collect();
    assert_eq!(
        designators,
        [vec!["C1"], vec!["r1", "R2", "R10"], vec!["A1"], vec!["R3"],]
    );
    assert_eq!(bom.assembled_rows_count(), 2);
    assert_eq!(bom.total_assembled_parts_count(), 4);
    assert_eq!(bom.mpn_manufacturer_columns(), [(1, 1)]);
}

#[test]
fn test_csv() {
    let bom = create_bom();
    let mut writer = BomCsvWriter::new(&bom);
    assert_eq!(
        "Quantity,Designators,Value,MPN\n\
         1,C1,100n,\"\"\"X7R\"\", 50V\"\n\
         3,\"r1, R2, R10\",1k,RC0603\n",
        writer.generate_csv().unwrap().to_csv_string().unwrap()
    );

    writer.set_include_non_mounted_parts(true);
    assert_eq!(
        "Quantity,Designators,Value,MPN\n\
         1,C1,100n,\"\"\"X7R\"\", 50V\"\n\
         3,\"r1, R2, R10\",1k,RC0603\n\
         0,A1,DNP,\n\
         0,R3,1k,RC0603\n",
        writer.generate_csv().unwrap().to_csv_string().unwrap()
    );
}
