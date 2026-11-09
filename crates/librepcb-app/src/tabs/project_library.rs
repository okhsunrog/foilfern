//! The project library tab ("Library Manager" of a project).
//!
//! Port of libs/librepcb/editor/project/library/projectlibrarytab.{h,cpp}
//! and projectlibrarymodel.{h,cpp}: lists the library elements of the
//! project with the latest version in the workspace libraries (and their
//! library), marks downgrades and deprecated elements, copies elements
//! which are not in the workspace into a local library, and opens the
//! project library updater ("Update All Elements...", see
//! [`crate::dialogs::project_library_updater`]).

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::types::{Uuid, Version};
use librepcb_core::workspace::{ElementKind, LibraryDb};

use super::{TabId, TabRequest, TabUpdate};
use crate::models::{UiModel, model_rc};
use crate::project::AppProject;

/// What the workspace knows about an element (upstream
/// `ProjectLibraryModel::ElementInfo`).
#[derive(Debug, Clone)]
struct ElementInfo {
    lib_dir: FilePath,
    dir: FilePath,
    version: Version,
    deprecated: bool,
}

/// The project library tab (upstream `ProjectLibraryTab`).
pub struct ProjectLibraryTab {
    id: TabId,
    project: Rc<AppProject>,
    items: Rc<UiModel<ui::ProjectLibraryItemData>>,
    checkable: BTreeSet<String>,
    checked: BTreeSet<String>,
    downgraded: i32,
    all_checked: bool,
    /// Element path → workspace element (cache, reset after rescans).
    elements: HashMap<String, Option<ElementInfo>>,
    /// Library directory → (name, icon).
    libraries: HashMap<FilePath, (String, slint::Image)>,
    revision: Option<u64>,
}

fn kind_of(t: ui::LibraryTreeViewItemType) -> ElementKind {
    match t {
        ui::LibraryTreeViewItemType::Device => ElementKind::Device,
        ui::LibraryTreeViewItemType::Component => ElementKind::Component,
        ui::LibraryTreeViewItemType::Symbol => ElementKind::Symbol,
        _ => ElementKind::Package,
    }
}

impl ProjectLibraryTab {
    /// Creates the tab of a project.
    pub fn new(project: Rc<AppProject>) -> Self {
        Self {
            id: TabId::new(),
            project,
            items: UiModel::shared(Vec::new()),
            checkable: BTreeSet::new(),
            checked: BTreeSet::new(),
            downgraded: 0,
            all_checked: false,
            elements: HashMap::new(),
            libraries: HashMap::new(),
            revision: None,
        }
    }

    /// The tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The project.
    pub fn project(&self) -> &Rc<AppProject> {
        &self.project
    }

    /// The items model (its UI writes go to [`Self::item_written()`]).
    pub fn items_model(&self) -> &Rc<UiModel<ui::ProjectLibraryItemData>> {
        &self.items
    }

    /// Forgets the cached workspace information (after a library rescan,
    /// upstream `reset()`).
    pub fn reset(&mut self, db: &LibraryDb) {
        self.elements.clear();
        self.libraries.clear();
        self.refresh(db);
    }

    /// Rebuilds the items if the project changed (upstream `refresh()`).
    pub fn refresh_if_modified(&mut self, db: &LibraryDb) -> bool {
        let revision = self.project.revision();
        if self.revision == Some(revision) {
            return false;
        }
        self.refresh(db);
        true
    }

    /// Rebuilds the items (upstream `ProjectLibraryModel::refresh()`).
    pub fn refresh(&mut self, db: &LibraryDb) {
        self.revision = Some(self.project.revision());
        let (locale_order, entries) = {
            let p = self.project.shared().lock();
            let project = p.project();
            let lib = project.library();
            let mut entries = Vec::new();
            for (t, dir, metas) in [
                (
                    ui::LibraryTreeViewItemType::Device,
                    "dev",
                    lib.devices()
                        .values()
                        .map(|e| e.metadata().clone())
                        .collect::<Vec<_>>(),
                ),
                (
                    ui::LibraryTreeViewItemType::Component,
                    "cmp",
                    lib.components()
                        .values()
                        .map(|e| e.metadata().clone())
                        .collect(),
                ),
                (
                    ui::LibraryTreeViewItemType::Symbol,
                    "sym",
                    lib.symbols()
                        .values()
                        .map(|e| e.metadata().clone())
                        .collect(),
                ),
                (
                    ui::LibraryTreeViewItemType::Package,
                    "pkg",
                    lib.packages()
                        .values()
                        .map(|e| e.metadata().clone())
                        .collect(),
                ),
            ] {
                for m in metas {
                    entries.push((
                        t,
                        format!("library/{dir}/{}", m.uuid()),
                        m.uuid(),
                        m.name().to_string(),
                        m.version().clone(),
                        m.is_deprecated(),
                    ));
                }
            }
            (project.settings().locale_order.clone(), entries)
        };
        let mut items = Vec::new();
        self.checkable.clear();
        self.downgraded = 0;
        for (t, path, uuid, name, version, deprecated) in entries {
            let info = self
                .elements
                .entry(path.clone())
                .or_insert_with(|| element_info(db, kind_of(t), uuid))
                .clone();
            let lib = info.as_ref().map(|i| {
                self.libraries
                    .entry(i.lib_dir.clone())
                    .or_insert_with(|| library_info(db, &i.lib_dir, &locale_order))
                    .clone()
            });
            let downgrade = info.as_ref().is_some_and(|i| i.version < version);
            if downgrade {
                self.downgraded += 1;
            }
            if info.is_none() {
                self.checkable.insert(path.clone());
            }
            items.push(ui::ProjectLibraryItemData {
                r#type: t,
                path: path.as_str().into(),
                name: name.into(),
                version: version.to_pretty_str(3, 10).into(),
                latest_version: info
                    .as_ref()
                    .map(|i| i.version.to_pretty_str(3, 10))
                    .unwrap_or_default()
                    .into(),
                library_icon: lib.as_ref().map(|l| l.1.clone()).unwrap_or_default(),
                library_name: lib.map(|l| l.0).unwrap_or_default().into(),
                deprecated: deprecated || info.as_ref().is_some_and(|i| i.deprecated),
                downgrade,
                checked: self.checked.contains(&path),
                copy_in_progress: false,
                action: ui::ProjectLibraryItemAction::None,
            });
        }
        // Upstream: sorted by type, then by name (numeric).
        items.sort_by(|a, b| {
            (a.r#type as i32).cmp(&(b.r#type as i32)).then_with(|| {
                crate::dialogs::natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase())
            })
        });
        self.checked = &self.checked & &self.checkable;
        self.items.replace_all(items);
    }

    /// The base tab data (upstream `getUiData()`).
    pub fn ui_data(&self) -> ui::TabData {
        let p = self.project.shared().lock();
        let writable = self.project.is_writable();
        let stack = p.editor.undo_stack();
        ui::TabData {
            r#type: ui::TabType::ProjectLibrary,
            title: p.project().metadata().name.as_str().into(),
            features: ui::TabFeatures {
                save: feature(writable),
                undo: feature(stack.can_undo()),
                redo: feature(stack.can_redo()),
                ..Default::default()
            },
            read_only: !writable,
            unsaved_changes: p.has_unsaved_changes(),
            undo_text: stack.undo_text().unwrap_or_default().into(),
            redo_text: stack.redo_text().unwrap_or_default().into(),
            ..Default::default()
        }
    }

    /// The per-kind data (upstream `getDerivedUiData()`).
    pub fn derived_ui_data(&self, projects: &[Rc<AppProject>]) -> ui::ProjectLibraryTabData {
        ui::ProjectLibraryTabData {
            project_index: projects
                .iter()
                .position(|p| Rc::ptr_eq(p, &self.project))
                .map_or(-1, |i| i as i32),
            items: model_rc(&self.items),
            items_content_y: 0.0,
            downgraded_items: self.downgraded,
            checkable_items: self.checkable.len() as i32,
            checked_items: self.checked.len() as i32,
            all_checked: self.all_checked,
            copy_to_library: Default::default(),
        }
    }

    /// Applies data written by the UI (upstream `setDerivedUiData()`):
    /// "all checked" and "copy to library".
    pub fn set_derived_ui_data(
        &mut self,
        data: &ui::ProjectLibraryTabData,
        local_libraries: &FilePath,
    ) -> TabUpdate {
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        if data.all_checked != self.all_checked {
            self.all_checked = data.all_checked;
            self.checked = if self.all_checked {
                self.checkable.clone()
            } else {
                BTreeSet::new()
            };
            let checked = self.checked.clone();
            for i in 0..self.items.len() {
                self.items.update(i, |item| {
                    item.checked = checked.contains(item.path.as_str());
                });
            }
        }
        if let Some(lib) = FilePath::new(data.copy_to_library.as_str())
            && lib.is_located_in_dir(local_libraries)
        {
            let (count, errors) = self.copy_to_library(&lib);
            if count > 0 {
                update.requests.push(TabRequest::RescanLibraries);
            }
            if !errors.is_empty() {
                update
                    .requests
                    .push(TabRequest::Notify(crate::notifications::Notification::new(
                        ui::NotificationType::Critical,
                        librepcb_i18n::tr!("librepcb::editor::ProjectLibraryTab", "Error"),
                        format!(
                            "{}\n\n- {}",
                            librepcb_i18n::tr!(
                                "librepcb::editor::ProjectLibraryTab",
                                "Failed to copy library elements:"
                            ),
                            errors.join("\n- ")
                        ),
                    )));
            }
        }
        update
    }

    /// An item was written by the UI (checked or "open").
    pub fn item_written(&mut self, row: usize, data: &ui::ProjectLibraryItemData) -> TabUpdate {
        let path = data.path.to_string();
        if data.action == ui::ProjectLibraryItemAction::Open {
            self.items
                .update(row, |i| i.action = ui::ProjectLibraryItemAction::None);
            if let Some(Some(info)) = self.elements.get(&path) {
                return TabUpdate {
                    requests: vec![TabRequest::OpenLibraryElement {
                        library: info.lib_dir.clone(),
                        kind: data.r#type,
                        path: info.dir.clone(),
                        duplicate: false,
                    }],
                    ..TabUpdate::default()
                };
            }
        } else if self.checkable.contains(&path) {
            if data.checked {
                self.checked.insert(path);
            } else {
                self.checked.remove(&path);
            }
            return TabUpdate {
                data_changed: true,
                ..TabUpdate::default()
            };
        }
        TabUpdate::default()
    }

    /// Handles a tab action: "apply" opens the project library updater.
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        match action {
            ui::TabAction::Apply => TabUpdate {
                requests: vec![TabRequest::ProjectLibraryUpdater(
                    self.project.path().clone(),
                )],
                ..TabUpdate::default()
            },
            _ => TabUpdate::default(),
        }
    }

    /// Copies the checked elements (not in the workspace) into a local
    /// library (upstream `ProjectLibraryModel::copyToLibrary()`); returns
    /// the number of copied elements and the errors.
    fn copy_to_library(&mut self, lib: &FilePath) -> (usize, Vec<String>) {
        let mut count = 0;
        let mut errors = Vec::new();
        let fs = self.project.shared().lock().file_system.clone();
        for path in self.checked.clone() {
            let parts: Vec<&str> = path.rsplitn(3, '/').collect();
            if parts.len() < 2 || !matches!(parts[1], "dev" | "cmp" | "sym" | "pkg") {
                errors.push(format!("Invalid element: {path}"));
                continue;
            }
            let dst = lib.path_to(&format!("{}/{}", parts[1], parts[0]));
            let result = (|| -> Result<(), String> {
                if dst.is_existing_dir() {
                    return Err(format!("Directory exists already: {}", dst.to_native()));
                }
                count += 1;
                let src = TransactionalDirectory::new(fs.clone(), &path);
                let dst_fs = std::sync::Arc::new(
                    TransactionalFileSystem::open_rw(&dst).map_err(|e| e.to_string())?,
                );
                let mut dst_dir = TransactionalDirectory::new(dst_fs.clone(), "");
                let copied = src
                    .copy_to(&mut dst_dir)
                    .and_then(|()| dst_fs.save())
                    .map_err(|e| e.to_string());
                drop(dst_dir);
                drop(dst_fs);
                if copied.is_err() {
                    let _ = std::fs::remove_dir_all(dst.as_path());
                }
                copied
            })();
            if let Err(e) = result {
                errors.push(e);
            }
        }
        (count, errors)
    }
}

fn feature(enabled: bool) -> ui::FeatureState {
    if enabled {
        ui::FeatureState::Enabled
    } else {
        ui::FeatureState::Disabled
    }
}

/// Upstream `ProjectLibraryModel::getElementInfo()`.
fn element_info(db: &LibraryDb, kind: ElementKind, uuid: Uuid) -> Option<ElementInfo> {
    let dir = match db.latest(kind, uuid) {
        Ok(Some(dir)) => dir,
        Ok(None) => return None,
        Err(e) => {
            log::error!("Failed to get library element info: {e}");
            return None;
        }
    };
    let lib_dir = dir.parent_dir()?.parent_dir()?;
    match db.metadata(kind, &dir) {
        Ok(Some(m)) => Some(ElementInfo {
            lib_dir,
            dir,
            version: m.version,
            deprecated: m.deprecated,
        }),
        Ok(None) => None,
        Err(e) => {
            log::error!("Failed to get library element info: {e}");
            None
        }
    }
}

/// Upstream `ProjectLibraryModel::getLibraryInfo()`: name and icon.
fn library_info(db: &LibraryDb, dir: &FilePath, locale_order: &[String]) -> (String, slint::Image) {
    let icon = db
        .library_metadata(dir)
        .ok()
        .flatten()
        .and_then(|m| crate::icons::decode_png(&m.icon_png))
        .unwrap_or_default();
    let name = db
        .translations(ElementKind::Library, dir, locale_order)
        .ok()
        .flatten()
        .map(|t| t.name)
        .unwrap_or_default();
    (name, icon)
}
