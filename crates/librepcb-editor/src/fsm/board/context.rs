//! The context the board editor states work in: the project editor, the
//! view, the clipboard and the FSM outputs (upstream
//! `BoardEditorFsm::Context` and the helpers of `BoardEditorState`).

use librepcb_core::project::board::Board;
use librepcb_core::project::{BoardId, Mutation, NetSignalId, Project};
use librepcb_core::types::{Layer, LengthUnit, Point, PositiveLength};

use super::BoardEditorSettings;
use super::output::{BoardRequest, Output};
use super::selection::BoardSelection;
use super::view::{BoardItemRef, BoardView, FindFilter, FindFlags, find_items_at_pos};
use crate::commands::ApplyMutations;
use crate::editor::{Command, ProjectEditor};
use crate::error::{Error, Result};
use crate::fsm::{Clipboard, CursorShape};

/// The context of an FSM call: what the application passes to every call.
pub struct BoardContext<'a> {
    /// The project editor (commands, undo groups).
    pub editor: &'a mut ProjectEditor,
    /// Hit testing on the view.
    pub view: &'a dyn BoardView,
    /// The clipboard.
    pub clipboard: &'a mut dyn Clipboard,
}

impl<'a> BoardContext<'a> {
    /// Creates a context.
    pub fn new(
        editor: &'a mut ProjectEditor,
        view: &'a dyn BoardView,
        clipboard: &'a mut dyn Clipboard,
    ) -> Self {
        Self {
            editor,
            view,
            clipboard,
        }
    }
}

impl std::fmt::Debug for BoardContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardContext").finish_non_exhaustive()
    }
}

/// The context of a state while it processes an input.
pub(crate) struct Cx<'a, 'b> {
    pub ctx: &'a mut BoardContext<'b>,
    pub out: &'a mut Output,
    pub selection: &'a mut BoardSelection,
    pub board: BoardId,
    pub settings: &'a BoardEditorSettings,
    /// Last known cursor position (upstream `QCursor::pos()` mapped to the
    /// scene).
    pub cursor_pos: Point,
    /// Whether the left button is held (upstream `e.buttons`).
    pub left_button: bool,
    /// Set by a state to leave to the select tool after processing
    /// (upstream signal `requestLeavingState()`).
    pub leave_requested: bool,
}

impl Cx<'_, '_> {
    pub fn project(&self) -> &Project {
        self.ctx.editor.project()
    }

    pub fn board_id(&self) -> BoardId {
        self.board
    }

    pub fn board(&self) -> Result<&Board> {
        self.project()
            .board(self.board)
            .ok_or_else(|| Error::not_found("Board", self.board))
    }

    pub fn grid(&self) -> PositiveLength {
        super::view::grid_interval(self.project(), self.board)
    }

    /// Length unit of the board (upstream `getLengthUnit()`).
    pub fn unit(&self) -> LengthUnit {
        self.board()
            .map(|b| b.settings().grid_unit)
            .unwrap_or_default()
    }

    pub fn ignore_locks(&self) -> bool {
        self.settings.ignore_locks
    }

    pub fn inner_layer_count(&self) -> usize {
        self.board()
            .map(|b| b.settings().inner_layer_count as usize)
            .unwrap_or(0)
    }

    /// Copper layers of the board, sorted.
    pub fn copper_layers(&self) -> Vec<Layer> {
        self.board()
            .map(|b| b.copper_layers().into_iter().collect())
            .unwrap_or_default()
    }

    /// Upstream `BoardEditorState::findItemsAtPos()`.
    pub fn find_items(
        &self,
        pos: Point,
        flags: FindFlags,
        filter: &FindFilter<'_>,
    ) -> Vec<BoardItemRef> {
        find_items_at_pos(
            self.project(),
            self.board,
            self.ctx.view,
            pos,
            flags,
            filter,
        )
    }

    /// Upstream `BoardEditorState::findItemAtPos()`.
    pub fn find_item(
        &self,
        pos: Point,
        flags: FindFlags,
        filter: &FindFilter<'_>,
    ) -> Option<BoardItemRef> {
        self.find_items(pos, flags, filter).into_iter().next()
    }

    pub fn is_group_active(&self) -> bool {
        self.ctx.editor.undo_stack().is_group_active()
    }

    pub fn group_len(&self) -> usize {
        self.ctx.editor.active_group_len().unwrap_or(0)
    }

    pub fn begin(&mut self, text: impl Into<String>) -> Result<()> {
        self.ctx.editor.begin_group(text)
    }

    pub fn commit(&mut self) -> Result<bool> {
        self.ctx.editor.commit_group()
    }

    pub fn abort(&mut self) {
        if self.is_group_active()
            && let Err(e) = self.ctx.editor.abort_group()
        {
            log::error!("Failed to abort the command group: {e}");
        }
    }

    /// Rolls back the active group to `len` operations.
    pub fn rollback_to(&mut self, len: usize) {
        self.ctx.editor.rollback_group_to(len);
    }

    /// Executes a command (in the active group, if any).
    pub fn exec<C: Command>(&mut self, command: C) -> Result<C::Output> {
        self.ctx.editor.execute(command)
    }

    /// Applies mutations (in the active group, if any; else as a group
    /// with the given text).
    pub fn apply(&mut self, text: String, mutations: Vec<Mutation>) -> Result<()> {
        if mutations.is_empty() {
            return Ok(());
        }
        self.exec(ApplyMutations {
            text: Some(text),
            mutations,
        })
    }

    /// Replaces the preview after `mark` in the active group by the given
    /// mutations and rebuilds the air wires.
    pub fn preview(&mut self, mark: usize, mutations: Vec<Mutation>) -> Result<()> {
        self.rollback_to(mark);
        let result = if mutations.is_empty() {
            Ok(())
        } else {
            self.ctx.editor.execute(ApplyMutations {
                text: None,
                mutations,
            })
        };
        self.rebuild_air_wires();
        result
    }

    /// Rebuilds the scheduled air wires (upstream
    /// `triggerAirWiresRebuild()`).
    pub fn rebuild_air_wires(&mut self) {
        let board = self.board;
        if let Err(e) = self.ctx.editor.rebuild_air_wires(board) {
            log::warn!("Failed to rebuild air wires: {e}");
        }
    }

    /// Reports an error (upstream `QMessageBox::critical()`).
    pub fn error(&mut self, e: impl std::fmt::Display) {
        let msg = e.to_string();
        log::warn!("Board editor: {msg}");
        self.out.requests.push(BoardRequest::ShowError(msg));
    }

    /// Sets the status bar message.
    pub fn status(&mut self, text: impl Into<String>, timeout_ms: Option<u32>) {
        self.out.view.set_status(text, timeout_ms);
    }

    /// Sets the view cursor (`None`: default).
    pub fn set_cursor(&mut self, shape: Option<CursorShape>) {
        self.out.view.cursor = shape;
    }

    /// Highlights nets (upstream `fsmCrossProbe()`).
    pub fn highlight_nets(&mut self, nets: impl IntoIterator<Item = NetSignalId>) {
        self.out.highlighted_nets = nets.into_iter().collect();
    }
}
