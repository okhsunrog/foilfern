//! Circuit mutations: inverses, validation errors, reverse index.

use std::collections::BTreeSet;

use librepcb_core::attribute::AttributeList;
use librepcb_core::project::circuit::{
    AssemblyVariant, Bus, ComponentAssemblyOption, ComponentAssemblyOptionList,
};
use librepcb_core::project::{
    AssemblyVariantId, BusId, Change, ChangesSince, ComponentSignalRef, EntityKind, Error,
    Mutation, NetSignalId,
};
use librepcb_core::types::{BusName, CircuitIdentifier, ElementName, FileProofName, Uuid};

use super::*;

#[test]
fn net_class_add_update_remove() {
    let mut p = new_project();
    let nc = NetClass::new(Uuid::new_random(), ElementName::new("power").unwrap());
    let id = NetClassId(nc.uuid());
    assert_inverse(&mut p, Mutation::AddNetClass(nc.clone()));
    p.apply(Mutation::AddNetClass(nc.clone())).unwrap();

    let mut renamed = nc.clone();
    renamed.set_name(ElementName::new("signal").unwrap());
    assert_inverse(&mut p, Mutation::UpdateNetClass(renamed));
    assert_inverse(&mut p, Mutation::RemoveNetClass(id));

    // Duplicate UUID and name.
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddNetClass(nc.clone())),
        Error::DuplicateUuid {
            kind: EntityKind::NetClass,
            ..
        }
    ));
    let same_name = NetClass::new(Uuid::new_random(), ElementName::new("power").unwrap());
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddNetClass(same_name)),
        Error::DuplicateName {
            kind: EntityKind::NetClass,
            ..
        }
    ));
    let mut clash = nc.clone();
    clash.set_name(ElementName::new("default").unwrap());
    assert!(matches!(
        assert_fails(&mut p, Mutation::UpdateNetClass(clash)),
        Error::DuplicateName { .. }
    ));

    // In use by a net signal.
    add_net(&mut p, "GND");
    let default = default_net_class(&p);
    assert!(matches!(
        assert_fails(&mut p, Mutation::RemoveNetClass(default)),
        Error::NetClassInUse(name) if name == "default"
    ));
    assert!(matches!(
        assert_fails(&mut p, Mutation::RemoveNetClass(NetClassId::new_random())),
        Error::NotFound {
            kind: EntityKind::NetClass,
            ..
        }
    ));
}

#[test]
fn net_signal_add_update_remove() {
    let mut p = new_project();
    let net_class = default_net_class(&p);
    let ns = NetSignal::new(
        Uuid::new_random(),
        net_class,
        CircuitIdentifier::new("GND").unwrap(),
        false,
    );
    let id = NetSignalId(ns.uuid());
    assert_inverse(&mut p, Mutation::AddNetSignal(ns.clone()));
    p.apply(Mutation::AddNetSignal(ns.clone())).unwrap();
    assert_eq!(p.circuit().generate_auto_net_signal_name(), "N1");

    let mut renamed = ns.clone();
    renamed.set_name(CircuitIdentifier::new("N1").unwrap(), true);
    assert_inverse(&mut p, Mutation::UpdateNetSignal(renamed));
    assert_inverse(&mut p, Mutation::RemoveNetSignal(id));

    // Unknown net class.
    let other = NetSignal::new(
        Uuid::new_random(),
        NetClassId::new_random(),
        CircuitIdentifier::new("VCC").unwrap(),
        false,
    );
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddNetSignal(other)),
        Error::InexistentNetClass(_)
    ));
    let mut moved = ns.clone();
    moved.set_net_class(NetClassId::new_random());
    assert!(matches!(
        assert_fails(&mut p, Mutation::UpdateNetSignal(moved)),
        Error::InexistentNetClass(_)
    ));

    // Changing the net class through an update works.
    let power = add_net_class(&mut p, "power");
    let mut moved = ns.clone();
    moved.set_net_class(power);
    assert_inverse(&mut p, Mutation::UpdateNetSignal(moved));

    // In use by a component signal.
    assert!(!p.is_net_signal_used(id));
    let (cmp, signals) = add_component(&mut p, "R1", Some(id));
    assert!(p.is_net_signal_used(id));
    assert_eq!(p.net_signal_uses(id).count(), 2);
    assert_eq!(p.net_signal_with_most_elements(), Some(id));
    assert!(matches!(
        assert_fails(&mut p, Mutation::RemoveNetSignal(id)),
        Error::NetSignalInUse(name) if name == "GND"
    ));
    for signal in &signals {
        p.set_component_signal_net(
            ComponentSignalRef {
                component: cmp,
                signal: *signal,
            },
            None,
        )
        .unwrap();
    }
    assert!(!p.is_net_signal_used(id));
    p.remove_net_signal(id).unwrap();
    assert!(p.is_ref_index_consistent());
}

#[test]
fn bus_add_update_remove() {
    let mut p = new_project();
    let bus = Bus::new(
        Uuid::new_random(),
        BusName::new("DATA[0..7]").unwrap(),
        false,
        true,
        None,
    );
    let id = BusId(bus.uuid());
    assert_inverse(&mut p, Mutation::AddBus(bus.clone()));
    p.apply(Mutation::AddBus(bus.clone())).unwrap();
    assert_eq!(p.circuit().generate_auto_bus_name(), "B1");
    let mut renamed = bus.clone();
    renamed.set_name(BusName::new("B1").unwrap(), true);
    assert_inverse(&mut p, Mutation::UpdateBus(renamed));
    assert_inverse(&mut p, Mutation::RemoveBus(id));
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddBus(bus)),
        Error::DuplicateUuid {
            kind: EntityKind::Bus,
            ..
        }
    ));
}

#[test]
fn assembly_variants() {
    let mut p = new_project();
    let std_id = AssemblyVariantId(p.circuit().assembly_variants().first().unwrap().uuid());
    assert!(matches!(
        assert_fails(&mut p, Mutation::RemoveAssemblyVariant(std_id)),
        Error::LastAssemblyVariant
    ));
    let lite = AssemblyVariant::new(Uuid::new_random(), FileProofName::new("Lite").unwrap(), "");
    let lite_id = AssemblyVariantId(lite.uuid());
    // Inserted at the front, removal restores the position.
    assert_inverse(
        &mut p,
        Mutation::AddAssemblyVariant {
            variant: lite.clone(),
            index: Some(0),
        },
    );
    p.add_assembly_variant(lite.clone(), Some(0)).unwrap();
    assert_eq!(
        p.circuit().assembly_variants().uuids(),
        vec![lite.uuid(), std_id.0]
    );
    let mut renamed = lite.clone();
    renamed.set_name(FileProofName::new("Std").unwrap());
    assert!(matches!(
        assert_fails(&mut p, Mutation::UpdateAssemblyVariant(renamed)),
        Error::DuplicateName {
            kind: EntityKind::AssemblyVariant,
            ..
        }
    ));
    let mut described = lite.clone();
    described.set_description("Lite version".to_owned());
    assert_inverse(&mut p, Mutation::UpdateAssemblyVariant(described));
    assert_inverse(&mut p, Mutation::RemoveAssemblyVariant(lite_id));
    assert_eq!(p.remove_assembly_variant(lite_id).unwrap(), lite);
    assert_eq!(p.circuit().assembly_variants().uuids(), vec![std_id.0]);
}

#[test]
fn component_instance_lifecycle() {
    let mut p = new_project();
    let gnd = add_net(&mut p, "GND");
    let (lib, variant, signals) = add_library_component(&mut p);
    let mut cmp = component_instance(&p, lib, variant, "R1");
    cmp.set_signal_net(&signals[0], Some(gnd)).unwrap();
    let id = cmp.id();
    let sig0 = ComponentSignalRef {
        component: id,
        signal: signals[0],
    };
    let sig1 = ComponentSignalRef {
        component: id,
        signal: signals[1],
    };

    assert_inverse(&mut p, Mutation::AddComponentInstance(cmp.clone()));
    assert!(!p.is_net_signal_used(gnd));
    p.add_component_instance(cmp.clone()).unwrap();
    assert!(p.is_net_signal_used(gnd));
    assert_eq!(p.circuit().generate_auto_component_instance_name("R"), "R2");
    assert_eq!(p.circuit().generate_auto_component_instance_name(""), "?1");

    // Connect / disconnect signals.
    let vcc = add_net(&mut p, "VCC");
    let inverse = assert_inverse(
        &mut p,
        Mutation::SetComponentSignalNet {
            signal: sig1,
            net: Some(vcc),
        },
    );
    assert_eq!(
        inverse,
        Mutation::SetComponentSignalNet {
            signal: sig1,
            net: None
        }
    );
    assert_inverse(
        &mut p,
        Mutation::SetComponentSignalNet {
            signal: sig0,
            net: Some(vcc),
        },
    );
    assert_inverse(
        &mut p,
        Mutation::SetComponentSignalNet {
            signal: sig0,
            net: None,
        },
    );
    assert!(matches!(
        assert_fails(
            &mut p,
            Mutation::SetComponentSignalNet {
                signal: sig0,
                net: Some(NetSignalId::new_random()),
            }
        ),
        Error::InexistentNetSignal(_)
    ));
    assert!(matches!(
        assert_fails(
            &mut p,
            Mutation::SetComponentSignalNet {
                signal: ComponentSignalRef {
                    component: id,
                    signal: Uuid::new_random(),
                },
                net: None,
            }
        ),
        Error::InexistentComponentSignal(_)
    ));
    // Setting the same net again is a no-op (nothing journaled).
    let revision = p.revision();
    p.set_component_signal_net(sig0, Some(gnd)).unwrap();
    assert_eq!(p.revision(), revision);

    // Properties.
    let mut props = p.circuit().component_instance(id).unwrap().properties();
    props.name = CircuitIdentifier::new("R2").unwrap();
    props.value = "10k".into();
    props.lock_assembly = true;
    props.assembly_options = ComponentAssemblyOptionList::from(vec![ComponentAssemblyOption::new(
        Uuid::new_random(),
        AttributeList::new(),
        BTreeSet::from([AssemblyVariantId(
            p.circuit().assembly_variants().first().unwrap().uuid(),
        )]),
        Default::default(),
    )]);
    assert_inverse(&mut p, Mutation::UpdateComponentInstance(props.clone()));
    p.apply(Mutation::UpdateComponentInstance(props)).unwrap();
    assert_eq!(
        p.circuit()
            .component_instance(id)
            .unwrap()
            .parts(None)
            .len(),
        1
    );
    let (other, _) = add_component(&mut p, "R3", None);
    let mut clash = p.circuit().component_instance(other).unwrap().properties();
    clash.name = CircuitIdentifier::new("R2").unwrap();
    assert!(matches!(
        assert_fails(&mut p, Mutation::UpdateComponentInstance(clash)),
        Error::DuplicateName {
            kind: EntityKind::Component,
            ..
        }
    ));

    // Removal (allowed while only signals are connected, like upstream).
    assert_inverse(&mut p, Mutation::RemoveComponentInstance(id));
    let removed = p.remove_component_instance(id).unwrap();
    assert_eq!(removed.name().as_str(), "R2");
    assert!(!p.is_net_signal_used(gnd));
    assert!(matches!(
        assert_fails(&mut p, Mutation::RemoveComponentInstance(id)),
        Error::NotFound {
            kind: EntityKind::Component,
            ..
        }
    ));
}

#[test]
fn component_instance_validation() {
    let mut p = new_project();
    let (lib, variant, _) = add_library_component(&mut p);

    // Unknown symbol variant.
    let lib_cmp = p.library().component(&lib).unwrap();
    assert!(
        ComponentInstance::new(
            Uuid::new_random(),
            lib_cmp,
            Uuid::new_random(),
            CircuitIdentifier::new("R1").unwrap(),
            false,
        )
        .is_err()
    );

    // Duplicate name.
    let a = component_instance(&p, lib, variant, "R1");
    p.add_component_instance(a.clone()).unwrap();
    let b = component_instance(&p, lib, variant, "R1");
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddComponentInstance(b)),
        Error::DuplicateName {
            kind: EntityKind::Component,
            ..
        }
    ));
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddComponentInstance(a)),
        Error::DuplicateUuid {
            kind: EntityKind::Component,
            ..
        }
    ));

    // Library component not in the project library.
    let (foreign, foreign_variant) = library_component(Uuid::new_random(), 1, Uuid::new_random());
    let c = ComponentInstance::new(
        Uuid::new_random(),
        &foreign,
        foreign_variant,
        CircuitIdentifier::new("R2").unwrap(),
        false,
    )
    .unwrap();
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddComponentInstance(c)),
        Error::MissingLibraryComponent(_)
    ));

    // Symbol variant not in the project library's component (same UUID,
    // other variant).
    let (stale, stale_variant) = library_component(lib, 2, Uuid::new_random());
    let d = ComponentInstance::new(
        Uuid::new_random(),
        &stale,
        stale_variant,
        CircuitIdentifier::new("R3").unwrap(),
        false,
    )
    .unwrap();
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddComponentInstance(d)),
        Error::Serialization(_)
    ));

    // Signals not matching the library component (same UUID and variant,
    // other signals).
    let (stale, _) = library_component(lib, 3, variant);
    let e = ComponentInstance::new(
        Uuid::new_random(),
        &stale,
        variant,
        CircuitIdentifier::new("R3").unwrap(),
        false,
    )
    .unwrap();
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddComponentInstance(e)),
        Error::InexistentComponentSignal(_)
    ));

    // Connected to an unknown net.
    let mut e = component_instance(&p, lib, variant, "R4");
    let signal = e.signals().keys().next().copied().unwrap();
    e.set_signal_net(&signal, Some(NetSignalId::new_random()));
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddComponentInstance(e)),
        Error::InexistentNetSignal(_)
    ));
}

#[test]
fn batch_rolls_back_on_error() {
    let mut p = new_project();
    let ok = NetClass::new(Uuid::new_random(), ElementName::new("power").unwrap());
    let clash = NetClass::new(Uuid::new_random(), ElementName::new("default").unwrap());
    let before = snapshot(&p);
    let cursor = p.revision();
    let error = p
        .apply(Mutation::Batch(vec![
            Mutation::AddNetClass(ok.clone()),
            Mutation::AddNetClass(clash),
        ]))
        .unwrap_err();
    assert!(matches!(error, Error::DuplicateName { .. }));
    assert_eq!(snapshot(&p), before, "the batch was rolled back");
    assert!(p.is_ref_index_consistent());
    // The rollback is journaled like any other change.
    assert_eq!(
        p.changes_since(cursor),
        ChangesSince::Changes(&[
            Change::NetClassAdded(NetClassId(ok.uuid())),
            Change::NetClassRemoved(NetClassId(ok.uuid())),
        ])
    );

    // A successful batch inverts to the reversed inverses.
    let signal = NetClass::new(Uuid::new_random(), ElementName::new("signal").unwrap());
    let inverse = assert_inverse(
        &mut p,
        Mutation::Batch(vec![
            Mutation::AddNetClass(ok.clone()),
            Mutation::AddNetClass(signal.clone()),
        ]),
    );
    assert_eq!(
        inverse,
        Mutation::Batch(vec![
            Mutation::RemoveNetClass(NetClassId(signal.uuid())),
            Mutation::RemoveNetClass(NetClassId(ok.uuid())),
        ])
    );
}

#[test]
fn journal() {
    let mut p = new_project();
    assert_eq!(p.revision(), 0);
    let cursor = p.revision();
    let gnd = add_net(&mut p, "GND");
    let (cmp, signals) = add_component(&mut p, "R1", Some(gnd));
    assert_eq!(p.revision(), 3);
    assert_eq!(
        p.changes_since(cursor),
        ChangesSince::Changes(&[
            Change::NetSignalAdded(gnd),
            Change::LibraryElementAdded {
                kind: librepcb_core::project::LibraryElementKind::Component,
                uuid: p.circuit().component_instance(cmp).unwrap().lib_component(),
            },
            Change::ComponentAdded(cmp),
        ])
    );
    let cursor = p.revision();
    let signal = ComponentSignalRef {
        component: cmp,
        signal: signals[0],
    };
    p.set_component_signal_net(signal, None).unwrap();
    assert_eq!(
        p.changes_since(cursor),
        ChangesSince::Changes(&[Change::ComponentSignalNetChanged {
            signal,
            from: Some(gnd),
            to: None,
        }])
    );
    assert_eq!(p.changes_since(p.revision()), ChangesSince::Changes(&[]));
}

#[test]
fn assembly_variant_order_is_journaled_and_reversible() {
    let mut project = new_project();
    let first = AssemblyVariantId(project.circuit().assembly_variants()[0].uuid());
    let variant =
        AssemblyVariant::new(Uuid::new_random(), FileProofName::new("Other").unwrap(), "");
    let other = AssemblyVariantId(variant.uuid());
    project.add_assembly_variant(variant, None).unwrap();
    assert_inverse(
        &mut project,
        Mutation::MoveAssemblyVariant {
            variant: other,
            index: 0,
        },
    );
    let before = project.revision();
    let inverse = project
        .apply(Mutation::MoveAssemblyVariant {
            variant: other,
            index: 0,
        })
        .unwrap();
    assert_eq!(
        project.circuit().assembly_variants().uuids(),
        vec![other.0, first.0]
    );
    assert!(matches!(
        project.changes_since(before),
        ChangesSince::Changes([Change::AssemblyVariantOrder])
    ));
    project.apply(inverse).unwrap();
    assert_eq!(
        project.circuit().assembly_variants().uuids(),
        vec![first.0, other.0]
    );
    let before = project.revision();
    project
        .apply(Mutation::MoveAssemblyVariant {
            variant: first,
            index: 0,
        })
        .unwrap();
    assert_eq!(project.revision(), before);
    assert!(
        project
            .apply(Mutation::MoveAssemblyVariant {
                variant: AssemblyVariantId::new_random(),
                index: 0
            })
            .is_err()
    );
}
