//! Rebuilding the plane fragments of the boards shown in board tabs.
//!
//! Port of the planes rebuild of
//! libs/librepcb/editor/project/board/boardeditor.cpp
//! (`schedulePlanesRebuild()`, `startPlanesRebuild()`,
//! `planesRebuildTimerTimeout()`) with
//! libs/librepcb/core/project/board/boardplanefragmentsbuilder.{h,cpp}
//! running in a worker thread:
//!
//! - While a board tab is open, the planes on the copper layers visible in
//!   its tabs are rebuilt automatically when they are outdated (after
//!   modifications outside of undo groups), at most once per second
//!   (checked with the project poll timer instead of upstream's 100 ms
//!   timer).
//! - "Rebuild All Planes" (`TabAction::PlanesRebuild`) rebuilds all planes
//!   of the board at once.
//!
//! The input data is extracted under the project lock, the fragments are
//! calculated without it and applied as derived data (no undo step, like
//! upstream); a result of an outdated project revision is applied too, but
//! the layers stay outdated (see `Project::apply_plane_job_result()`). The
//! air wires are rebuilt afterwards like upstream.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use librepcb_core::project::BoardId;
use librepcb_core::project::board::PlaneFragments;
use librepcb_core::types::Layer;
use librepcb_editor::SharedProject;

use crate::app::{State, with_current_state};
use crate::project::AppProject;
use crate::tabs::Tab;

/// Minimum pause between automatic rebuilds of a board (upstream
/// `planesRebuildTimerTimeout()`).
const MIN_PAUSE: Duration = Duration::from_millis(1000);

/// The rebuild state of one board.
#[derive(Debug, Default)]
struct BoardPlanes {
    /// A rebuild is running in a worker thread.
    running: bool,
    /// When the last rebuild finished.
    finished: Option<Instant>,
    /// A forced rebuild was requested while another one was running.
    force_pending: bool,
}

/// The plane rebuilds of all open projects (by project and board).
#[derive(Debug, Default)]
pub struct PlaneRebuilds {
    boards: HashMap<(usize, BoardId), BoardPlanes>,
}

impl PlaneRebuilds {
    /// Whether a rebuild of the board is running.
    pub fn is_running(&self, project: &Rc<AppProject>, board: BoardId) -> bool {
        self.boards
            .get(&(key(project), board))
            .is_some_and(|b| b.running)
    }

    /// When the last rebuild of the board finished.
    pub fn last_finished(&self, project: &Rc<AppProject>, board: BoardId) -> Option<Instant> {
        self.boards
            .get(&(key(project), board))
            .and_then(|b| b.finished)
    }
}

impl State {
    /// The plane rebuilds.
    pub fn plane_rebuilds(&self) -> &PlaneRebuilds {
        &self.planes
    }
}

fn key(project: &Rc<AppProject>) -> usize {
    Rc::as_ptr(project) as usize
}

impl State {
    /// Starts the automatic rebuilds of the boards shown in board tabs
    /// whose planes on visible layers are outdated (called periodically).
    pub(crate) fn schedule_plane_rebuilds(&mut self) {
        // The boards of the open tabs with their visible copper layers.
        let mut boards: Vec<(Rc<AppProject>, BoardId, BTreeSet<Layer>)> = Vec::new();
        for section in &self.sections {
            for tab in section.tabs() {
                let Tab::Board2d(t) = tab else { continue };
                let hidden: BTreeSet<Layer> = t.hidden_layers().collect();
                let Some(copper) = t
                    .project()
                    .shared()
                    .try_lock()
                    .and_then(|p| p.project().board(t.board()).map(|b| b.copper_layers()))
                else {
                    continue;
                };
                let visible: BTreeSet<Layer> = copper.difference(&hidden).copied().collect();
                match boards
                    .iter_mut()
                    .find(|(p, b, _)| Rc::ptr_eq(p, t.project()) && *b == t.board())
                {
                    Some((_, _, layers)) => layers.extend(visible),
                    None => boards.push((Rc::clone(t.project()), t.board(), visible)),
                }
            }
        }
        let open: BTreeSet<(usize, BoardId)> =
            boards.iter().map(|(p, b, _)| (key(p), *b)).collect();
        self.planes
            .boards
            .retain(|k, state| state.running || open.contains(k));
        let now = Instant::now();
        for (project, board, layers) in boards {
            let state = self
                .planes
                .boards
                .entry((key(&project), board))
                .or_default();
            if state.running || state.finished.is_some_and(|f| now - f < MIN_PAUSE) {
                continue;
            }
            self.start_plane_rebuild(&project, board, Some(&layers));
        }
    }

    /// Rebuilds all planes of a board ("Rebuild All Planes", upstream
    /// `startPlanesRebuild(true)`).
    pub(crate) fn rebuild_all_planes(&mut self, project: &Rc<AppProject>, board: BoardId) {
        let state = self.planes.boards.entry((key(project), board)).or_default();
        if state.running {
            state.force_pending = true;
            return;
        }
        self.start_plane_rebuild(project, board, None);
    }

    /// Starts a rebuild of the outdated planes on `layers` (all planes if
    /// `None`) in a worker thread; returns whether one was started.
    fn start_plane_rebuild(
        &mut self,
        project: &Rc<AppProject>,
        board: BoardId,
        layers: Option<&BTreeSet<Layer>>,
    ) -> bool {
        let job = {
            let Some(p) = project.shared().try_lock() else {
                return false;
            };
            // Like upstream, no automatic rebuild while a command group is
            // active (e.g. a trace is being drawn).
            if layers.is_some() && p.editor.undo_stack().is_group_active() {
                return false;
            }
            match p.project().plane_job(board, layers) {
                Ok(Some(job)) => job,
                Ok(None) => return false,
                Err(e) => {
                    log::error!("Failed to prepare the plane rebuild: {e}");
                    return false;
                }
            }
        };
        let state = self.planes.boards.entry((key(project), board)).or_default();
        state.running = true;
        let shared = Arc::clone(project.shared());
        let spawned = std::thread::Builder::new()
            .name("planes".into())
            .spawn(move || {
                let result = job.run();
                let _ = slint::invoke_from_event_loop(move || {
                    with_current_state(|s| s.plane_rebuild_finished(&shared, board, result));
                });
            });
        if let Err(e) = spawned {
            log::error!("Failed to start the plane rebuild thread: {e}");
            state.running = false;
            return false;
        }
        true
    }

    /// Applies the fragments calculated by a worker (upstream
    /// `BoardPlaneFragmentsBuilder::finished`).
    fn plane_rebuild_finished(
        &mut self,
        shared: &SharedProject,
        board: BoardId,
        result: PlaneFragments,
    ) {
        let Some(project) = self
            .projects
            .iter()
            .find(|p| Arc::ptr_eq(p.shared(), shared))
            .cloned()
        else {
            return;
        };
        for error in &result.errors {
            log::warn!("Plane rebuild: {error}");
        }
        let applied = shared.lock().editor.update_derived_data(|p| {
            let changed = p
                .apply_plane_job_result(result)
                .map_err(|e| e.to_string())?;
            if changed {
                p.force_air_wires_rebuild(board)
                    .map_err(|e| e.to_string())?;
            }
            Ok::<_, String>(changed)
        });
        let force = {
            let state = self
                .planes
                .boards
                .entry((key(&project), board))
                .or_default();
            state.running = false;
            state.finished = Some(Instant::now());
            std::mem::take(&mut state.force_pending)
        };
        match applied {
            Ok(Ok(true)) => self.sync_project_tabs(&project),
            Ok(Ok(false)) => {}
            // An undo group became active meanwhile: the planes stay
            // outdated and are rebuilt after it.
            Ok(Err(e)) => log::error!("Failed to apply the plane fragments: {e}"),
            Err(e) => log::debug!("Plane fragments not applied: {e}"),
        }
        if force {
            self.rebuild_all_planes(&project, board);
        }
    }
}
