//! Tests of the project model (no upstream counterpart: upstream has no
//! unit tests of `core/project`; the round trip of real projects is in
//! `tests/project_roundtrip.rs`).
//!
//! The central helper is [`assert_inverse()`]: applying a mutation and
//! then its inverse restores the previous state, and the reverse index
//! stays consistent with a rebuild from scratch.

mod board;
mod circuit_test;
mod erc_test;
mod json_export_test;
mod output_job_runner_test;
mod project_test;
mod schematic_test;
mod serde_test;

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use librepcb_core::fileio::TransactionalDirectory;
use librepcb_core::job::OutputJobList;
use librepcb_core::library::BaseMetadata;
use librepcb_core::library::cmp::{Component, ComponentSignal, ComponentSymbolVariant};
use librepcb_core::project::board::Board;
use librepcb_core::project::circuit::{Circuit, ComponentInstance, NetClass, NetSignal};
use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{
    ComponentInstanceId, Mutation, NetClassId, NetSignalId, Project, ProjectMetadata,
    ProjectSettings,
};
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{CircuitIdentifier, ElementName, SignalRole, Uuid, Version};

/// Creates a new project in a temporary (in-memory) file system with the
/// default assembly variant and net class.
pub fn new_project() -> Project {
    Project::create(
        TransactionalDirectory::new_temporary().unwrap(),
        "test.lpp",
        Uuid::new_random,
    )
    .unwrap()
}

/// The UUID of the "default" net class of a new project.
pub fn default_net_class(p: &Project) -> NetClassId {
    p.circuit().net_class_by_name("default").unwrap().0
}

/// Creates a library component with `signal_count` signals ("S1", "S2",
/// ...) and one symbol variant `variant`. Returns the component and the
/// variant UUID.
pub fn library_component(uuid: Uuid, signal_count: usize, variant: Uuid) -> (Component, Uuid) {
    let mut cmp = Component::new(BaseMetadata::new(
        uuid,
        "0.1".parse::<Version>().unwrap(),
        "test",
        Utc::now(),
        ElementName::new("Test Component").unwrap(),
        "",
        "",
    ))
    .unwrap();
    for i in 1..=signal_count {
        cmp.signals_mut().push(ComponentSignal::new(
            Uuid::new_random(),
            CircuitIdentifier::new(format!("S{i}")).unwrap(),
            SignalRole::Passive,
            "",
            false,
            false,
            false,
        ));
    }
    cmp.symbol_variants_mut().push(ComponentSymbolVariant::new(
        variant,
        "",
        ElementName::new("default").unwrap(),
        "",
    ));
    (cmp, variant)
}

/// Adds a library component with two signals to the project and returns
/// its UUID, its variant and its signal UUIDs.
pub fn add_library_component(p: &mut Project) -> (Uuid, Uuid, Vec<Uuid>) {
    let uuid = Uuid::new_random();
    let (cmp, variant) = library_component(uuid, 2, Uuid::new_random());
    let signals = cmp.signals().uuids();
    p.add_library_component(cmp).unwrap();
    (uuid, variant, signals)
}

/// Creates a component instance of the library component `lib` named
/// `name` (not added to the project).
pub fn component_instance(p: &Project, lib: Uuid, variant: Uuid, name: &str) -> ComponentInstance {
    ComponentInstance::new(
        Uuid::new_random(),
        p.library().component(&lib).unwrap(),
        variant,
        CircuitIdentifier::new(name).unwrap(),
        false,
    )
    .unwrap()
}

/// Adds a net signal "name" in the default net class.
pub fn add_net(p: &mut Project, name: &str) -> NetSignalId {
    let net_class = default_net_class(p);
    p.add_net_signal(NetSignal::new(
        Uuid::new_random(),
        net_class,
        CircuitIdentifier::new(name).unwrap(),
        false,
    ))
    .unwrap()
}

/// Adds a net class.
pub fn add_net_class(p: &mut Project, name: &str) -> NetClassId {
    p.add_net_class(NetClass::new(
        Uuid::new_random(),
        ElementName::new(name).unwrap(),
    ))
    .unwrap()
}

/// Adds a component instance with two signals, both connected to `net`.
pub fn add_component(
    p: &mut Project,
    name: &str,
    net: Option<NetSignalId>,
) -> (ComponentInstanceId, Vec<Uuid>) {
    let (lib, variant, signals) = add_library_component(p);
    let mut cmp = component_instance(p, lib, variant, name);
    for signal in &signals {
        cmp.set_signal_net(signal, net).unwrap();
    }
    (p.add_component_instance(cmp).unwrap(), signals)
}

/// The comparable state of a project.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    metadata: ProjectMetadata,
    settings: ProjectSettings,
    output_jobs: OutputJobList,
    erc_approvals: BTreeSet<SExpression>,
    circuit: Circuit,
    schematics: Vec<Schematic>,
    boards: Vec<Board>,
    library: BTreeMap<&'static str, Vec<Uuid>>,
}

/// Captures the comparable state of `p`.
pub fn snapshot(p: &Project) -> Snapshot {
    Snapshot {
        metadata: p.metadata().clone(),
        settings: p.settings().clone(),
        output_jobs: p.output_jobs().clone(),
        erc_approvals: p.erc_approvals().clone(),
        circuit: p.circuit().clone(),
        schematics: p.schematics().to_vec(),
        boards: p.boards().to_vec(),
        library: BTreeMap::from([
            ("sym", p.library().symbols().keys().copied().collect()),
            ("pkg", p.library().packages().keys().copied().collect()),
            ("cmp", p.library().components().keys().copied().collect()),
            ("dev", p.library().devices().keys().copied().collect()),
        ]),
    }
}

/// Applies `m`, checks that the state changed and the reverse index is
/// consistent, applies the inverse and checks that the previous state is
/// restored, re-applies the inverse of the inverse (redo) and checks that
/// the state after `m` is reached again, then undoes once more so that
/// the project is left in its original state. Returns the inverse of `m`.
pub fn assert_inverse(p: &mut Project, m: Mutation) -> Mutation {
    let before = snapshot(p);
    let revision = p.revision();
    let inverse = p.apply(m).expect("mutation applies");
    assert!(p.revision() > revision, "a change was journaled");
    assert!(p.is_ref_index_consistent());
    let after = snapshot(p);
    assert_ne!(after, before, "the mutation changed the state");
    let redo = p.apply(inverse.clone()).expect("inverse applies");
    assert_eq!(snapshot(p), before, "the inverse restores the state");
    assert!(p.is_ref_index_consistent());
    let undo_again = p.apply(redo).expect("redo applies");
    assert_eq!(
        snapshot(p),
        after,
        "redo reaches the state after the mutation"
    );
    assert!(p.is_ref_index_consistent());
    assert_eq!(undo_again, inverse, "redo yields the same inverse");
    p.apply(undo_again).expect("undo applies again");
    assert_eq!(snapshot(p), before, "left in the original state");
    inverse
}

/// Applies `m` and asserts that it fails and left the state untouched.
pub fn assert_fails(p: &mut Project, m: Mutation) -> librepcb_core::project::Error {
    let before = snapshot(p);
    let revision = p.revision();
    let error = p.apply(m).expect_err("mutation fails");
    assert_eq!(
        snapshot(p),
        before,
        "a failed mutation leaves the state untouched"
    );
    assert_eq!(p.revision(), revision, "a failed mutation journals nothing");
    assert!(p.is_ref_index_consistent());
    error
}

mod attribute_lookup_test;
