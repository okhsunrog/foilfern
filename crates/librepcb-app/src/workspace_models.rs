//! Models of the home tab and the home panel: recent/favorite projects and
//! the workspace folder tree.
//!
//! Port of libs/librepcb/editor/workspace/quickaccessmodel.{h,cpp} and
//! libs/librepcb/editor/workspace/filesystemmodel.{h,cpp}. The recent and
//! favorite projects are stored in the workspace data directory
//! (`recent_projects.lp`, `favorite_projects.lp`) with upstream's format,
//! so both applications share them.
//!
//! Not ported yet: watching directories for changes (the tree is refreshed
//! when a directory is expanded), creating folders and removing files from
//! the tree (they need dialogs), and persisting the expanded directories.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::{Rc, Weak};

use librepcb_app_ui as ui;
use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::project::Project;
use librepcb_core::serialization::{Mode, SExpression};
use slint::SharedString;

use crate::icons::Icon;
use crate::models::UiModel;

/// Callback to open a file (project) from a model.
pub type OpenFileFn = Rc<dyn Fn(FilePath)>;

/// Callback for favorite changes.
type FavoriteFn = Rc<dyn Fn(&FilePath, bool)>;

/// Maximum number of recent (non-favorite) projects shown.
const MAX_RECENT: usize = 5;

/// Recent and favorite projects (`Data.quick-access-items`).
pub struct QuickAccess {
    workspace_root: FilePath,
    recent_file: FilePath,
    favorites_file: FilePath,
    recent: Vec<FilePath>,
    favorites: Vec<FilePath>,
    model: Rc<UiModel<ui::TreeViewItemData>>,
    /// Called with a project and its favorite state when that changed.
    on_favorite_changed: Option<FavoriteFn>,
}

impl QuickAccess {
    /// Loads the lists of a workspace.
    pub fn new(workspace_root: &FilePath, data_dir: &FilePath) -> Rc<RefCell<Self>> {
        let mut qa = Self {
            workspace_root: workspace_root.clone(),
            recent_file: data_dir.path_to("recent_projects.lp"),
            favorites_file: data_dir.path_to("favorite_projects.lp"),
            recent: Vec::new(),
            favorites: Vec::new(),
            model: UiModel::shared(Vec::new()),
            on_favorite_changed: None,
        };
        qa.recent = qa.load(&qa.recent_file.clone());
        qa.favorites = qa.load(&qa.favorites_file.clone());
        qa.refresh_items();
        Rc::new(RefCell::new(qa))
    }

    /// Connects the model's UI actions (open, unpin, remove).
    pub fn connect(this: &Rc<RefCell<Self>>, open: OpenFileFn) {
        let weak = Rc::downgrade(this);
        this.borrow()
            .model
            .set_handler(move |_row, data: ui::TreeViewItemData| {
                let Some(qa) = weak.upgrade() else { return };
                let Some(fp) = FilePath::new(data.user_data.as_str()) else {
                    return;
                };
                qa.borrow_mut().set_favorite(&fp, data.pinned);
                match data.action {
                    ui::TreeViewItemAction::Open => open(fp),
                    ui::TreeViewItemAction::Delete => {
                        let mut qa = qa.borrow_mut();
                        qa.set_favorite(&fp, false);
                        qa.discard_recent(&fp);
                    }
                    _ => {}
                }
            });
    }

    /// Sets the callback for favorite changes (to update the folder tree).
    pub fn set_on_favorite_changed(&mut self, f: impl Fn(&FilePath, bool) + 'static) {
        self.on_favorite_changed = Some(Rc::new(f));
    }

    /// The model for `Data.quick-access-items`.
    pub fn model(&self) -> &Rc<UiModel<ui::TreeViewItemData>> {
        &self.model
    }

    /// Moves a project to the top of the recent projects.
    pub fn push_recent(&mut self, fp: &FilePath) {
        if self.recent.first() == Some(fp) {
            return;
        }
        self.recent.retain(|p| p != fp);
        self.recent.insert(0, fp.clone());
        self.refresh_items();
        self.save(
            &self.recent_file.clone(),
            "librepcb_recent_projects",
            &self.recent.clone(),
        );
    }

    /// Removes a project from the recent projects.
    pub fn discard_recent(&mut self, fp: &FilePath) {
        let len = self.recent.len();
        self.recent.retain(|p| p != fp);
        if self.recent.len() != len {
            self.refresh_items();
            self.save(
                &self.recent_file.clone(),
                "librepcb_recent_projects",
                &self.recent.clone(),
            );
        }
    }

    /// Whether a project is a favorite.
    pub fn is_favorite(&self, fp: &FilePath) -> bool {
        self.favorites.contains(fp)
    }

    /// Adds or removes a favorite project.
    pub fn set_favorite(&mut self, fp: &FilePath, favorite: bool) {
        let changed = if favorite && !self.favorites.contains(fp) {
            self.favorites.push(fp.clone());
            true
        } else if !favorite && self.favorites.contains(fp) {
            self.favorites.retain(|p| p != fp);
            true
        } else {
            false
        };
        if changed {
            self.refresh_items();
            self.save(
                &self.favorites_file.clone(),
                "librepcb_favorite_projects",
                &self.favorites.clone(),
            );
            if let Some(f) = self.on_favorite_changed.clone() {
                let fp = fp.clone();
                crate::models::defer(move || f(&fp, favorite));
            }
        }
    }

    fn load(&self, file: &FilePath) -> Vec<FilePath> {
        if !file.is_existing_file() {
            return Vec::new();
        }
        let parsed = file_utils::read_file(file)
            .ok()
            .and_then(|data| SExpression::parse(&data, Some(file.as_path()), Mode::LibrePcb).ok());
        let Some(root) = parsed else {
            log::warn!("Failed to read {}.", file.to_native());
            return Vec::new();
        };
        root.children_named("project")
            .filter_map(|c| c.child("@0").and_then(|v| v.value().ok()))
            .map(|rel| FilePath::from_relative(&self.workspace_root, rel))
            .collect()
    }

    fn save(&self, file: &FilePath, root_name: &str, paths: &[FilePath]) {
        let mut root = SExpression::list(root_name);
        if let Some(list) = root.as_list_mut() {
            for fp in paths {
                list.ensure_line_break();
                list.append_child("project", &fp.to_relative(&self.workspace_root));
            }
            list.ensure_line_break();
        }
        let result = root
            .to_byte_array(Mode::LibrePcb)
            .map_err(|e| e.to_string())
            .and_then(|bytes| file_utils::write_file(file, &bytes).map_err(|e| e.to_string()));
        if let Err(e) = result {
            log::warn!("Failed to save {}: {e}", file.to_native());
        }
    }

    fn refresh_items(&mut self) {
        let mut listed: Vec<FilePath> = Vec::new();
        let mut items = Vec::new();
        for fp in self.recent.iter().chain(&self.favorites) {
            let favorite = self.favorites.contains(fp);
            if (listed.len() < MAX_RECENT || favorite)
                && !listed.contains(fp)
                && fp.is_existing_file()
            {
                items.push(ui::TreeViewItemData {
                    level: 0,
                    icon: Icon::App.image(),
                    text: fp.file_name().into(),
                    hint: fp.to_native().into(),
                    user_data: fp.as_str().into(),
                    is_project_file_or_folder: true,
                    has_children: false,
                    expanded: false,
                    supports_pinning: true,
                    pinned: favorite,
                    action: ui::TreeViewItemAction::None,
                });
                listed.push(fp.clone());
            }
        }
        self.model.replace_all(items);
    }
}

/// The workspace folder tree (`Data.workspace-folder-tree`): a flat list of
/// items with levels, directories expanded on demand.
pub struct FileSystemTree {
    root: FilePath,
    expanded: BTreeSet<FilePath>,
    model: Rc<UiModel<ui::TreeViewItemData>>,
    quick_access: Weak<RefCell<QuickAccess>>,
}

impl FileSystemTree {
    /// Creates the tree of `root` (the workspace's projects directory).
    pub fn new(root: &FilePath, quick_access: &Rc<RefCell<QuickAccess>>) -> Rc<RefCell<Self>> {
        let mut tree = Self {
            root: root.clone(),
            expanded: BTreeSet::new(),
            model: UiModel::shared(Vec::new()),
            quick_access: Rc::downgrade(quick_access),
        };
        tree.expand_dir(&root.clone(), 0, 0);
        Rc::new(RefCell::new(tree))
    }

    /// Connects the model's UI actions (expand/collapse, pin, open, new
    /// project in a folder).
    pub fn connect(this: &Rc<RefCell<Self>>, open: OpenFileFn, new_project: OpenFileFn) {
        let weak = Rc::downgrade(this);
        this.borrow()
            .model
            .set_handler(move |row, data: ui::TreeViewItemData| {
                if let Some(tree) = weak.upgrade() {
                    let action = tree.borrow_mut().row_written(row, data);
                    match action {
                        Some((fp, false)) => open(fp),
                        Some((fp, true)) => new_project(fp),
                        None => {}
                    }
                }
            });
    }

    /// The model for `Data.workspace-folder-tree`.
    pub fn model(&self) -> &Rc<UiModel<ui::TreeViewItemData>> {
        &self.model
    }

    /// Updates the pinned state of a project file item.
    pub fn favorite_changed(&self, fp: &FilePath, favorite: bool) {
        for (i, item) in self.model.to_vec().into_iter().enumerate() {
            if item.supports_pinning && item.user_data.as_str() == fp.as_str() {
                self.model.update(i, |item| item.pinned = favorite);
            }
        }
    }

    /// Handles a row written by the UI; returns a file to open (`false`)
    /// or a folder to create a new project in (`true`).
    fn row_written(&mut self, row: usize, data: ui::TreeViewItemData) -> Option<(FilePath, bool)> {
        let fp = FilePath::new(data.user_data.as_str())?;
        // The row already contains the UI's data; compare with the state
        // before by looking at the following rows.
        let has_children_listed = self
            .model
            .get(row + 1)
            .is_some_and(|next| next.level > data.level);
        if data.expanded && !has_children_listed {
            self.expand_dir(&fp, row + 1, data.level + 1);
        } else if !data.expanded && has_children_listed {
            self.collapse_dir(&fp, row + 1, data.level + 1);
        }
        if data.supports_pinning
            && let Some(qa) = self.quick_access.upgrade()
            && qa.borrow().is_favorite(&fp) != data.pinned
        {
            qa.borrow_mut().set_favorite(&fp, data.pinned);
        }
        self.model
            .update(row, |item| item.action = ui::TreeViewItemAction::None);
        match data.action {
            ui::TreeViewItemAction::Open => Some((fp, false)),
            ui::TreeViewItemAction::NewProject => Some((fp, true)),
            ui::TreeViewItemAction::None => None,
            other => {
                log::warn!("Unhandled action in workspace folder tree: {other:?}");
                None
            }
        }
    }

    fn expand_dir(&mut self, dir: &FilePath, index: usize, level: i32) {
        let mut entries: Vec<(bool, String, FilePath)> = match std::fs::read_dir(dir.as_path()) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let fp = dir.path_to(&name);
                    (!fp.is_existing_dir(), name, fp)
                })
                .collect(),
            Err(e) => {
                log::warn!("Failed to list {}: {e}", dir.to_native());
                Vec::new()
            }
        };
        // Directories first, then by name (like `QDir::Name | QDir::DirsFirst`).
        entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let quick_access = self.quick_access.upgrade();
        let mut items = Vec::new();
        let mut to_expand = Vec::new();
        for (i, (is_file, name, fp)) in entries.into_iter().enumerate() {
            let is_dir = !is_file;
            let expand = is_dir && self.expanded.contains(&fp);
            if expand {
                to_expand.push(index + i);
            }
            let is_project_file = matches!(fp.suffix(), "lpp" | "lppz");
            let is_pinnable = is_project_file && quick_access.is_some();
            let is_project_folder = Project::is_project_directory(&fp);
            let icon = if is_project_file {
                Icon::App
            } else if is_project_folder {
                Icon::ProjectFolder
            } else if is_dir {
                Icon::Folder
            } else {
                Icon::File
            };
            items.push(ui::TreeViewItemData {
                level,
                icon: icon.image(),
                text: SharedString::from(name.as_str()),
                hint: SharedString::new(),
                user_data: fp.as_str().into(),
                is_project_file_or_folder: is_project_folder
                    || Project::is_file_path_inside_project_directory(&fp),
                has_children: is_dir,
                expanded: expand,
                supports_pinning: is_pinnable,
                pinned: is_pinnable
                    && quick_access
                        .as_ref()
                        .is_some_and(|qa| qa.borrow().is_favorite(&fp)),
                action: ui::TreeViewItemAction::None,
            });
        }
        self.model.insert_many(index, items);
        if *dir != self.root {
            self.expanded.insert(dir.clone());
        }
        // Expand children from bottom to top to keep indices valid.
        for i in to_expand.into_iter().rev() {
            if let Some(item) = self.model.get(i)
                && let Some(fp) = FilePath::new(item.user_data.as_str())
            {
                self.expand_dir(&fp, i + 1, item.level + 1);
            }
        }
    }

    fn collapse_dir(&mut self, dir: &FilePath, index: usize, level: i32) {
        let count = self.model.to_vec()[index.min(self.model.len())..]
            .iter()
            .take_while(|item| item.level >= level)
            .count();
        self.model.remove_range(index, count);
        self.expanded.remove(dir);
    }
}
