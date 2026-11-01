//! ERC tests. Upstream has no core/project/erc unit tests; the open-net
//! vector below is from tests/cli/open_project/test_run_erc.py.

use super::*;
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::erc::{
    ElectricalRuleCheck, ElectricalRuleCheckMessage, ErcMessageKind as Kind,
};
use librepcb_core::project::{ComponentSignalRef, ProjectLoader};
use librepcb_core::rule_check::Severity;
use librepcb_core::serialization::Mode;

fn check(p: &Project) -> Vec<ElectricalRuleCheckMessage> {
    ElectricalRuleCheck::new(p).run_checks().unwrap()
}

#[test]
fn empty_project_and_unused_net_classes() {
    let mut p = new_project();
    assert!(check(&p).is_empty());
    let other = add_net_class(&mut p, "Unused");
    let messages = check(&p);
    assert_eq!(messages.len(), 2);
    assert!(
        messages
            .iter()
            .all(|m| matches!(m.kind(), Kind::UnusedNetClass(_)))
    );
    add_net(&mut p, "N");
    let messages = check(&p);
    assert_eq!(
        messages
            .iter()
            .filter(|m| matches!(m.kind(), Kind::UnusedNetClass(id) if *id == other))
            .count(),
        1
    );
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m.kind(), Kind::UnusedNetClass(id) if *id == default_net_class(&p)))
    );
}

#[test]
fn upstream_open_net_vector_and_approved_findings_are_still_returned() {
    let mut p = new_project();
    let id: Uuid = "0654411b-a090-4025-a026-4c61e686662e".parse().unwrap();
    p.add_net_signal(NetSignal::new(
        id,
        default_net_class(&p),
        "N".parse().unwrap(),
        false,
    ))
    .unwrap();
    let messages = check(&p);
    assert_eq!(messages.len(), 1);
    let message = messages[0].rule_message();
    assert_eq!(message.severity(), Severity::Warning);
    assert_eq!(message.message(), "Less than two pins in net: 'N'");
    assert_eq!(
        message
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        "(approved open_net (net 0654411b-a090-4025-a026-4c61e686662e))\n"
    );
    assert!(message.locations().is_empty());
    assert_eq!(messages[0].schematic(), None);
    p.apply(Mutation::SetErcApproval {
        approval: message.approval().clone(),
        approved: true,
    })
    .unwrap();
    let before = snapshot(&p);
    let revision = p.revision();
    assert_eq!(check(&p), messages);
    assert_eq!(snapshot(&p), before);
    assert_eq!(p.revision(), revision);
}

#[test]
fn physical_signals_count_but_schematic_only_signals_do_not() {
    let mut p = new_project();
    let net = add_net(&mut p, "N");
    add_component(&mut p, "R1", Some(net)); // Two real signals on one component count as two.
    assert!(
        !check(&p)
            .iter()
            .any(|m| matches!(m.kind(), Kind::OpenNet(_)))
    );
    let virtual_net = add_net(&mut p, "VIRTUAL");
    let (mut lib, variant) = library_component(Uuid::new_random(), 3, Uuid::new_random());
    lib.set_schematic_only(true);
    let lib_id = lib.metadata().uuid();
    let signals = lib.signals().uuids();
    p.add_library_component(lib).unwrap();
    let mut component = component_instance(&p, lib_id, variant, "SUPPLY1");
    for signal in signals {
        component
            .set_signal_net(&signal, Some(virtual_net))
            .unwrap();
    }
    p.add_component_instance(component).unwrap();
    assert!(
        check(&p)
            .iter()
            .any(|m| matches!(m.kind(), Kind::OpenNet(id) if *id == virtual_net))
    );
}

#[test]
fn required_signal_takes_precedence_over_forced_name_and_names_are_expanded() {
    let mut p = new_project();
    let (mut lib, variant) = library_component(Uuid::new_random(), 2, Uuid::new_random());
    let signals = lib.signals().uuids();
    lib.signals_mut()
        .by_uuid_mut(&signals[0])
        .unwrap()
        .set_is_required(true);
    for signal in &signals {
        lib.signals_mut()
            .by_uuid_mut(signal)
            .unwrap()
            .set_forced_net_name("{{VALUE}}".into());
    }
    let lib_id = lib.metadata().uuid();
    p.add_library_component(lib).unwrap();
    let mut component = component_instance(&p, lib_id, variant, "U1");
    component.set_value("EXPECTED".into());
    let id = p.add_component_instance(component).unwrap();
    let messages = check(&p);
    assert!(
        messages.iter().any(
            |m| matches!(m.kind(), Kind::UnconnectedRequiredSignal(s) if s.signal == signals[0])
        )
    );
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m.kind(), Kind::ForcedNetNameConflict(s) if s.signal == signals[0]))
    );
    let forced = messages
        .iter()
        .find(|m| matches!(m.kind(), Kind::ForcedNetNameConflict(_)))
        .unwrap();
    assert_eq!(
        forced.rule_message().message(),
        "Net name conflict: '' != 'EXPECTED' ('U1:S2')"
    );
    assert_eq!(forced.rule_message().severity(), Severity::Error);
    assert_eq!(
        forced
            .rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!(
            "(approved forced_net_name_conflict\n (component {})\n (signal {})\n)\n",
            id.0, signals[1]
        )
    );
    let net = add_net(&mut p, "EXPECTED");
    for signal in signals {
        p.apply(Mutation::SetComponentSignalNet {
            signal: ComponentSignalRef {
                component: id,
                signal,
            },
            net: Some(net),
        })
        .unwrap();
    }
    assert!(check(&p).is_empty());
}

#[test]
fn unused_bus_has_upstream_approval() {
    let mut p = new_project();
    let id = p
        .add_bus(librepcb_core::project::circuit::Bus::new(
            Uuid::new_random(),
            "BUS".parse().unwrap(),
            false,
            false,
            None,
        ))
        .unwrap();
    let messages = check(&p);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind(), &Kind::UnusedBus(id));
    assert_eq!(messages[0].rule_message().severity(), Severity::Hint);
    assert_eq!(
        messages[0]
            .rule_message()
            .approval()
            .to_string_with_mode(Mode::LibrePcb)
            .unwrap(),
        format!("(approved unused_bus (bus {}))\n", id.0)
    );
}

#[test]
fn loader_removes_obsolete_approvals_only_after_migration() {
    use librepcb_core::fileio::{
        FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem,
    };
    use librepcb_core::serialization::List;
    use std::sync::Arc;

    let source = crate::helpers::test_data_dir().join("projects/v1");
    let filename = std::fs::read_dir(&source)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .find(|name| name.ends_with(".lpp"))
        .unwrap();
    let old_directory = || {
        TransactionalDirectory::new(
            Arc::new(TransactionalFileSystem::open_ro(&FilePath::new(&source).unwrap()).unwrap()),
            "",
        )
    };
    let mut loader = ProjectLoader::new();
    let baseline = loader.open(old_directory(), &filename).unwrap();
    assert!(loader.migration_log().is_some());
    let live = check(&baseline)
        .iter()
        .find(|message| matches!(message.kind(), Kind::OpenNet(_)))
        .expect("v1 fixture must contain an open net")
        .rule_message()
        .approval()
        .clone();
    let mut obsolete = List::new("approved");
    obsolete.push(SExpression::token("obsolete_test_rule"));
    let obsolete = SExpression::List(obsolete);
    let mut erc = List::new("librepcb_erc");
    erc.ensure_line_break();
    erc.push(live.clone());
    erc.ensure_line_break();
    erc.push(obsolete.clone());
    erc.ensure_line_break();
    let bytes = SExpression::List(erc)
        .to_byte_array(Mode::LibrePcb)
        .unwrap();
    // A current-format project keeps existing approvals, even if obsolete.
    let mut current = TransactionalDirectory::new(
        Arc::clone(baseline.directory().file_system()),
        baseline.directory().path(),
    );
    current.write("circuit/erc.lp", &bytes).unwrap();
    let reopened = loader.open(current, &filename).unwrap();
    assert!(loader.migration_log().is_none());
    assert_eq!(
        reopened.erc_approvals(),
        &BTreeSet::from([live.clone(), obsolete])
    );
    // The same approval file in an old project is intersected with the
    // findings after migration; nothing is newly approved.
    let mut old = old_directory();
    old.write("circuit/erc.lp", &bytes).unwrap();
    let upgraded = loader.open(old, &filename).unwrap();
    assert!(loader.migration_log().is_some());
    assert_eq!(upgraded.erc_approvals(), &BTreeSet::from([live.clone()]));
    assert_eq!(upgraded.revision(), 0);
    let expected = format!(
        "(librepcb_erc\n {}\n)\n",
        live.to_string_with_mode(Mode::LibrePcb).unwrap().trim_end()
    );
    assert_eq!(
        String::from_utf8(upgraded.directory().read("circuit/erc.lp").unwrap()).unwrap(),
        expected
    );
}
