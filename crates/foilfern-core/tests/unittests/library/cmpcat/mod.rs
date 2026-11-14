//! Port of tests/unittests/core/library/cmpcat/componentcategorytest.cpp.

use foilfern_core::library::cat::ComponentCategory;

use super::assert_upgrade_v01;

const UUID: &str = "1039f038-20a6-4bfe-89c1-99f34fbb45bd";

#[test]
fn test_upgrade_v01() {
    assert_upgrade_v01::<ComponentCategory>(&format!("libraries/v0.1.lplib/cmpcat/{UUID}"), UUID);
}
