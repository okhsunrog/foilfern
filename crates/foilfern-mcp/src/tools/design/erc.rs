//! Read-only comparison of ERC on the source project and reviewed patch.

use std::collections::{BTreeMap, VecDeque};

use foilfern_core::project::erc::{ErcMessage, ErcMessageKind, run_erc};
use foilfern_core::project::{Project, SchematicId};
use foilfern_core::rule_check::Severity;
use foilfern_core::types::Uuid;
use serde_json::{Value, json};

use super::super::output::erc_message_json;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Subject {
    OpenWire(SchematicId, Uuid),
    NetJunction(SchematicId, Uuid),
    BusJunction(SchematicId, Uuid),
    Other(ErcMessageKind),
}

type Key = (Subject, Severity);

fn key(message: &ErcMessage) -> Key {
    let subject = match message.kind() {
        // Segment splits can give a surviving line or junction a new parent ID.
        ErcMessageKind::OpenWireInSegment { segment, line } => {
            Subject::OpenWire(segment.schematic, line)
        }
        ErcMessageKind::UnconnectedNetJunction { segment, junction } => {
            Subject::NetJunction(segment.schematic, junction)
        }
        ErcMessageKind::UnconnectedBusJunction { segment, junction } => {
            Subject::BusJunction(segment.schematic, junction)
        }
        other => Subject::Other(other),
    };
    (subject, message.message().severity())
}

pub(super) fn compare(before: &Project, after: &Project) -> Value {
    let mut baseline = BTreeMap::<_, VecDeque<_>>::new();
    for message in run_erc(before) {
        baseline
            .entry(key(&message))
            .or_default()
            .push_back(message);
    }
    let mut introduced = Vec::new();
    let mut inherited = Vec::new();
    for message in run_erc(after) {
        let matched = baseline
            .get_mut(&key(&message))
            .and_then(VecDeque::pop_front);
        let mut output = erc_message_json(after, &message);
        if let Some(previous) = matched {
            let was_approved = before.erc_approvals().contains(previous.approval());
            let is_approved = after.erc_approvals().contains(message.approval());
            if was_approved != is_approved {
                output["previously_approved"] = json!(was_approved);
            }
            inherited.push(output);
        } else {
            introduced.push(output);
        }
    }
    let resolved = baseline
        .into_values()
        .flatten()
        .map(|message| erc_message_json(before, &message))
        .collect::<Vec<_>>();
    json!({"introduced": introduced, "inherited": inherited, "resolved": resolved})
}
