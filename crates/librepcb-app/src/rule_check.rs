//! The electrical and design rule checks of the open projects and the
//! rule check panel's data.
//!
//! Port of libs/librepcb/editor/rulecheck/rulecheckmessagesmodel.{h,cpp}
//! ([`RuleCheckMessages`]: sorted messages, approvals, highlight actions
//! written by `rulecheckpanel.slint`), the ERC handling of
//! libs/librepcb/editor/project/projecteditor.cpp (`scheduleErcRun()`,
//! `runErc()`), the DRC handling of
//! libs/librepcb/editor/project/board/boardeditor.cpp (`startDrc()`,
//! `setDrcResult()`, the progress notification) and the "zoom to location"
//! of `MainWindow::highlightErcMessage()`,
//! `SchematicTab::highlightErcMessage()` and
//! `Board2dTab::highlightDrcMessage()`.
//!
//! - The ERC runs automatically on the UI thread after every change of the
//!   project (like upstream, delayed by 100 ms while a schematic tab of the
//!   project is shown, else by 1 s).
//! - The DRC (full or quick check) runs on demand in a worker thread: the
//!   planes and air wires are rebuilt and the input data is extracted
//!   under the project lock (the plane fragments are calculated without
//!   it), the checks run without the lock and report progress to a
//!   progress notification.
//!
//! Differences to upstream (see `COMPAT.md`):
//! - Approving a message is a mutation through the undo stack
//!   (`SetErcApproval`/`SetDrcApproval`, undoable) instead of a direct
//!   project modification; approving a DRC message keeps the DRC results
//!   "up to date".
//! - Approvals of messages which disappeared are removed after each run
//!   like upstream (`ProjectEditor::update_erc_approvals()`,
//!   `update_drc_approvals()`: not undoable, marks the project modified),
//!   except while an undo group is active or the project is read-only.
//! - Selecting a message zooms to its location but does not draw the
//!   location marker yet; automatic fixes are not supported yet.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use librepcb_app_ui as ui;
use librepcb_canvas::kurbo::Rect;
use librepcb_core::project::board::drc::{BoardDrcData, DrcProgress, DrcResult, run_drc};
use librepcb_core::project::erc::run_erc;
use librepcb_core::project::{BoardId, BoardMutation, Mutation, SchematicId};
use librepcb_core::rule_check::{RuleCheckMessage, Severity};
use librepcb_core::serialization::SExpression;
use librepcb_core::utils::toolbox::compare_numeric;
use librepcb_editor::SharedProject;
use librepcb_i18n::tr;

use crate::app::{State, deferred, with_current_state};
use crate::models::{UiModel, model_rc};
use crate::notifications::{Notification, NotificationId};
use crate::project::AppProject;
use crate::tabs::{Tab, TabUpdate};

/// ERC delay while a schematic tab of the project is shown (upstream
/// `scheduleErcRun()`).
const ERC_DELAY_ACTIVE: Duration = Duration::from_millis(100);

/// ERC delay otherwise.
const ERC_DELAY_INACTIVE: Duration = Duration::from_millis(1000);

/// Margin around a message location for the zoom (upstream: 20 mm for the
/// ERC, 1 mm for the DRC).
const ERC_ZOOM_MARGIN_MM: f64 = 20.0;
const DRC_ZOOM_MARGIN_MM: f64 = 1.0;

/// A modification of a notification, posted from a worker thread.
type NotificationUpdate = Box<dyn FnOnce(&mut ui::NotificationData) + Send>;

/// Which check of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// The electrical rule check.
    Erc,
    /// The design rule check of a board.
    Drc(BoardId),
}

/// A message with the schematic its locations refer to (ERC).
#[derive(Debug, Clone)]
pub struct CheckMessage {
    /// The message.
    pub message: RuleCheckMessage,
    /// The schematic of the locations (ERC messages only).
    pub schematic: Option<SchematicId>,
    /// Whether the message can be fixed automatically (library checks).
    pub autofix: bool,
}

/// What the UI requested by writing a message row.
#[derive(Debug, Clone, PartialEq)]
pub enum RowAction {
    /// Nothing to do.
    None,
    /// The message was approved or its approval removed.
    Approve {
        /// The approval node of the message.
        approval: SExpression,
        /// The new state.
        approved: bool,
    },
    /// Fix the message automatically.
    Autofix {
        /// Index of the message (after sorting).
        index: usize,
    },
    /// Highlight the message (and zoom to its location).
    Highlight {
        /// Index of the message (after sorting).
        index: usize,
        /// Whether to zoom to the location.
        zoom: bool,
    },
}

/// The messages of one rule check run (upstream `RuleCheckMessagesModel`),
/// the `messages` model of `RuleCheckData`.
pub struct RuleCheckMessages {
    entries: Vec<CheckMessage>,
    approvals: BTreeSet<SExpression>,
    model: Rc<UiModel<ui::RuleCheckMessageData>>,
}

impl RuleCheckMessages {
    /// Creates an empty list; `on_write` is called (asynchronously) with
    /// rows written by the UI (approval, highlight actions).
    pub fn new(on_write: impl Fn(usize, ui::RuleCheckMessageData) + 'static) -> Self {
        let model = UiModel::shared(Vec::new());
        model.set_handler(on_write);
        Self {
            entries: Vec::new(),
            approvals: BTreeSet::new(),
            model,
        }
    }

    /// Replaces the messages and approvals (upstream `setMessages()`).
    pub fn set_messages(&mut self, entries: Vec<CheckMessage>, approvals: BTreeSet<SExpression>) {
        self.entries = entries;
        self.approvals = approvals;
        self.sort();
    }

    /// The messages (sorted like in the panel).
    pub fn entries(&self) -> &[CheckMessage] {
        &self.entries
    }

    /// Whether a message is approved.
    pub fn is_approved(&self, message: &RuleCheckMessage) -> bool {
        self.approvals.contains(message.approval())
    }

    /// Unapproved messages.
    pub fn unapproved(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| !self.is_approved(&e.message))
            .count()
    }

    /// Messages of error severity (approved or not).
    pub fn errors(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.message.severity() == Severity::Error)
            .count()
    }

    /// The model for the UI.
    pub fn model(&self) -> &Rc<UiModel<ui::RuleCheckMessageData>> {
        &self.model
    }

    /// Sorts by approval state, severity (errors first) and message
    /// (natural order), then refreshes the model (upstream
    /// `sortMessages()`).
    fn sort(&mut self) {
        let approvals = &self.approvals;
        self.entries.sort_by(|a, b| {
            let aa = approvals.contains(a.message.approval());
            let ba = approvals.contains(b.message.approval());
            aa.cmp(&ba)
                .then_with(|| b.message.severity().cmp(&a.message.severity()))
                .then_with(|| compare_numeric(a.message.message(), b.message.message()))
        });
        let rows = self
            .entries
            .iter()
            .map(|e| ui::RuleCheckMessageData {
                severity: severity_to_ui(e.message.severity()),
                message: e.message.message().into(),
                description: e.message.description().into(),
                approved: self.approvals.contains(e.message.approval()),
                autofixed: false,
                supports_autofix: e.autofix,
                action_window_id: 0,
                action: ui::RuleCheckMessageAction::None,
            })
            .collect();
        self.model.replace_all(rows);
    }

    /// Handles a row written by the UI (upstream `set_row_data()`).
    pub fn row_written(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> RowAction {
        let Some(entry) = self.entries.get(row) else {
            return RowAction::None;
        };
        let approval = entry.message.approval().clone();
        let approved = self.approvals.contains(&approval);
        if data.approved != approved {
            if data.approved {
                self.approvals.insert(approval.clone());
            } else {
                self.approvals.remove(&approval);
            }
            self.sort();
            return RowAction::Approve {
                approval,
                approved: data.approved,
            };
        }
        // Reset the action so that the same action can be requested again.
        self.model
            .update(row, |r| r.action = ui::RuleCheckMessageAction::None);
        match data.action {
            ui::RuleCheckMessageAction::Highlight => RowAction::Highlight {
                index: row,
                zoom: false,
            },
            ui::RuleCheckMessageAction::HighlightAndZoomTo => RowAction::Highlight {
                index: row,
                zoom: true,
            },
            ui::RuleCheckMessageAction::Autofix => RowAction::Autofix { index: row },
            _ => RowAction::None,
        }
    }
}

/// Upstream `l2s(RuleCheckMessage::Severity)`.
fn severity_to_ui(s: Severity) -> ui::NotificationType {
    match s {
        Severity::Hint => ui::NotificationType::Info,
        Severity::Warning => ui::NotificationType::Warning,
        Severity::Error => ui::NotificationType::Critical,
    }
}

/// The state of one check (ERC or the DRC of one board).
#[derive(Default)]
pub struct CheckState {
    messages: Option<RuleCheckMessages>,
    running: bool,
    /// Undo stack state when the check started (the DRC is outdated when
    /// it changed).
    undo_state: u64,
    /// Project revision the ERC ran on.
    revision: Option<u64>,
    execution_error: String,
    /// Progress notification of a running DRC.
    notification: Option<NotificationId>,
}

impl CheckState {
    /// The messages of the last run.
    pub fn messages(&self) -> Option<&RuleCheckMessages> {
        self.messages.as_ref()
    }

    /// Whether the check is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    fn data(
        &self,
        kind: ui::RuleCheckType,
        state: ui::RuleCheckState,
        read_only: bool,
    ) -> ui::RuleCheckData {
        let messages = self.messages.as_ref();
        ui::RuleCheckData {
            r#type: kind,
            state,
            messages: messages.map(|m| model_rc(m.model())).unwrap_or_default(),
            unapproved: messages.map_or(0, |m| m.unapproved() as i32),
            errors: messages.map_or(0, |m| m.errors() as i32),
            execution_error: self.execution_error.as_str().into(),
            read_only,
        }
    }
}

/// The rule checks of a project.
#[derive(Default)]
pub struct ProjectChecks {
    erc: CheckState,
    drc: BTreeMap<BoardId, CheckState>,
    erc_timer: slint::Timer,
}

impl ProjectChecks {
    /// The ERC.
    pub fn erc(&self) -> &CheckState {
        &self.erc
    }

    /// The DRC of a board, if it ran or runs.
    pub fn drc(&self, board: BoardId) -> Option<&CheckState> {
        self.drc.get(&board)
    }

    fn check_mut(&mut self, kind: CheckKind) -> &mut CheckState {
        match kind {
            CheckKind::Erc => &mut self.erc,
            CheckKind::Drc(board) => self.drc.entry(board).or_default(),
        }
    }

    /// `ProjectData.erc` (upstream `ProjectEditor::getUiData()`).
    pub fn erc_data(&self, read_only: bool) -> ui::RuleCheckData {
        let state = if self.erc.messages.is_some() {
            ui::RuleCheckState::UpToDate
        } else {
            ui::RuleCheckState::NotRunYet
        };
        self.erc.data(ui::RuleCheckType::Erc, state, read_only)
    }

    /// `BoardData.drc` (upstream `BoardEditor::getUiData()`).
    pub fn drc_data(&self, board: BoardId, undo_state: u64, read_only: bool) -> ui::RuleCheckData {
        let Some(check) = self.drc.get(&board) else {
            return ui::RuleCheckData {
                r#type: ui::RuleCheckType::Drc,
                state: ui::RuleCheckState::NotRunYet,
                read_only,
                ..Default::default()
            };
        };
        let state = if check.running {
            ui::RuleCheckState::Running
        } else if check.messages.is_none() {
            ui::RuleCheckState::NotRunYet
        } else if check.undo_state == undo_state {
            ui::RuleCheckState::UpToDate
        } else {
            ui::RuleCheckState::Outdated
        };
        check.data(ui::RuleCheckType::Drc, state, read_only)
    }
}

/// Bounding rectangle (mm, Y up) of message locations.
fn locations_rect(message: &RuleCheckMessage) -> Option<Rect> {
    let mut points = message
        .locations()
        .iter()
        .flat_map(|p| p.vertices())
        .map(|v| v.pos.to_mm());
    let (x, y) = points.next()?;
    let mut rect = Rect::new(x, y, x, y);
    for (x, y) in points {
        rect = rect.union_pt(librepcb_canvas::kurbo::Point::new(x, y));
    }
    Some(rect)
}

/// Runs the DRC of a board (upstream `BoardDesignRuleCheck::run()` incl.
/// the preparation of `Project::run_drc()`), locking the project only
/// while reading or updating it.
fn drc_worker(
    shared: &SharedProject,
    board: BoardId,
    quick: bool,
    progress: &(dyn Fn(DrcProgress<'_>) + Sync),
) -> Result<DrcResult, String> {
    progress(DrcProgress::Percent(1));
    if !quick {
        // Force rebuilding the planes; the fragments are calculated
        // without holding the lock.
        progress(DrcProgress::Status(&tr!(
            "librepcb::BoardDesignRuleCheck",
            "Rebuild planes..."
        )));
        let job = shared
            .lock()
            .project()
            .plane_job(board, None)
            .map_err(|e| e.to_string())?;
        if let Some(job) = job {
            let result = job.run();
            shared
                .lock()
                .editor
                .update_derived_data(|p| p.apply_plane_job_result(result))
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        }
    }
    progress(DrcProgress::Percent(10));
    let data = shared
        .lock()
        .editor
        .update_derived_data(|p| {
            if !quick {
                p.force_air_wires_rebuild(board)?;
            }
            let settings = p
                .board(board)
                .map(|b| b.settings().drc_settings.clone())
                .unwrap_or_default();
            BoardDrcData::new(p, board, &settings, quick)
        })
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    progress(DrcProgress::Percent(12));
    Ok(run_drc(&data, progress))
}

impl State {
    /// Refreshes the `Data.projects` row of a project (e.g. after a check).
    pub(crate) fn refresh_project_row(&self, project: &Rc<AppProject>) {
        if let Some(i) = self.projects.iter().position(|p| Rc::ptr_eq(p, project)) {
            self.projects_model.set(i, project.ui_data());
        }
    }

    /// Schedules the ERC of every project whose revision changed since its
    /// last ERC (upstream: after every undo stack change).
    pub(crate) fn schedule_rule_checks(&mut self) {
        for project in &self.projects {
            // Skip projects locked by a worker (exports, DRC preparation).
            let Some(revision) = project.shared().try_lock().map(|p| p.project().revision()) else {
                continue;
            };
            let checks = project.checks().borrow();
            if checks.erc.revision == Some(revision) || checks.erc_timer.running() {
                continue;
            }
            let delay = if self.schematic_tab_shown(project) {
                ERC_DELAY_ACTIVE
            } else {
                ERC_DELAY_INACTIVE
            };
            let weak_project = Rc::downgrade(project);
            let state = self.this.clone();
            checks
                .erc_timer
                .start(slint::TimerMode::SingleShot, delay, move || {
                    let project = weak_project.clone();
                    deferred(&state, move |s| {
                        if let Some(p) = project.upgrade() {
                            s.run_erc(&p);
                        }
                    });
                });
        }
    }

    fn schematic_tab_shown(&self, project: &Rc<AppProject>) -> bool {
        self.sections.iter().any(|s| {
            usize::try_from(s.current_index())
                .ok()
                .and_then(|i| s.tabs().get(i))
                .is_some_and(|t| matches!(t, Tab::Schematic(t) if Rc::ptr_eq(t.project(), project)))
        })
    }

    /// Runs the ERC of a project now (upstream `ProjectEditor::runErc()`).
    pub fn run_erc(&mut self, project: &Rc<AppProject>) {
        let Some((entries, approvals, revision, approvals_changed)) =
            project.shared().try_lock().map(|mut p| {
                let messages = run_erc(p.project());
                // Remove approvals of disappeared messages (upstream
                // `ProjectEditor::runErc()`, not undoable). An active undo
                // group keeps them until the next run; read-only projects
                // are not modified.
                let approvals_changed = !p.editor.undo_stack().is_group_active()
                    && p.file_system.is_writable()
                    && p.editor
                        .update_erc_approvals(&messages)
                        .inspect_err(|e| log::error!("Failed to update the ERC approvals: {e}"))
                        .unwrap_or(false);
                let proj = p.project();
                let entries: Vec<CheckMessage> = messages
                    .into_iter()
                    .map(|m| CheckMessage {
                        schematic: m.schematic(),
                        message: m.into(),
                        autofix: false,
                    })
                    .collect();
                (
                    entries,
                    proj.erc_approvals().clone(),
                    proj.revision(),
                    approvals_changed,
                )
            })
        else {
            // Busy: the next poll schedules it again.
            return;
        };
        {
            let mut checks = project.checks().borrow_mut();
            let handler = self.rule_check_handler(project, CheckKind::Erc);
            let erc = &mut checks.erc;
            erc.messages
                .get_or_insert_with(|| RuleCheckMessages::new(handler))
                .set_messages(entries, approvals);
            erc.revision = Some(revision);
            erc.execution_error.clear();
        }
        if approvals_changed {
            // Unsaved changes indicator of the tabs.
            self.sync_project_tabs(project);
        }
        self.refresh_project_row(project);
    }

    /// The handler of rows written by the rule check panel.
    fn rule_check_handler(
        &self,
        project: &Rc<AppProject>,
        kind: CheckKind,
    ) -> impl Fn(usize, ui::RuleCheckMessageData) + 'static {
        let project: Weak<AppProject> = Rc::downgrade(project);
        let state = self.this.clone();
        move |row, data| {
            let project = project.clone();
            deferred(&state, move |s| {
                if let Some(p) = project.upgrade() {
                    s.rule_check_row_written(&p, kind, row, &data);
                }
            });
        }
    }

    fn rule_check_row_written(
        &mut self,
        project: &Rc<AppProject>,
        kind: CheckKind,
        row: usize,
        data: &ui::RuleCheckMessageData,
    ) {
        let action = {
            let mut checks = project.checks().borrow_mut();
            match checks.check_mut(kind).messages.as_mut() {
                Some(m) => m.row_written(row, data),
                None => RowAction::None,
            }
        };
        match action {
            RowAction::None | RowAction::Autofix { .. } => {}
            RowAction::Approve { approval, approved } => {
                self.set_message_approved(project, kind, approval, approved);
            }
            RowAction::Highlight { index, zoom } => {
                self.highlight_rule_check_message(project, kind, index, zoom);
            }
        }
    }

    /// Stores an approval in the project (upstream
    /// `Project::setErcMessageApproved()`, `Board::setDrcMessageApproved()`).
    fn set_message_approved(
        &mut self,
        project: &Rc<AppProject>,
        kind: CheckKind,
        approval: SExpression,
        approved: bool,
    ) {
        let text = if approved {
            tr!("RuleCheckListItem", "Approve")
        } else {
            tr!("RuleCheckListItem", "Remove Approval")
        };
        let mutation = match kind {
            CheckKind::Erc => Mutation::SetErcApproval { approval, approved },
            CheckKind::Drc(board) => Mutation::Board(BoardMutation::SetDrcApproval {
                board,
                approval,
                approved,
            }),
        };
        let result = {
            let mut p = project.shared().lock();
            let before = p.editor.undo_stack().state_id();
            let result = p.editor.apply_mutations(text, vec![mutation]);
            // Approving does not outdate the DRC results.
            if let CheckKind::Drc(board) = kind {
                let after = p.editor.undo_stack().state_id();
                let mut checks = project.checks().borrow_mut();
                if let Some(check) = checks.drc.get_mut(&board)
                    && check.undo_state == before
                {
                    check.undo_state = after;
                }
            }
            result
        };
        if let Err(e) = result {
            log::error!("Failed to store the approval: {e}");
            self.show_status(&e.to_string(), 4000);
        }
        self.sync_project_tabs(project);
    }

    /// Shows the location of a message: opens the tab of its schematic or
    /// board and zooms to it (upstream `highlightErcMessage()`,
    /// `highlightDrcMessage()`; the location marker is not drawn yet).
    pub fn highlight_rule_check_message(
        &mut self,
        project: &Rc<AppProject>,
        kind: CheckKind,
        index: usize,
        zoom: bool,
    ) {
        let Some((rect, schematic)) = ({
            let checks = project.checks().borrow();
            let check = match kind {
                CheckKind::Erc => Some(&checks.erc),
                CheckKind::Drc(board) => checks.drc.get(&board),
            };
            check
                .and_then(|c| c.messages.as_ref())
                .and_then(|m| m.entries().get(index))
                .map(|e| (locations_rect(&e.message), e.schematic))
        }) else {
            return;
        };
        let Some(pidx) = self.projects.iter().position(|p| Rc::ptr_eq(p, project)) else {
            return;
        };
        // Open (or switch to) the tab.
        let tab_pos = match kind {
            CheckKind::Erc => {
                let Some(sch) = schematic else { return };
                let Some(i) = project.shared().lock().project().schematic_index(sch) else {
                    return;
                };
                self.open_schematic_tab(pidx as i32, i as i32, true);
                self.find_tab_where(|t| {
                    matches!(t, Tab::Schematic(t) if Rc::ptr_eq(t.project(), project) && t.schematic() == sch)
                })
            }
            CheckKind::Drc(board) => {
                let Some(i) = project.shared().lock().project().board_index(board) else {
                    return;
                };
                self.open_board_tab(pidx as i32, i as i32, true);
                self.find_tab_where(|t| {
                    matches!(t, Tab::Board2d(t) if Rc::ptr_eq(t.project(), project) && t.board() == board)
                })
            }
        };
        let (Some((si, ti)), Some(rect), true) = (tab_pos, rect, zoom) else {
            return;
        };
        let margin = match kind {
            CheckKind::Erc => ERC_ZOOM_MARGIN_MM + 1.0,
            CheckKind::Drc(_) => DRC_ZOOM_MARGIN_MM,
        };
        let rect = rect.inflate(margin, margin);
        if let Some(tab) = self.sections[si].tab_mut(ti) {
            match tab {
                Tab::Schematic(t) => t.canvas_mut().zoom_to(rect),
                Tab::Board2d(t) => t.canvas_mut().zoom_to(rect),
                _ => return,
            }
        }
        self.apply_update(si, ti, TabUpdate::repaint());
    }

    /// `Backend.trigger-board()` actions other than opening the tab.
    pub(crate) fn trigger_board_action(
        &mut self,
        project: i32,
        board: i32,
        action: ui::BoardAction,
    ) {
        let Some(prj) = usize::try_from(project)
            .ok()
            .and_then(|i| self.projects.get(i))
            .cloned()
        else {
            return;
        };
        let Some((id, _)) = usize::try_from(board).ok().and_then(|i| prj.board_at(i)) else {
            return;
        };
        match action {
            ui::BoardAction::RunDrc => self.start_drc(&prj, id, false),
            ui::BoardAction::RunQuickCheck => self.start_drc(&prj, id, true),
            ui::BoardAction::OpenSetupDialog | ui::BoardAction::OpenDrcSetupDialog => {
                if let Some(dialog) = crate::dialogs::setup::BoardSetupDialog::new(&prj, id) {
                    self.show_form_dialog(Rc::clone(&prj), Box::new(dialog));
                    if action == ui::BoardAction::OpenDrcSetupDialog {
                        self.show_form_dialog_page(2);
                    }
                }
            }
            other => self.not_implemented(&format!("{other:?}")),
        }
    }

    /// Starts the DRC of a board in a worker thread (upstream
    /// `BoardEditor::startDrc()`).
    pub fn start_drc(&mut self, project: &Rc<AppProject>, board: BoardId, quick: bool) {
        if project
            .checks()
            .borrow()
            .drc(board)
            .is_some_and(CheckState::is_running)
        {
            return;
        }
        let title = if quick {
            tr!("librepcb::editor::BoardEditor", "Running Quick Check")
        } else {
            tr!("librepcb::editor::BoardEditor", "Running Design Rule Check")
        } + "...";
        let notification = self
            .notifications
            .borrow_mut()
            .push_with_id(Notification::new(
                ui::NotificationType::Progress,
                title,
                String::new(),
            ));
        {
            let undo_state = project.shared().lock().editor.undo_stack().state_id();
            let mut checks = project.checks().borrow_mut();
            let check = checks.check_mut(CheckKind::Drc(board));
            check.running = true;
            check.undo_state = undo_state;
            check.notification = notification;
        }
        self.refresh_project_row(project);

        let shared = Arc::clone(project.shared());
        let spawned = std::thread::Builder::new()
            .name("drc".into())
            .spawn(move || {
                let last_percent = AtomicU32::new(0);
                let progress = |p: DrcProgress<'_>| {
                    let Some(id) = notification else { return };
                    let update: Option<NotificationUpdate> = match p {
                        DrcProgress::Percent(pc) => {
                            (last_percent.swap(pc, Ordering::Relaxed) != pc).then(|| {
                                Box::new(move |d: &mut ui::NotificationData| {
                                    d.progress = pc as i32;
                                }) as _
                            })
                        }
                        DrcProgress::Status(text) => {
                            let text = slint::SharedString::from(text);
                            Some(Box::new(move |d: &mut ui::NotificationData| {
                                d.description = text;
                            }))
                        }
                    };
                    if let Some(update) = update {
                        let _ = slint::invoke_from_event_loop(move || {
                            with_current_state(|s| {
                                s.notifications.borrow_mut().update(id, update);
                            });
                        });
                    }
                };
                let result = drc_worker(&shared, board, quick, &progress);
                let _ = slint::invoke_from_event_loop(move || {
                    with_current_state(|s| s.drc_finished(&shared, board, result));
                });
            });
        if let Err(e) = spawned {
            log::error!("Failed to start the DRC thread: {e}");
            self.drc_finished(project.shared(), board, Err(e.to_string()));
        }
    }

    /// Shows the DRC result (upstream `BoardEditor::setDrcResult()`).
    fn drc_finished(
        &mut self,
        shared: &SharedProject,
        board: BoardId,
        result: Result<DrcResult, String>,
    ) {
        let Some(project) = self
            .projects
            .iter()
            .find(|p| Arc::ptr_eq(p.shared(), shared))
            .cloned()
        else {
            return;
        };
        let approvals = {
            let mut p = shared.lock();
            // Remove approvals of disappeared messages (upstream
            // `BoardEditor::setDrcResult()`, not undoable).
            if let Ok(result) = &result
                && !p.editor.undo_stack().is_group_active()
                && p.file_system.is_writable()
                && let Err(e) = p.editor.update_drc_approvals(board, result)
            {
                log::error!("Failed to update the DRC approvals: {e}");
            }
            p.project()
                .board(board)
                .map(|b| b.drc_approvals().clone())
                .unwrap_or_default()
        };
        let handler = self.rule_check_handler(&project, CheckKind::Drc(board));
        let notification = {
            let mut checks = project.checks().borrow_mut();
            let check = checks.check_mut(CheckKind::Drc(board));
            check.running = false;
            match result {
                Ok(result) => {
                    let entries = result
                        .messages
                        .into_iter()
                        .map(|m| CheckMessage {
                            message: m.message().clone(),
                            schematic: None,
                            autofix: false,
                        })
                        .collect();
                    check
                        .messages
                        .get_or_insert_with(|| RuleCheckMessages::new(handler))
                        .set_messages(entries, approvals);
                    check.execution_error = result.errors.join("\n\n");
                }
                Err(e) => {
                    check
                        .messages
                        .get_or_insert_with(|| RuleCheckMessages::new(handler));
                    check.execution_error = e;
                }
            }
            check.notification.take()
        };
        if let Some(id) = notification {
            self.notifications.borrow_mut().dismiss(id);
        }
        // The planes and air wires were rebuilt.
        self.sync_project_tabs(&project);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use librepcb_core::geometry::{Path, Vertex};
    use librepcb_core::types::Point;

    fn message(severity: Severity, text: &str, key: &str) -> CheckMessage {
        CheckMessage {
            message: RuleCheckMessage::new(severity, text, "", key, Vec::new()),
            schematic: None,
            autofix: false,
        }
    }

    #[test]
    fn sorting_and_approvals() {
        let mut m = RuleCheckMessages::new(|_, _| {});
        let approved = message(Severity::Error, "E approved", "a");
        m.set_messages(
            vec![
                message(Severity::Hint, "hint", "h"),
                message(Severity::Warning, "W10", "w10"),
                approved.clone(),
                message(Severity::Warning, "W2", "w2"),
                message(Severity::Error, "E", "e"),
            ],
            [approved.message.approval().clone()].into(),
        );
        let texts: Vec<_> = m.entries().iter().map(|e| e.message.message()).collect();
        assert_eq!(texts, ["E", "W2", "W10", "hint", "E approved"]);
        assert_eq!(m.unapproved(), 4);
        assert_eq!(m.errors(), 2);
        let rows = m.model().to_vec();
        assert!(rows[4].approved);
        assert_eq!(rows[0].severity, ui::NotificationType::Critical);

        // Approving moves the message to the end.
        let mut row = rows[1].clone();
        row.approved = true;
        let action = m.row_written(1, &row);
        assert!(matches!(action, RowAction::Approve { approved: true, .. }));
        assert_eq!(m.entries()[4].message.message(), "W2");
        assert_eq!(m.unapproved(), 3);

        // Highlight requests.
        let mut row = m.model().get(0).unwrap();
        row.action = ui::RuleCheckMessageAction::HighlightAndZoomTo;
        assert_eq!(
            m.row_written(0, &row),
            RowAction::Highlight {
                index: 0,
                zoom: true
            }
        );
        assert_eq!(
            m.model().get(0).unwrap().action,
            ui::RuleCheckMessageAction::None
        );
    }

    #[test]
    fn location_bounds() {
        let path = |pts: &[(i64, i64)]| {
            Path::new(
                pts.iter()
                    .map(|(x, y)| Vertex::at(Point::from_nm(*x, *y)))
                    .collect(),
            )
        };
        let msg = RuleCheckMessage::new(
            Severity::Error,
            "x",
            "",
            "x",
            vec![
                path(&[(0, 0), (1_000_000, 2_000_000)]),
                path(&[(-3_000_000, 500_000)]),
            ],
        );
        let r = locations_rect(&msg).unwrap();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (-3.0, 0.0, 1.0, 2.0));
    }
}
