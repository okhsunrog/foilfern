//! Port of tests/unittests/core/library/dev/devicetest.cpp.

use foilfern_core::library::dev::Device;

use super::assert_upgrade_v01;

const UUID: &str = "4f5ee784-4b1b-407c-802b-44625163d90f";

#[test]
fn test_upgrade_v01() {
    assert_upgrade_v01::<Device>(&format!("libraries/v0.1.lplib/dev/{UUID}"), UUID);
}
