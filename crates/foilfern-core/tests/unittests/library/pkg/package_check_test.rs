//! Tests of the package check (no upstream unit test exists) against the
//! output of the upstream CLI (`librepcb-cli open-library --all --check`,
//! LibrePCB 2.1.1, C locale).
//!
//! The packages in `check_fixtures/` trigger every package check message:
//! hand-written cases, and one randomly generated package with pads,
//! holes and legend lines near the check thresholds (where the curve
//! approximations of Qt matter). `expected.txt` is the upstream CLI output
//! for these packages without message approvals. The package files contain
//! the approvals written by this implementation for all messages, which
//! the upstream CLI accepted (no non-approved messages left).

use std::path::Path;

use foilfern_core::fileio::FilePath;
use foilfern_core::library::LibraryBaseElement;
use foilfern_core::library::pkg::Package;
use foilfern_core::rule_check::RuleCheckMessage;

use crate::library::open_dir;

fn fixtures_dir() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/unittests/library/pkg/check_fixtures"
    ))
}

/// Returns the expected messages per package (`"name (uuid)"` and the
/// message lines).
fn expected() -> Vec<(String, Vec<String>)> {
    let content = std::fs::read_to_string(fixtures_dir().join("expected.txt")).unwrap();
    let mut result: Vec<(String, Vec<String>)> = Vec::new();
    for line in content.lines() {
        if let Some(header) = line.strip_prefix("  - ") {
            result.push((header.trim_end_matches(':').to_owned(), Vec::new()));
        } else if let Some(msg) = line.strip_prefix("    - ") {
            result.last_mut().unwrap().1.push(msg.to_owned());
        }
    }
    result
}

#[test]
fn test_package_check_against_upstream() {
    foilfern_i18n::set_language("en").unwrap();
    let expected = expected();
    assert_eq!(expected.len(), 6);
    for (header, expected_msgs) in expected {
        let uuid = &header[header.rfind('(').unwrap() + 1..header.len() - 1];
        let dir = FilePath::new(fixtures_dir().join(uuid)).unwrap();
        let package = Package::open(open_dir(&dir, false)).unwrap();
        assert_eq!(
            header,
            format!(
                "{} ({})",
                package.metadata().name(),
                package.metadata().uuid()
            )
        );

        // Sorted like the CLI: by severity (descending), then by message
        // (case insensitive).
        let mut msgs: Vec<RuleCheckMessage> = package
            .run_checks()
            .unwrap()
            .iter()
            .map(RuleCheckMessage::from)
            .collect();
        msgs.sort_by(|a, b| {
            b.severity()
                .cmp(&a.severity())
                .then_with(|| a.message().to_lowercase().cmp(&b.message().to_lowercase()))
        });
        let actual: Vec<String> = msgs
            .iter()
            .map(|m| {
                format!(
                    "[{}] {}",
                    m.severity().name_tr().to_uppercase(),
                    m.message()
                )
            })
            .collect();
        assert_eq!(actual, expected_msgs, "{header}");

        // All messages are approved by the approvals in the file.
        for msg in &msgs {
            assert!(
                package
                    .metadata()
                    .message_approvals()
                    .contains(msg.approval()),
                "{header}: not approved: {}",
                msg.message()
            );
        }
        assert_eq!(package.metadata().message_approvals().len(), {
            let mut approvals: Vec<_> = msgs.iter().map(|m| m.approval().clone()).collect();
            approvals.sort();
            approvals.dedup();
            approvals.len()
        });
    }
}
