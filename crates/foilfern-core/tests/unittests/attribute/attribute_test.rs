//! Port of tests/unittests/core/attribute/attributetest.cpp.

use std::path::{Path, PathBuf};

use foilfern_core::attribute::{Attribute, AttributeKey, AttributeList, AttributeType};
use foilfern_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};

struct Data {
    key: &'static str,
    attribute_type: &'static str,
    unit: &'static str,
    value: &'static str,
    serialized: &'static str,
    valid_sexpression: bool,
}

const fn d(serialized: &'static str, valid_sexpression: bool) -> Data {
    Data {
        key: "FOO",
        attribute_type: "voltage",
        unit: "volt",
        value: "4.2",
        serialized,
        valid_sexpression,
    }
}

#[rustfmt::skip]
const DATA: &[Data] = &[
    // Invalid serialization
    d("(attribute \"FOO\" (type foo) (unit volt) (value \"4.2\"))\n", false),
    d("(attribute \"FOO\" (type voltage) (unit volt) (value \"foo\"))\n", false),
    d("(attribute \"FOO\" (type voltage) (unit foo) (value \"4.2\"))\n", false),
    d("(attribute (type voltage) (unit foo) (value \"4.2\"))\n", false),
    d("(attribute \"\" (type voltage) (unit volt) (value \"4.2\"))\n", false),
    // Valid serialization
    d("(attribute \"FOO\" (type voltage) (unit volt) (value \"4.2\"))\n", true),
];

fn attribute(data: &Data) -> Attribute {
    let attribute_type: AttributeType = data.attribute_type.parse().unwrap();
    Attribute::new(
        AttributeKey::new(data.key).unwrap(),
        attribute_type,
        data.value,
        attribute_type.unit_from_string(data.unit).unwrap(),
    )
    .unwrap()
}

#[test]
fn test_construct_from_sexpression() {
    for data in DATA {
        let attribute = attribute(data);
        let sexpr = SExpression::parse(data.serialized.as_bytes(), None, Mode::LibrePcb).unwrap();
        let result = Attribute::deserialize(&sexpr);
        if data.valid_sexpression {
            assert_eq!(result.unwrap(), attribute);
        } else {
            assert!(result.is_err(), "{}", data.serialized);
        }
    }
}

/// Not upstream: every attribute in the `.lp` files of the test data must
/// load and serialize to the identical node.
#[test]
fn test_roundtrip_test_data() {
    fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, files);
            } else if path.extension().is_some_and(|e| e == "lp") {
                files.push(path);
            }
        }
    }
    fn check(node: &SExpression, count: &mut usize) {
        let Some(list) = node.as_list() else {
            return;
        };
        if list.name() == "attribute" && node.child("type").is_some() {
            let attribute = Attribute::deserialize(node).unwrap();
            let mut root = List::new("attribute");
            attribute.serialize(&mut root);
            assert_eq!(&SExpression::List(root), node);
            *count += 1;
        }
        for child in list.children() {
            check(child, count);
        }
    }
    let data_dir = Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data");
    let mut files = Vec::new();
    collect(&data_dir, &mut files);
    let mut count = 0;
    for file in files {
        let content = std::fs::read(&file).unwrap();
        let root = SExpression::parse(&content, Some(&file), Mode::LibrePcb).unwrap();
        check(&root, &mut count);
    }
    assert!(count > 50, "only {count} attributes found");
}

#[test]
fn test_serialize() {
    for data in DATA.iter().filter(|d| d.valid_sexpression) {
        let mut root = List::new("attribute");
        attribute(data).serialize(&mut root);
        let bytes = SExpression::List(root)
            .to_byte_array(Mode::LibrePcb)
            .unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), data.serialized);
    }
}

/// Not upstream: list (de)serialization and upstream `operator|`.
#[test]
fn test_attribute_list() {
    let string = |key: &str, value: &str| {
        Attribute::new(
            AttributeKey::new(key).unwrap(),
            AttributeType::String,
            value,
            None,
        )
        .unwrap()
    };
    let lhs = AttributeList::from(vec![string("A", "1"), string("B", "2")]);
    let rhs = AttributeList::from(vec![string("B", "3"), string("C", "4")]);
    let merged = lhs.merged_with(&rhs);
    let values: Vec<_> = merged
        .iter()
        .map(|a| (a.key().as_str(), a.value()))
        .collect();
    assert_eq!(values, [("A", "1"), ("B", "2"), ("C", "4")]);

    let mut root = List::new("root");
    merged.serialize(&mut root);
    let root = SExpression::List(root);
    let expected = "(root\n \
        (attribute \"A\" (type string) (unit none) (value \"1\"))\n \
        (attribute \"B\" (type string) (unit none) (value \"2\"))\n \
        (attribute \"C\" (type string) (unit none) (value \"4\"))\n)\n";
    assert_eq!(root.to_string_with_mode(Mode::LibrePcb).unwrap(), expected);
    assert_eq!(AttributeList::deserialize(&root).unwrap(), merged);
}
