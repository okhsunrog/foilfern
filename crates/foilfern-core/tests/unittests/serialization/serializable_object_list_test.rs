//! Port of tests/unittests/core/serialization/serializableobjectlisttest.cpp.
//!
//! Upstream stores elements as `std::shared_ptr`, so several tests check
//! pointer identity. Here the list owns its elements by value, so these tests
//! check value equality instead; copy tests check that the copy is deep.

use std::collections::HashSet;

use foilfern_core::serialization::{
    DeserializeObject, List, ListTagName, SExpression, SerializableObjectList, SerializeObject,
};
use foilfern_core::types::Uuid;

use super::serializable_object_mock::{
    MinimalSerializableObjectMock as MinimalMock, SerializableObjectMock as Mock,
};

struct TagNameProvider;

impl ListTagName for TagNameProvider {
    const TAG_NAME: &'static str = "test";
}

type MinimalList = SerializableObjectList<MinimalMock, TagNameProvider>;
type TestList = SerializableObjectList<Mock, TagNameProvider>;

fn mocks() -> [Mock; 3] {
    let mock = |uuid: &str, name| Mock::new(uuid.parse().unwrap(), name);
    [
        mock("c2ceffd2-4cc5-43c6-941c-fc64a341d026", "foo"),
        mock("4484ba9b-f3f8-4487-9109-10a8e9844fdc", "bar"),
        mock("162bf1b0-f45e-4175-9656-33b5adc73ed0", "pcb"),
    ]
}

fn list(items: &[&Mock]) -> TestList {
    items.iter().map(|m| (*m).clone()).collect()
}

#[test]
fn test_instantiation_with_minimal_element_class() {
    let sexpr = SExpression::list("list");
    let l1 = MinimalList::new(); // default ctor
    let l2 = l1; // move
    let mut l3 = MinimalList::deserialize(&sexpr).unwrap(); // SExpression ctor
    l3.push(MinimalMock::new("foo"));
    assert_eq!(l2.len(), 0);
    assert!(l2.get(0).is_none());
    assert!(l3.get(0).is_some());
}

#[test]
fn test_default_constructor() {
    assert_eq!(TestList::new().len(), 0);
}

#[test]
fn test_copy_constructor() {
    let m = mocks();
    let l1 = list(&[&m[0], &m[1]]);
    let mut l2 = l1.clone();
    assert_eq!(l2.len(), 2);
    assert_eq!(l2[0], m[0]);
    assert_eq!(l2[1], m[1]);
    // The copy is deep.
    l2[0].name = "changed".into();
    assert_eq!(l1[0], m[0]);
}

#[test]
fn test_move_constructor() {
    let m = mocks();
    let l1 = list(&[&m[0]]);
    let l2 = l1;
    assert_eq!(l2.len(), 1);
    assert_eq!(l2[0], m[0]);
}

#[test]
fn test_value_initializer_list_constructor() {
    let l: TestList = [
        Mock::new(Uuid::new_random(), "foo"),
        Mock::new(Uuid::new_random(), "bar"),
    ]
    .into_iter()
    .collect();
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].name, "foo");
    assert_eq!(l[1].name, "bar");
}

#[test]
fn test_sexpression_constructor() {
    let m = mocks();
    let mut e = List::new("list");
    e.ensure_line_break();
    e.append_child("test", &m[0].uuid)
        .append_child("name", "foo");
    e.ensure_line_break();
    e.append_child("test", &m[1].uuid)
        .append_child("name", "bar");
    e.ensure_line_break();
    e.append_child("none", &m[2].uuid)
        .append_child("name", "bar");
    e.ensure_line_break();
    let l = TestList::deserialize(&e.into()).unwrap();
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].uuid, m[0].uuid);
    assert_eq!(l[1].uuid, m[1].uuid);
    assert_eq!(l[0].name, "foo");
    assert_eq!(l[1].name, "bar");
}

#[test]
fn test_get_uuids() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2], &m[2]]);
    let vector = vec![m[0].uuid, m[1].uuid, m[2].uuid, m[2].uuid];
    let set: HashSet<Uuid> = [m[0].uuid, m[1].uuid, m[2].uuid].into();
    assert_eq!(l.uuids(), vector);
    assert_eq!(l.uuid_set(), set);
}

#[test]
fn test_index_of_uuid() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]);
    assert_eq!(l.index_of_uuid(&m[1].uuid), Some(1));
}

#[test]
fn test_index_of_name_case_sensitive() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]);
    assert_eq!(l.index_of_name("pcb", true), Some(2));
    assert_eq!(l.index_of_name("PCB", true), None);
}

#[test]
fn test_index_of_name_case_insensitive() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]);
    assert_eq!(l.index_of_name("pcb", false), Some(2));
    assert_eq!(l.index_of_name("PCB", false), Some(2));
}

#[test]
fn test_contains_uuid() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]);
    assert!(l.contains_uuid(&m[1].uuid));
    assert!(!l.contains_uuid(&Uuid::new_random()));
}

#[test]
fn test_contains_name() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]);
    assert!(l.contains_name(&m[2].name));
    assert!(!l.contains_name(""));
}

#[test]
fn test_lookup_by_uuid_and_name() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]);
    assert_eq!(l.by_uuid(&m[1].uuid), Some(&m[1]));
    assert_eq!(l.required_by_uuid(&m[1].uuid), Ok(&m[1]));
    assert_eq!(l.by_name("PCB", false), Some(&m[2]));
    assert_eq!(l.required_by_name("pcb"), Ok(&m[2]));
    let uuid = Uuid::new_random();
    assert_eq!(l.by_uuid(&uuid), None);
    assert_eq!(
        l.required_by_uuid(&uuid).unwrap_err().to_string(),
        format!("There is no element of type \"test\" with the UUID \"{uuid}\" in the list.")
    );
    assert_eq!(
        l.required_by_name("PCB").unwrap_err().to_string(),
        "There is no element of type \"test\" with the name \"PCB\" in the list."
    );
}

#[test]
fn test_data_access() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]);
    assert_eq!(l.first(), Some(&m[0]));
    assert_eq!(l[0], m[0]);
    assert_eq!(l[1], m[1]);
    assert_eq!(l[2], m[2]);
    assert_eq!(l.last(), Some(&m[2]));
}

#[test]
fn test_iterator_on_empty_list() {
    let l = TestList::new();
    assert_eq!(l.iter().count(), 0);
}

#[test]
fn test_const_iterator() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]);
    let mut i = 0;
    for mock in &l {
        assert_eq!(*mock, m[i]);
        i += 1;
    }
    assert_eq!(i, 3);
}

#[test]
fn test_mutable_iterator() {
    let m = mocks();
    let mut l = list(&[&m[0], &m[1], &m[2]]);
    for (i, mock) in (&mut l).into_iter().enumerate() {
        mock.name = i.to_string();
    }
    assert_eq!(l[0].name, "0");
    assert_eq!(l[1].name, "1");
    assert_eq!(l[2].name, "2");
}

#[test]
fn test_swap() {
    let m = mocks();
    let mut l = list(&[&m[0], &m[1], &m[2]]);
    l.swap(2, 1);
    assert_eq!(l[0], m[0]);
    assert_eq!(l[1], m[2]);
    assert_eq!(l[2], m[1]);
    l.swap(0, 100); // Indices are clamped.
    assert_eq!(l[0], m[1]);
    assert_eq!(l[2], m[0]);
}

#[test]
fn test_insert() {
    let m = mocks();
    let mut l = TestList::new();
    assert_eq!(l.insert(0, m[0].clone()), 0);
    assert_eq!(l.insert(0, m[1].clone()), 0);
    assert_eq!(l.insert(1, m[2].clone()), 1);
    assert_eq!(l.len(), 3);
    assert_eq!(l[0], m[1]);
    assert_eq!(l[1], m[2]);
    assert_eq!(l[2], m[0]);
    assert_eq!(l.insert(100, m[0].clone()), 3); // Index is clamped.
}

#[test]
fn test_append_item() {
    let m = mocks();
    let mut l = TestList::new();
    l.push(m[0].clone());
    l.push(m[1].clone());
    l.push(m[2].clone());
    assert_eq!(l.len(), 3);
    assert_eq!(l[0], m[0]);
    assert_eq!(l[1], m[1]);
    assert_eq!(l[2], m[2]);
}

#[test]
fn test_append_list() {
    let m = mocks();
    let mut l1 = list(&[&m[0]]);
    let l2 = list(&[&m[1], &m[2]]);
    l1.extend(l2);
    assert_eq!(l1.len(), 3);
    assert_eq!(l1[0], m[0]);
    assert_eq!(l1[1], m[1]);
    assert_eq!(l1[2], m[2]);
}

#[test]
fn test_take() {
    let m = mocks();
    let mut l = list(&[&m[0], &m[1], &m[2]]);
    assert_eq!(l.take(1), Some(m[1].clone()));
    assert_eq!(l.take_by_uuid(&m[2].uuid), Some(m[2].clone()));
    assert_eq!(l.take_by_name("foo"), Some(m[0].clone()));
    assert_eq!(l.take(0), None);
    assert!(l.is_empty());
}

#[test]
fn test_clear() {
    let m = mocks();
    let mut l = list(&[&m[0], &m[1], &m[2]]);
    l.clear();
    assert_eq!(l.len(), 0);
}

#[test]
fn test_sorted_by_uuid() {
    let m = mocks();
    let l = list(&[&m[0], &m[1], &m[2]]).sorted_by_uuid();
    assert_eq!(l.uuids(), [m[2].uuid, m[1].uuid, m[0].uuid]);
}

#[test]
fn test_serialize() {
    let m = mocks();
    let mut e = List::new("list");
    list(&[&m[0], &m[1], &m[2]]).serialize(&mut e);
    // (list
    //  (test c2ceffd2-4cc5-43c6-941c-fc64a341d026
    //   (name "foo")
    //  )
    //  (test 4484ba9b-f3f8-4487-9109-10a8e9844fdc
    //   (name "bar")
    //  )
    //  (test 162bf1b0-f45e-4175-9656-33b5adc73ed0
    //   (name "pcb")
    //  )
    // )
    let children = e.children();
    assert_eq!(children.len(), 7);
    for (i, mock) in m.iter().enumerate() {
        assert!(children[i * 2].is_line_break());
        let list = children[i * 2 + 1].as_list().unwrap();
        assert_eq!(list.name(), "test");
        assert_eq!(list.children().len(), 4);
        assert!(list.children()[0].is_token());
        assert_eq!(list.children()[0].value().unwrap(), mock.uuid.to_string());
        assert!(list.children()[1].is_line_break());
        let name = list.children()[2].as_list().unwrap();
        assert_eq!(name.name(), "name");
        assert_eq!(name.children().len(), 1);
        assert_eq!(name.children()[0].value().unwrap(), mock.name);
        assert!(list.children()[3].is_line_break());
    }
    assert!(children[6].is_line_break());
}

#[test]
fn test_serialize_empty() {
    let mut e = List::new("list");
    TestList::new().serialize(&mut e);
    // (list
    // )
    assert_eq!(e.children().len(), 1);
    assert!(e.children()[0].is_line_break());
}

#[test]
fn test_operator_equal() {
    let m = mocks();
    assert!(TestList::new() == TestList::new());
    assert!(list(&[&m[0], &m[1]]) == list(&[&m[0], &m[1]]));
    assert!(list(&[&m[0], &m[1]]) != list(&[&m[0], &m[2]]));
    assert!(list(&[&m[0]]) != list(&[&m[0], &m[1]]));
}

#[test]
fn test_operator_assign() {
    let m = mocks();
    let l1 = list(&[&m[0], &m[1]]);
    let mut l2 = list(&[&m[2]]);
    assert_eq!(l2.len(), 1);
    l2 = l1.clone();
    assert_eq!(l1.len(), 2);
    assert_eq!(l2.len(), 2);
    assert_eq!(l2[0], m[0]);
    assert_eq!(l2[1], m[1]);
}
