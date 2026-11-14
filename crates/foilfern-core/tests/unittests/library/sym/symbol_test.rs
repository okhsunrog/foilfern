//! Port of tests/unittests/core/library/sym/symboltest.cpp.

use foilfern_core::library::sym::Symbol;

use crate::library::assert_upgrade_v01;

const UUID: &str = "35aad2af-5cd4-42ae-8576-fe7febad0d8a";

#[test]
fn test_upgrade_v01() {
    assert_upgrade_v01::<Symbol>(&format!("libraries/v0.1.lplib/sym/{UUID}"), UUID);
}
