//! Port of tests/unittests/core/types/signalroletest.cpp.

use foilfern_core::serialization::{FromSExpression, Mode, SExpression, ToSExpression};
use foilfern_core::types::SignalRole;

#[test]
fn test_serialize() {
    let bytes = SignalRole::OpenDrain
        .to_sexpression()
        .to_byte_array(Mode::LibrePcb)
        .unwrap();
    assert_eq!(bytes, b"opendrain\n");
}

#[test]
fn test_deserialize() {
    let sexpr = SExpression::string("opendrain");
    assert_eq!(
        SignalRole::from_sexpression(&sexpr),
        Ok(SignalRole::OpenDrain)
    );
}

#[test]
fn test_deserialize_empty() {
    assert!(SignalRole::from_sexpression(&SExpression::string("")).is_err());
}

#[test]
fn test_deserialize_invalid() {
    assert!(SignalRole::from_sexpression(&SExpression::string("foo")).is_err());
}
