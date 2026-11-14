//! Ports of tests/unittests/core/geometry/*.cpp.

mod hole_test;
mod image_test;
mod pad_geometry_test;
mod pad_test;
mod path_test;
mod polygon_test;
mod stroke_text_test;
mod text_test;
mod trace_test;
mod vertex_test;
mod via_test;

use foilfern_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};

/// Serializes `obj` into a list named `name` and returns the file content.
pub fn serialize_str(obj: &impl SerializeObject, name: &str) -> String {
    let mut root = List::new(name);
    obj.serialize(&mut root);
    SExpression::List(root)
        .to_string_with_mode(Mode::LibrePcb)
        .unwrap()
}

/// Parses an S-expression.
pub fn parse(content: &str) -> SExpression {
    SExpression::parse(content.as_bytes(), None, Mode::LibrePcb).unwrap()
}

/// Upstream `testSerializeAndDeserialize`: serializing, deserializing and
/// serializing again must give identical output.
pub fn assert_roundtrip<T: SerializeObject + DeserializeObject>(obj: &T) {
    let s1 = serialize_str(obj, "obj");
    let obj2 = T::deserialize(&parse(&s1)).unwrap();
    let s2 = serialize_str(&obj2, "obj");
    assert_eq!(s1, s2);
}
