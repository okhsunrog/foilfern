//! Port of tests/unittests/core/library/cmp/componentsymbolvariantitemtest.cpp.

use foilfern_core::library::cmp::{ComponentSymbolVariantItem, ComponentSymbolVariantItemSuffix};
use foilfern_core::types::{Angle, Point, Uuid};

fn obj1() -> ComponentSymbolVariantItem {
    ComponentSymbolVariantItem::new(
        "c53e0493-d446-4c97-a302-95d62840c762".parse().unwrap(),
        "d2f47222-2c1c-4097-8611-9559c3198fdf".parse().unwrap(),
        Point::from_nm(12, 34),
        Angle::new(42),
        true,
        ComponentSymbolVariantItemSuffix::new("A").unwrap(),
    )
}

fn obj2() -> ComponentSymbolVariantItem {
    ComponentSymbolVariantItem::new(
        "0e97ed2c-b7c8-40e5-aa9e-194cde326c3e".parse().unwrap(),
        "a2b53202-e2a8-488e-9847-a742d093e4be".parse().unwrap(),
        Point::from_nm(56, 78),
        Angle::new(24),
        false,
        ComponentSymbolVariantItemSuffix::new("B").unwrap(),
    )
}

/// Creates an item with the attributes of `obj1()`, except the one taken
/// from `obj2()` by `modify`.
fn modified(
    modify: impl Fn(&mut ComponentSymbolVariantItem, &ComponentSymbolVariantItem),
) -> ComponentSymbolVariantItem {
    let mut obj = obj1();
    modify(&mut obj, &obj2());
    obj
}

#[test]
fn test_operator_assignment() {
    let o1 = obj1();
    let mut o2 = obj2();
    o2.clone_from(&o1);
    assert_eq!(o1.uuid(), o2.uuid());
    assert_eq!(o1.symbol_uuid(), o2.symbol_uuid());
    assert_eq!(o1.symbol_position(), o2.symbol_position());
    assert_eq!(o1.symbol_rotation(), o2.symbol_rotation());
    assert_eq!(o1.is_required(), o2.is_required());
    assert_eq!(o1.suffix(), o2.suffix());
}

#[test]
fn test_operator_equal() {
    let o1 = obj1();
    let obj = ComponentSymbolVariantItem::new(
        o1.uuid(),
        o1.symbol_uuid(),
        o1.symbol_position(),
        o1.symbol_rotation(),
        o1.is_required(),
        o1.suffix().clone(),
    );
    assert!(obj == o1);
    assert!(!(obj != o1));
}

#[test]
fn test_operator_equal_on_different_uuid() {
    let uuid: Uuid = obj2().uuid();
    let obj = obj1().with_uuid(uuid);
    assert!(obj != obj1());
}

#[test]
fn test_operator_equal_on_different_symbol() {
    let obj = modified(|o, o2| {
        o.set_symbol_uuid(o2.symbol_uuid());
    });
    assert!(obj != obj1());
}

#[test]
fn test_operator_equal_on_different_pos() {
    let obj = modified(|o, o2| {
        o.set_symbol_position(o2.symbol_position());
    });
    assert!(obj != obj1());
}

#[test]
fn test_operator_equal_on_different_rot() {
    let obj = modified(|o, o2| {
        o.set_symbol_rotation(o2.symbol_rotation());
    });
    assert!(obj != obj1());
}

#[test]
fn test_operator_equal_on_different_required() {
    let obj = modified(|o, o2| {
        o.set_is_required(o2.is_required());
    });
    assert!(obj != obj1());
}

#[test]
fn test_operator_equal_on_different_suffix() {
    let obj = modified(|o, o2| {
        o.set_suffix(o2.suffix().clone());
    });
    assert!(obj != obj1());
}
