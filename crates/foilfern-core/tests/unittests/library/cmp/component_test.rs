//! Port of tests/unittests/core/library/cmp/componenttest.cpp.

use foilfern_core::library::cmp::Component;

use crate::library::assert_upgrade_v01;

const UUID: &str = "45022bef-9310-4aa2-92ef-acec1b95aa4e";

#[test]
fn test_upgrade_v01() {
    assert_upgrade_v01::<Component>(&format!("libraries/v0.1.lplib/cmp/{UUID}"), UUID);
}
