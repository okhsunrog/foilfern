//! Keeping scenes up to date with the project's change journal (no
//! upstream counterpart: upstream's graphics items follow the model
//! through Qt signals).

use librepcb_core::project::{ChangesSince, Project};

use crate::board::BoardScene;
use crate::error::Result;
use crate::schematic::SchematicScene;

/// A scene which can be updated incrementally from the project's change
/// journal ([`SchematicScene`], [`BoardScene`]).
pub trait IncrementalScene {
    /// Updates the scene after the project changed; returns whether it
    /// was rebuilt from scratch (see
    /// [`SchematicScene::apply_changes()`],
    /// [`BoardScene::apply_changes()`]).
    fn apply_changes(&mut self, project: &Project, changes: ChangesSince<'_>) -> Result<bool>;
}

impl IncrementalScene for SchematicScene {
    fn apply_changes(&mut self, project: &Project, changes: ChangesSince<'_>) -> Result<bool> {
        SchematicScene::apply_changes(self, project, changes)
    }
}

impl IncrementalScene for BoardScene {
    fn apply_changes(&mut self, project: &Project, changes: ChangesSince<'_>) -> Result<bool> {
        BoardScene::apply_changes(self, project, changes)
    }
}

/// The journal cursor of a scene: pulls the changes since the last sync
/// with [`Project::changes_since()`] and applies them.
///
/// ```ignore
/// let mut scene = BoardScene::build(&project, board, BoardSide::Top, &scheme)?;
/// let mut sync = SceneSync::new(&project);
/// // ... the project is modified ...
/// sync.sync(&project, &mut scene)?;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneSync {
    cursor: u64,
}

impl SceneSync {
    /// A cursor at the current revision of the project (for a scene which
    /// was just built from it).
    pub fn new(project: &Project) -> Self {
        Self::at(project.revision())
    }

    /// A cursor at a given revision.
    pub fn at(cursor: u64) -> Self {
        Self { cursor }
    }

    /// The revision the scene is up to date with.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Whether the project changed since the last sync.
    pub fn is_outdated(&self, project: &Project) -> bool {
        project.revision() != self.cursor
    }

    /// The changes since the last sync; [`ChangesSince::Resync`] if the
    /// journal no longer holds them (or was reset, e.g. for another
    /// project).
    pub fn pending<'a>(&self, project: &'a Project) -> ChangesSince<'a> {
        if self.cursor > project.revision() {
            ChangesSince::Resync
        } else {
            project.changes_since(self.cursor)
        }
    }

    /// Applies the changes since the last sync to a scene and advances the
    /// cursor; returns whether the scene was rebuilt from scratch. On error
    /// the cursor is kept.
    pub fn sync(&mut self, project: &Project, scene: &mut impl IncrementalScene) -> Result<bool> {
        if !self.is_outdated(project) {
            return Ok(false);
        }
        let rebuilt = scene.apply_changes(project, self.pending(project))?;
        self.cursor = project.revision();
        Ok(rebuilt)
    }
}
