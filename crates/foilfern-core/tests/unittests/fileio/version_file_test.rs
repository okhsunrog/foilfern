//! Port of tests/unittests/core/fileio/versionfiletest.cpp.

use foilfern_core::fileio::VersionFile;
use foilfern_core::types::Version;

fn v(s: &str) -> Version {
    s.parse().unwrap()
}

#[test]
fn test_get_version() {
    let p = VersionFile::new(v("1.2.3"));
    assert_eq!(p.version(), &v("1.2.3"));
}

#[test]
fn test_set_version() {
    let mut p = VersionFile::new(v("1.2.3"));
    p.set_version(v("1.5.3"));
    assert_eq!(p.version(), &v("1.5.3"));
}

#[test]
fn test_to_byte_array() {
    let p = VersionFile::new(v("1.2.3"));
    assert_eq!(p.to_bytes(), b"1.2.3\n");
}

#[test]
fn test_from_byte_array_normal() {
    let p = VersionFile::from_bytes(b"1.2.3\n").unwrap();
    assert_eq!(p.version().to_string(), "1.2.3");
}

#[test]
fn test_from_byte_array_no_eol() {
    let p = VersionFile::from_bytes(b"1.2.3").unwrap();
    assert_eq!(p.version().to_string(), "1.2.3");
}

#[test]
fn test_from_byte_array_multiline() {
    let p = VersionFile::from_bytes(b"1.2.3\nsomecomment\n").unwrap();
    assert_eq!(p.version().to_string(), "1.2.3");
}

#[test]
fn test_from_byte_array_wrong() {
    assert!(VersionFile::from_bytes(b"dead").is_err());
}

#[test]
fn test_from_byte_array_empty() {
    assert!(VersionFile::from_bytes(b"").is_err());
}
