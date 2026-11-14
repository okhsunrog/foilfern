//! Change notification of the project model (no upstream counterpart:
//! upstream uses Qt signals such as `Circuit::netSignalAdded()` and the
//! `onEdited` signals of the items).
//!
//! Every applied [`Mutation`](super::Mutation) appends the entity level
//! events upstream would have emitted to the project's [`ChangeLog`], a
//! bounded journal. Consumers (scene builders, rule check panels, MCP
//! subscriptions) pull with their own cursor via
//! [`Project::changes_since()`](super::Project::changes_since); a cursor
//! that fell off the ring gets [`ChangesSince::Resync`] and rebuilds from
//! scratch.

use super::id::{
    AssemblyVariantId, BoardId, BusId, ComponentInstanceId, ComponentSignalRef, NetClassId,
    NetSignalId, SchematicId,
};
use super::library::LibraryElementKind;
use super::{BoardChange, SchematicChange};
use crate::types::Uuid;

/// Default capacity of the change log (entries kept for lagging consumers).
pub const DEFAULT_CHANGE_LOG_CAPACITY: usize = 4096;

/// A change of the project model.
///
/// Serde: externally tagged enum.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum Change {
    /// Name, author, version, creation date or attributes changed
    /// (upstream `Project::attributesChanged()`).
    ProjectMetadata,
    /// Locale order, norm order, BOM attributes or the assembly lock default
    /// changed (upstream `Project::attributesChanged()` /
    /// `normOrderChanged()`).
    ProjectSettings,
    /// A file in the project's asset overlay changed.
    ProjectAsset,
    /// The output jobs changed.
    OutputJobs,
    /// The ERC message approvals changed.
    ErcApprovals,
    /// An assembly variant was added.
    AssemblyVariantAdded(AssemblyVariantId),
    /// An assembly variant was removed.
    AssemblyVariantRemoved(AssemblyVariantId),
    /// Name or description of an assembly variant changed.
    AssemblyVariantChanged(AssemblyVariantId),
    /// Assembly variant order changed (the first variant is the default).
    AssemblyVariantOrder,
    /// A net class was added.
    NetClassAdded(NetClassId),
    /// A net class was removed.
    NetClassRemoved(NetClassId),
    /// Name or design rules of a net class changed (upstream
    /// `NetClass::designRulesModified()`).
    NetClassChanged(NetClassId),
    /// A net signal was added.
    NetSignalAdded(NetSignalId),
    /// A net signal was removed.
    NetSignalRemoved(NetSignalId),
    /// Name, auto-name flag or net class of a net signal changed (upstream
    /// `NetSignal::nameChanged()`).
    NetSignalChanged(NetSignalId),
    /// A bus was added.
    BusAdded(BusId),
    /// A bus was removed.
    BusRemoved(BusId),
    /// Name or properties of a bus changed (upstream `Bus::nameChanged()`,
    /// `Circuit::busRenamed()`).
    BusChanged(BusId),
    /// A component instance was added.
    ComponentAdded(ComponentInstanceId),
    /// A component instance was removed.
    ComponentRemoved(ComponentInstanceId),
    /// Name, value, attributes, assembly options or lock flag of a
    /// component instance changed (upstream
    /// `ComponentInstance::attributesChanged()`).
    ComponentChanged(ComponentInstanceId),
    /// The net of a component signal changed (upstream
    /// `ComponentSignalInstance::netSignalChanged()`).
    ComponentSignalNetChanged {
        /// The component signal.
        signal: ComponentSignalRef,
        /// The previous net.
        from: Option<NetSignalId>,
        /// The new net.
        to: Option<NetSignalId>,
    },
    /// An element was added to the project library.
    LibraryElementAdded {
        /// Kind of the element.
        kind: LibraryElementKind,
        /// UUID of the element.
        uuid: Uuid,
    },
    /// An element was removed from the project library.
    LibraryElementRemoved {
        /// Kind of the element.
        kind: LibraryElementKind,
        /// UUID of the element.
        uuid: Uuid,
    },
    /// The artwork of a symbol in the project library changed.
    LibrarySymbolArtworkChanged(Uuid),
    /// A schematic page was added (upstream `Project::schematicAdded()`).
    SchematicAdded(SchematicId),
    /// A schematic page was removed.
    SchematicRemoved(SchematicId),
    /// Name or grid settings of a schematic changed (upstream
    /// `Schematic::nameChanged()`).
    SchematicChanged(SchematicId),
    /// An item of a schematic changed.
    Schematic {
        /// The schematic.
        id: SchematicId,
        /// The change.
        change: SchematicChange,
    },
    /// A board was added (upstream `Project::boardAdded()`).
    BoardAdded(BoardId),
    /// A board was removed.
    BoardRemoved(BoardId),
    /// Name or settings of a board changed (upstream
    /// `Board::attributesChanged()`).
    BoardChanged(BoardId),
    /// An item of a board changed.
    Board {
        /// The board.
        id: BoardId,
        /// The change.
        change: BoardChange,
    },
}

/// Result of [`ChangeLog::changes_since()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesSince<'a> {
    /// The changes after the given revision, oldest first.
    Changes(&'a [Change]),
    /// The revision is too old, the consumer must rebuild from scratch.
    Resync,
}

/// Bounded journal of [`Change`]s with monotonically increasing sequence
/// numbers (the project revision).
#[derive(Debug, Clone)]
pub struct ChangeLog {
    /// Sequence number of `entries[0]` (the first entry ever has number 1;
    /// revision 0 is "nothing changed yet").
    first_seq: u64,
    entries: Vec<Change>,
    capacity: usize,
}

impl ChangeLog {
    /// Creates an empty log keeping at most `capacity` entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            first_seq: 1,
            entries: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    /// Returns the sequence number of the latest change (0 if none).
    pub fn revision(&self) -> u64 {
        self.first_seq + self.entries.len() as u64 - 1
    }

    /// Appends a change and returns its sequence number.
    pub fn push(&mut self, change: Change) -> u64 {
        if self.entries.len() >= self.capacity {
            // Drop the older half at once (amortized O(1)).
            let drop = self.entries.len() / 2 + 1;
            self.entries.drain(..drop);
            self.first_seq += drop as u64;
        }
        self.entries.push(change);
        self.revision()
    }

    /// Returns the changes with a sequence number greater than `revision`
    /// (the value a consumer got from [`revision()`](Self::revision) last
    /// time), or [`ChangesSince::Resync`] if they are no longer available.
    pub fn changes_since(&self, revision: u64) -> ChangesSince<'_> {
        if revision + 1 < self.first_seq {
            return ChangesSince::Resync;
        }
        let start = (revision + 1 - self.first_seq) as usize;
        ChangesSince::Changes(self.entries.get(start..).unwrap_or(&[]))
    }

    /// Forgets all changes but keeps the revision counter.
    pub fn clear(&mut self) {
        self.first_seq = self.revision() + 1;
        self.entries.clear();
    }

    /// Advances the revision and forces earlier readers to resync.
    pub(crate) fn invalidate(&mut self) {
        self.clear();
        self.first_seq += 1;
    }

    /// Forgets all changes and restarts the revision counter at 0 (for a
    /// freshly created or opened project).
    pub fn reset(&mut self) {
        self.first_seq = 1;
        self.entries.clear();
    }
}

impl Default for ChangeLog {
    fn default() -> Self {
        Self::new(DEFAULT_CHANGE_LOG_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_and_resync() {
        let mut log = ChangeLog::new(4);
        assert_eq!(log.revision(), 0);
        assert_eq!(log.changes_since(0), ChangesSince::Changes(&[]));
        assert_eq!(log.push(Change::ProjectMetadata), 1);
        assert_eq!(log.push(Change::ProjectSettings), 2);
        assert_eq!(
            log.changes_since(0),
            ChangesSince::Changes(&[Change::ProjectMetadata, Change::ProjectSettings])
        );
        assert_eq!(
            log.changes_since(1),
            ChangesSince::Changes(&[Change::ProjectSettings])
        );
        assert_eq!(log.changes_since(2), ChangesSince::Changes(&[]));
        for _ in 0..3 {
            log.push(Change::ErcApprovals);
        }
        assert_eq!(log.revision(), 5);
        assert_eq!(log.changes_since(0), ChangesSince::Resync);
        assert_eq!(log.changes_since(1), ChangesSince::Resync);
        assert_eq!(log.changes_since(5), ChangesSince::Changes(&[]));
        log.clear();
        assert_eq!(log.revision(), 5);
        assert_eq!(log.changes_since(5), ChangesSince::Changes(&[]));
        assert_eq!(log.changes_since(4), ChangesSince::Resync);
    }
}
