//! Undo, redo and save from the tool bars and menus of project tabs.
//!
//! Port of the `TabAction::Undo`/`Redo`/`Save` handling of
//! libs/librepcb/editor/project/schematic/schematictab.cpp and
//! board/board2dtab.cpp (which forward to the project's undo stack and
//! `ProjectEditor::saveProject()`). The undo stack is shared with the
//! embedded MCP server, so these also undo, redo and save the agent's
//! edits ("AI: <tool>" groups).

use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_i18n::tr;

use crate::app::State;
use crate::notifications::Notification;

impl State {
    /// Handles undo, redo and save of a project tab; returns whether the
    /// action was handled.
    pub(crate) fn trigger_tab_history(
        &mut self,
        section: usize,
        tab: usize,
        action: ui::TabAction,
    ) -> bool {
        if !matches!(
            action,
            ui::TabAction::Undo | ui::TabAction::Redo | ui::TabAction::Save
        ) {
            return false;
        }
        let Some(project) = self
            .sections
            .get(section)
            .and_then(|s| s.tabs().get(tab))
            .and_then(|t| t.project())
            .cloned()
        else {
            return false;
        };
        if action == ui::TabAction::Save {
            if let Some(index) = self.projects.iter().position(|p| Rc::ptr_eq(p, &project)) {
                self.trigger_project(index as i32, ui::ProjectAction::Save);
            }
            return true;
        }
        let result = {
            let mut p = project.shared().lock();
            if action == ui::TabAction::Undo {
                p.editor.undo()
            } else {
                p.editor.redo()
            }
        };
        match result {
            Ok(true) => {}
            Ok(false) => {}
            Err(e) => {
                log::error!("{action:?} failed: {e}");
                self.notifications.borrow_mut().push(Notification::new(
                    ui::NotificationType::Critical,
                    tr!("librepcb::editor::ProjectEditor", "Error"),
                    e.to_string(),
                ));
            }
        }
        self.sync_project_tabs(&project);
        true
    }
}
