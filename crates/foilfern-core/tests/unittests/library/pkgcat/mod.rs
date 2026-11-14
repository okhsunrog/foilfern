//! Port of tests/unittests/core/library/pkgcat/packagecategorytest.cpp.

use foilfern_core::library::cat::PackageCategory;

use super::assert_upgrade_v01;

const UUID: &str = "a20f0330-06d3-4bc2-a1fa-f8577deb6770";

#[test]
fn test_upgrade_v01() {
    assert_upgrade_v01::<PackageCategory>(&format!("libraries/v0.1.lplib/pkgcat/{UUID}"), UUID);
}
