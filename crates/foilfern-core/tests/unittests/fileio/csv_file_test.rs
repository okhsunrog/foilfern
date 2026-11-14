//! Port of tests/unittests/core/fileio/csvfiletest.cpp.

use foilfern_core::fileio::{CsvFile, file_utils};

use crate::helpers::TempDir;

const EXPECTED: &str = "# Foo\n\
                        # Bar\n\
                        \n\
                        Column,Column With Space,\"With,Comma\",\"\"\"With Quotes\"\"\"\n\
                        ,,,\n\
                        Value,Value With Space,\"With,Comma\",\"\"\"With Quotes\"\"\"\n\
                        -1.2345,Foo Bar, spaces around ,äöü\n";

fn populated() -> CsvFile {
    let mut f = CsvFile::new();
    f.set_comment("Foo\nBar");
    f.set_header([
        "Column",
        "Column With Space",
        "With,Comma",
        "\"With Quotes\"",
    ]);
    f.add_value(["", "", "", ""]).unwrap();
    f.add_value(["Value", "Value With Space", "With,Comma", "\"With Quotes\""])
        .unwrap();
    f.add_value(["-1.2345", "Foo\r\nBar", " spaces around ", "äöü"])
        .unwrap();
    f
}

#[test]
fn test_default_constructor() {
    let f = CsvFile::new();
    assert_eq!(f.comment(), "");
    assert!(f.header().is_empty());
    assert!(f.values().is_empty());
    assert_eq!(f.to_csv_string().unwrap(), "");
}

#[test]
fn test_comment_only() {
    let mut f = CsvFile::new();
    f.set_comment("Foo\n\nBar");
    assert_eq!(f.to_csv_string().unwrap(), "# Foo\n#\n# Bar\n\n");
}

#[test]
fn test_header_only() {
    let mut f = CsvFile::new();
    f.set_header(["Foo", "Bar"]);
    assert_eq!(f.to_csv_string().unwrap(), "Foo,Bar\n");
}

#[test]
fn test_set_header_clears_values() {
    let mut f = CsvFile::new();
    f.set_header(["Foo", "Bar"]);
    f.add_value(["V1", "V2"]).unwrap();
    assert_eq!(f.values().len(), 1);
    f.set_header(["Foo", "Bar"]);
    assert!(f.values().is_empty());
}

#[test]
fn test_add_values_throws_exception_if_no_header_set() {
    let mut f = CsvFile::new();
    assert!(f.add_value(["V1", "V2"]).is_err());
}

#[test]
fn test_add_values_throws_exception_if_wrong_count() {
    let mut f = CsvFile::new();
    f.set_header(["Foo"]);
    assert!(f.add_value(["V1", "V2"]).is_err());
}

#[test]
fn test_to_string_with_quoting_and_escaping() {
    assert_eq!(populated().to_csv_string().unwrap(), EXPECTED);
}

#[test]
fn test_save_to_file() {
    let tmp = TempDir::new();
    let fp = tmp.path().path_to("file.csv");
    populated().save_to_file(&fp).unwrap();
    assert_eq!(file_utils::read_file(&fp).unwrap(), EXPECTED.as_bytes());
}
