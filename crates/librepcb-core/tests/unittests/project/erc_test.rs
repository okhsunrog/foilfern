//! Tests of the electrical rule check, the project attribute lookup and the
//! BOM generator on in-memory projects (upstream has no unit tests of
//! these; real projects are compared with the upstream CLI in
//! `tests/project_exports.rs`).

use librepcb_core::attribute::{Attribute, AttributeKey, AttributeType};
use librepcb_core::export::BomCsvWriter;
use librepcb_core::project::erc::{ErcMessageKind, run_erc};
use librepcb_core::project::{AssemblyVariantId, BomGenerator, Mutation, ProjectAttributeLookup};
use librepcb_core::rule_check::{Severity, all_approvals};
use librepcb_core::serialization::Mode;

use super::{add_component, add_net, add_net_class, new_project};

#[test]
fn open_net_and_unconnected_signals() {
    let mut p = new_project();
    let net = add_net(&mut p, "N1");
    // One component with two signals on `net` -> not open.
    add_component(&mut p, "R1", Some(net));
    let open = add_net(&mut p, "N2");
    let msgs = run_erc(&p);
    let kinds: Vec<_> = msgs.iter().map(|m| m.kind()).collect();
    assert!(kinds.contains(&ErcMessageKind::OpenNet(open)));
    assert!(!kinds.contains(&ErcMessageKind::OpenNet(net)));
    let msg = msgs
        .iter()
        .find(|m| m.kind() == ErcMessageKind::OpenNet(open))
        .unwrap();
    assert_eq!(msg.message().severity(), Severity::Warning);
    assert_eq!(msg.message().message(), "Less than two pins in net: 'N2'");
    assert_eq!(
        msg.approval().to_string_with_mode(Mode::LibrePcb).unwrap(),
        format!("(approved open_net (net {}))\n", open.0)
    );
    // No schematic, so no location.
    assert_eq!(msg.schematic(), None);
    assert!(msg.message().locations().is_empty());
}

#[test]
fn unused_net_class() {
    let mut p = new_project();
    // Only one (the default) net class: no message.
    assert!(
        !run_erc(&p)
            .iter()
            .any(|m| matches!(m.kind(), ErcMessageKind::UnusedNetClass(_)))
    );
    let nc = add_net_class(&mut p, "Power");
    let msgs = run_erc(&p);
    let unused: Vec<_> = msgs
        .iter()
        .filter(|m| matches!(m.kind(), ErcMessageKind::UnusedNetClass(_)))
        .collect();
    // Both the default and the new net class are unused.
    assert_eq!(unused.len(), 2);
    let msg = unused
        .iter()
        .find(|m| m.kind() == ErcMessageKind::UnusedNetClass(nc))
        .unwrap();
    assert_eq!(msg.message().severity(), Severity::Hint);
    assert_eq!(msg.message().message(), "Unused net class: 'Power'");
    assert_eq!(
        msg.approval().to_string_with_mode(Mode::LibrePcb).unwrap(),
        format!("(approved unused_netclass (netclass {}))\n", nc.0)
    );
}

#[test]
fn approvals_are_unique_per_item() {
    let mut p = new_project();
    add_net(&mut p, "A");
    add_net(&mut p, "B");
    let msgs = run_erc(&p);
    let approvals = all_approvals(msgs.iter().map(|m| m.message()));
    assert_eq!(approvals.len(), msgs.len());
}

#[test]
fn attribute_lookup_chain() {
    let mut p = new_project();
    let mut metadata = p.metadata().clone();
    metadata.attributes.push(
        Attribute::new(
            AttributeKey::new("FOO").unwrap(),
            AttributeType::String,
            "project",
            None,
        )
        .unwrap(),
    );
    p.apply(Mutation::SetProjectMetadata(metadata)).unwrap();
    let (cmp, _) = add_component(&mut p, "R1", None);
    let cmp = p.circuit().component_instance(cmp).unwrap();

    let lookup = ProjectAttributeLookup::for_project(&p, None);
    assert_eq!(lookup.value("FOO").as_deref(), Some("project"));
    assert_eq!(
        lookup.value("PROJECT_FILENAME").as_deref(),
        Some("test.lpp")
    );
    assert_eq!(lookup.value("PROJECT_BASENAME").as_deref(), Some("test"));
    assert_eq!(lookup.value("PAGES").as_deref(), Some("0"));
    assert_eq!(lookup.value("VARIANT"), None);
    assert_eq!(lookup.value("NAME"), None);
    assert_eq!(lookup.value("UNKNOWN"), None);
    assert_eq!(
        lookup.substitute("{{PAGE_X_OF_Y}}"),
        // No schematic: {{PAGE}} is unknown (removed with one space).
        "Page of 0"
    );

    let av = p.circuit().assembly_variants().iter().next().unwrap();
    let lookup = ProjectAttributeLookup::for_project(&p, Some(av));
    assert_eq!(lookup.value("VARIANT").as_deref(), Some("Std"));
    assert_eq!(lookup.value("VARIANT_INDEX").as_deref(), Some("0"));

    let lookup = ProjectAttributeLookup::for_component(&p, cmp, None, None);
    assert_eq!(lookup.value("NAME").as_deref(), Some("R1"));
    assert_eq!(lookup.value("COMPONENT").as_deref(), Some("Test Component"));
    assert_eq!(lookup.value("FOO").as_deref(), Some("project"));
    assert_eq!(lookup.value("DEVICE"), None);
    assert_eq!(lookup.substitute("{{NAME}}: {{MPN or 'n/a'}}"), "R1: n/a");
}

#[test]
fn bom_of_project() {
    let mut p = new_project();
    add_component(&mut p, "R2", None);
    add_component(&mut p, "R1", None);
    let av = AssemblyVariantId(
        p.circuit()
            .assembly_variants()
            .iter()
            .next()
            .unwrap()
            .uuid(),
    );
    let mut generator = BomGenerator::new(&p);
    generator.set_additional_attributes(vec!["FOO".to_owned(), "BAR[]".to_owned()]);
    let bom = generator.generate(None, av);
    assert_eq!(
        bom.columns(),
        ["Package", "FOO", "Value", "MPN", "Manufacturer", "BAR"]
    );
    // Without assembly options, components are not mounted.
    assert_eq!(bom.items().len(), 1);
    assert_eq!(bom.items()[0].designators(), ["R1", "R2"]);
    assert!(!bom.items()[0].is_mount());
    assert_eq!(bom.items()[0].attributes(), ["N/A", "", "", "", "", ""]);
    let mut writer = BomCsvWriter::new(&bom);
    writer.set_include_non_mounted_parts(true);
    let csv = writer.generate_csv().unwrap().to_csv_string().unwrap();
    assert!(csv.contains("0,\"R1, R2\",N/A,,,,,"), "{csv}");
}

/// Copies the upstream test project `name` into a temporary directory.
fn copy_test_project(name: &str) -> (crate::helpers::TempDir, std::path::PathBuf) {
    let src = crate::helpers::test_data_dir().join("projects").join(name);
    let tmp = crate::helpers::TempDir::new();
    let dst = tmp.path().as_path().join(name);
    for entry in walkdir::WalkDir::new(&src) {
        let entry = entry.unwrap();
        let target = dst.join(entry.path().strip_prefix(&src).unwrap());
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
    (tmp, dst)
}

fn open_project(dir: &std::path::Path, writable: bool) -> librepcb_core::project::Project {
    use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
    let path = FilePath::new(dir).unwrap();
    let fs = if writable {
        TransactionalFileSystem::open_rw(&path)
    } else {
        TransactionalFileSystem::open_ro(&path)
    }
    .unwrap();
    librepcb_core::project::ProjectLoader::new()
        .open(
            TransactionalDirectory::new(std::sync::Arc::new(fs), ""),
            "test project.lpp",
        )
        .unwrap()
}

/// After a file format migration, only the approvals of messages which
/// still occur are kept (upstream `ProjectLoader::open()`).
#[test]
fn obsolete_approvals_removed_after_migration() {
    let obsolete = "(approved open_net (net 00000000-0000-4000-8000-000000000000))";

    let (_tmp, dir) = copy_test_project("v1");
    // An approval of the upgraded project.
    let msgs = run_erc(&open_project(&dir, false));
    let valid = msgs[0].approval().clone();
    let valid_str = valid.to_string_with_mode(Mode::LibrePcb).unwrap();
    std::fs::write(
        dir.join("circuit/erc.lp"),
        format!("(librepcb_erc\n {}\n {obsolete}\n)\n", valid_str.trim_end()),
    )
    .unwrap();
    let p = open_project(&dir, true);
    assert_eq!(
        p.erc_approvals().iter().collect::<Vec<_>>(),
        vec![&valid],
        "{:?}",
        p.erc_approvals()
    );
    drop(p);

    // Without migration, approvals are kept as they are.
    let (_tmp, dir) = copy_test_project("v1");
    let mut p = open_project(&dir, true);
    p.save().unwrap();
    p.directory().file_system().save().unwrap();
    drop(p);
    std::fs::write(
        dir.join("circuit/erc.lp"),
        format!("(librepcb_erc\n {obsolete}\n)\n"),
    )
    .unwrap();
    let p = open_project(&dir, false);
    assert_eq!(p.erc_approvals().len(), 1);
}
