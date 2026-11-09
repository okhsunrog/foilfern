//! Editing in the schematic and board tabs, on the application side: key
//! events of the scenes, the requests of the tabs (notifications, context
//! menus, dialogs), cross-probing between the tabs of a project and aborting
//! tools which keep an undo group open.
//!
//! Port of the corresponding parts of
//! libs/librepcb/editor/project/projecteditor.{h,cpp} (`abortBlockingToolsInOtherEditors()`,
//! the cross probe of `ProjectEditor::getCrossProbe()`) and
//! libs/librepcb/editor/mainwindow.cpp (scene key events).

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use librepcb_app_ui as ui;
use slint::ComponentHandle;

use super::{State, deferred};
use crate::models::vec_model;
use crate::project::AppProject;
use crate::tabs::{TabId, TabRequest, TabUpdate};
use librepcb_i18n::tr;

/// State of the editing support of the application.
#[derive(Debug, Default)]
pub struct EditingState {
    /// The tab which opened the undo group of a project (by project
    /// pointer), to abort its tool when another tab is used.
    group_owners: HashMap<usize, TabId>,
    /// The tab of the open context menu.
    menu_tab: Option<TabId>,
    /// The tab for which the "add component" dialog is opened again when
    /// it finished adding the chosen component ("Add more").
    pub(super) reopen_add_component: Option<TabId>,
}

fn project_key(project: &Rc<AppProject>) -> usize {
    Rc::as_ptr(project) as usize
}

/// Whether a tab action only changes the view (does not need to abort tools
/// of other tabs).
pub fn is_view_action(action: ui::TabAction) -> bool {
    use ui::TabAction as A;
    matches!(
        action,
        A::ZoomIn
            | A::ZoomOut
            | A::ZoomFit
            | A::GridIntervalIncrease
            | A::GridIntervalDecrease
            | A::LayersNone
            | A::LayersTop
            | A::LayersBottom
            | A::LayersTopBottom
            | A::LayersAll
            | A::Close
    )
}

/// Binds the scene key callbacks of `Backend` and the `SceneEditor` global.
pub fn bind(window: &ui::AppWindow, weak: &Weak<std::cell::RefCell<State>>) {
    let b = window.global::<ui::Backend>();
    // Key events are processed synchronously: the UI needs to know whether
    // they were handled (like upstream).
    let w = weak.clone();
    b.on_scene_key_pressed(move |section, event| {
        let Some(s) = w.upgrade() else { return false };
        let Ok(mut s) = s.try_borrow_mut() else {
            return false;
        };
        s.scene_key_event(section, &event, true)
    });
    let w = weak.clone();
    b.on_scene_key_released(move |section, event| {
        let Some(s) = w.upgrade() else { return false };
        let Ok(mut s) = s.try_borrow_mut() else {
            return false;
        };
        s.scene_key_event(section, &event, false)
    });

    let e = window.global::<ui::SceneEditor>();
    let w = weak.clone();
    e.on_menu_activated(move |_section, index| {
        deferred(&w, move |s| s.context_menu_activated(index));
    });
}

impl State {
    /// `Backend.scene-key-pressed` / `scene-key-released`.
    fn scene_key_event(
        &mut self,
        section: i32,
        event: &slint::language::KeyEvent,
        pressed: bool,
    ) -> bool {
        let Ok(section) = usize::try_from(section) else {
            return false;
        };
        let Some(tab) = self.sections.get(section).map(|s| s.current_index()) else {
            return false;
        };
        if pressed {
            self.abort_blocking_tools_except(section, tab);
        }
        let Some(t) = self
            .sections
            .get_mut(section)
            .and_then(|s| s.current_tab_mut())
        else {
            return false;
        };
        let (handled, update) = if pressed {
            t.key_pressed(event)
        } else {
            t.key_released(event)
        };
        if let Ok(tab) = usize::try_from(tab) {
            self.apply_update(section, tab, update);
            self.after_tab_event(section, tab);
        }
        handled
    }

    /// Handles the requests of a tab and the modification of its project.
    pub(super) fn apply_tab_requests(
        &mut self,
        section: usize,
        tab: usize,
        requests: Vec<TabRequest>,
        project_modified: bool,
    ) {
        let Some(t) = self.sections.get(section).and_then(|s| s.tabs().get(tab)) else {
            return;
        };
        let id = t.id();
        let project = t.project().cloned();
        let t_unit = t.length_unit();
        for request in requests {
            match request {
                TabRequest::Notify(n) => self.notifications.borrow_mut().push(n),
                TabRequest::ContextMenu { pos, entries } => {
                    self.editing.menu_tab = Some(id);
                    if let Some(w) = self.window() {
                        let e = w.global::<ui::SceneEditor>();
                        e.set_menu_section(section as i32);
                        e.set_menu_x(pos.x as f32);
                        e.set_menu_y(pos.y as f32);
                        e.set_menu_entries(vec_model(
                            entries
                                .iter()
                                .map(|entry| ui::SceneMenuEntry {
                                    text: entry.text.as_str().into(),
                                    enabled: entry.enabled,
                                    checkable: entry.checked.is_some(),
                                    checked: entry.checked.unwrap_or(false),
                                    is_default: entry.is_default,
                                })
                                .collect(),
                        ));
                        e.set_menu_serial(e.get_menu_serial().wrapping_add(1));
                    }
                }
                TabRequest::AddComponent { search_term } => {
                    self.open_add_component_dialog(id, &search_term);
                }
                TabRequest::Properties(target) => {
                    let Some(project) = project.clone() else {
                        continue;
                    };
                    let unit = t_unit.unwrap_or(librepcb_core::types::LengthUnit::Millimeters);
                    if let Some(dialog) = crate::dialogs::open_properties(&project, &target, unit) {
                        self.open_form_dialog(project, Some(id), dialog);
                    }
                }
                TabRequest::ChooseImageFile => {
                    self.choose_image_file(id);
                }
                TabRequest::ImportDxf { layers, kind } => {
                    let title = tr!("librepcb::editor::DxfImportDialog", "Choose file");
                    let filters = [crate::file_dialog::Filter {
                        name: "*.dxf".into(),
                        extensions: vec!["dxf", "DXF"],
                    }];
                    let Some(path) = crate::file_dialog::open_file(&title, &filters, None) else {
                        continue;
                    };
                    let Some(fp) = crate::app::absolute_file_path(&path) else {
                        continue;
                    };
                    let unit = t_unit.unwrap_or(librepcb_core::types::LengthUnit::Millimeters);
                    let dialog =
                        crate::dialogs::board::DxfImportDialog::new(fp, &layers, unit, kind);
                    match project.clone() {
                        Some(project) => self.open_form_dialog(project, Some(id), Box::new(dialog)),
                        None => self.open_library_dialog(Some(id), Box::new(dialog)),
                    }
                }
                TabRequest::OpenLibrary { .. }
                | TabRequest::DownloadLibrary { .. }
                | TabRequest::LibraryDownloaded(_)
                | TabRequest::LibraryModified
                | TabRequest::RescanLibraries
                | TabRequest::OpenLibraryElement { .. }
                | TabRequest::RemoveLibraryElements(_)
                | TabRequest::ChooseLibraryIcon
                | TabRequest::LibraryItemProperties(_)
                | TabRequest::ImportPinsDialog
                | TabRequest::ChooseStepFile { .. }
                | TabRequest::CourtyardOffsetDialog
                | TabRequest::MoveAlign { .. }
                | TabRequest::ChooseElement(_)
                | TabRequest::OpenUrl(_)
                | TabRequest::ChoosePinoutFile
                | TabRequest::DesignRulesName { .. }
                | TabRequest::ConfirmClose { .. }
                | TabRequest::DuplicateLibraryElement => self.apply_library_request(id, request),
                TabRequest::ProjectLibraryUpdater(fp) => {
                    self.show_project_library_updater(fp, &[]);
                }
                TabRequest::RebuildPlanes(board) => {
                    if let Some(project) = project.clone() {
                        self.rebuild_all_planes(&project, board);
                    }
                }
                TabRequest::LineWidth { current } => {
                    let Some(project) = project.clone() else {
                        continue;
                    };
                    let unit = t_unit.unwrap_or(librepcb_core::types::LengthUnit::Millimeters);
                    let dialog = crate::dialogs::board::LineWidthDialog::new(current, unit);
                    self.open_form_dialog(project, Some(id), Box::new(dialog));
                }
            }
        }
        if project_modified && let Some(project) = project {
            self.project_changed_by_tab(&project);
        }
    }

    /// Updates the other tabs and the project data after a tab modified
    /// its project (instead of waiting for the polling timer).
    pub(super) fn project_changed_by_tab(&mut self, project: &Rc<AppProject>) {
        if let Some(i) = self.projects.iter().position(|p| Rc::ptr_eq(p, project)) {
            self.projects_model.set(i, project.ui_data());
        }
        self.poll_projects();
    }

    /// Called after an event was dispatched to a tab: tracks the owner of
    /// an open undo group and cross-probes the tab's selection to the other
    /// tabs of the project.
    pub(super) fn after_tab_event(&mut self, section: usize, tab: usize) {
        self.maybe_reopen_add_component(section, tab);
        let Some(t) = self.sections.get(section).and_then(|s| s.tabs().get(tab)) else {
            return;
        };
        let Some(project) = t.project().cloned() else {
            return;
        };
        let id = t.id();
        let probe = t.cross_probe();
        let key = project_key(&project);
        let active = project
            .shared()
            .lock()
            .editor
            .undo_stack()
            .is_group_active();
        if active {
            self.editing.group_owners.entry(key).or_insert(id);
        } else {
            self.editing.group_owners.remove(&key);
        }
        let Some(probe) = probe else { return };
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                let t = &mut self.sections[si].tabs_mut()[ti];
                if t.id() == id || !t.project().is_some_and(|p| Rc::ptr_eq(p, &project)) {
                    continue;
                }
                let update = t.set_cross_probe(&probe);
                if update != TabUpdate::default() {
                    self.apply_update(si, ti, update);
                }
            }
        }
    }

    /// Aborts the tool of the tab which keeps an undo group of the project
    /// of tab `tab` open, unless it is that tab (upstream
    /// `abortBlockingToolsInOtherEditors()`).
    pub(super) fn abort_blocking_tools_except(&mut self, section: usize, tab: i32) {
        let Some(t) = usize::try_from(tab)
            .ok()
            .and_then(|i| self.sections.get(section).and_then(|s| s.tabs().get(i)))
        else {
            return;
        };
        let Some(project) = t.project().cloned() else {
            return;
        };
        let id = t.id();
        let key = project_key(&project);
        let Some(owner) = self.editing.group_owners.get(&key).copied() else {
            return;
        };
        if owner == id {
            return;
        }
        self.editing.group_owners.remove(&key);
        if let Some((si, ti)) = self.find_tab(owner) {
            let update = self.sections[si].tabs_mut()[ti].abort_blocking_tool();
            self.apply_update(si, ti, update);
        }
    }

    /// Aborts the tool which keeps an undo group of the project of tab
    /// `tab` open, including the tab's own tool (before undo, redo and
    /// save).
    pub(super) fn abort_all_blocking_tools(&mut self, section: usize, tab: usize) {
        self.abort_blocking_tools_except(section, tab as i32);
        if let Some(t) = self.sections.get_mut(section).and_then(|s| s.tab_mut(tab)) {
            let update = t.abort_blocking_tool();
            self.apply_update(section, tab, update);
        }
    }

    /// Aborts the tools of all tabs of a project which keep an undo group
    /// open (before saving it).
    pub(super) fn abort_project_tools(&mut self, project: &Rc<AppProject>) {
        self.editing.group_owners.remove(&project_key(project));
        for si in 0..self.sections.len() {
            for ti in 0..self.sections[si].tabs().len() {
                let t = &mut self.sections[si].tabs_mut()[ti];
                if t.project().is_some_and(|p| Rc::ptr_eq(p, project)) {
                    let update = t.abort_blocking_tool();
                    self.apply_update(si, ti, update);
                }
            }
        }
    }

    /// Asks for an image file for the image tool of a schematic or symbol
    /// tab (upstream `ImageHelpers::execImageChooserDialog()`) and passes it to
    /// the tab.
    fn choose_image_file(&mut self, tab: TabId) {
        let title = tr!("ImageHelpers", "Choose Image File");
        let filters = [crate::file_dialog::Filter {
            name: format!(
                "{} (*.png *.jpg *.jpeg *.svg)",
                tr!("ImageHelpers", "Image Files")
            ),
            extensions: vec!["png", "jpg", "jpeg", "svg", "PNG", "JPG", "JPEG", "SVG"],
        }];
        let Some(path) = crate::file_dialog::open_file(&title, &filters, None) else {
            return;
        };
        let data = match load_image_file(&path) {
            Ok(data) => data,
            Err(message) => {
                self.notifications
                    .borrow_mut()
                    .push(crate::notifications::Notification {
                        auto_popup: true,
                        ..crate::notifications::Notification::new(
                            ui::NotificationType::Critical,
                            tr!("librepcb::editor::SchematicTab", "Error"),
                            message,
                        )
                    });
                return;
            }
        };
        if let Some((si, ti)) = self.find_tab(tab) {
            let update = self.sections[si].tabs_mut()[ti].add_image(data);
            self.apply_update(si, ti, update);
            self.after_tab_event(si, ti);
        }
    }

    /// An entry of the scene context menu was chosen.
    fn context_menu_activated(&mut self, index: i32) {
        let (Some(id), Ok(index)) = (self.editing.menu_tab.take(), usize::try_from(index)) else {
            return;
        };
        if let Some((si, ti)) = self.find_tab(id) {
            let update = self.sections[si].tabs_mut()[ti].context_menu_action(index);
            self.apply_update(si, ti, update);
            self.after_tab_event(si, ti);
        }
    }
}

/// Reads an image file for the image tool: PNG, JPEG and SVG (the formats
/// schematics support; upstream converts other formats with Qt's image
/// readers, see COMPAT.md).
pub fn load_image_file(
    path: &std::path::Path,
) -> Result<librepcb_editor::fsm::schematic::ImageData, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let format = match ext.as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "svg" => "svg",
        _ => {
            // Not translated upstream (`ImageHelpers::execImageChooserDialog()`).
            return Err(format!(
                "Failed to convert image '{}' to a supported format. Please try a different image format.",
                path.display()
            ));
        }
    };
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let basename = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    Ok(librepcb_editor::fsm::schematic::ImageData {
        data,
        format: format.to_owned(),
        basename,
    })
}
