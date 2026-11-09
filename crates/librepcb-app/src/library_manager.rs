//! The libraries panel: installed (local) libraries and the libraries of the
//! API servers (install, update, uninstall).
//!
//! Port of libs/librepcb/editor/library/librariesmodel.{h,cpp}
//! ([`LibrariesModel`], both modes) and of the library panel actions of
//! libs/librepcb/editor/mainwindow.cpp. The network operations (library
//! list, icons, downloads) run on the tokio runtime of [`network`] and post
//! their results back to the UI thread.
//!
//! Differences to upstream: no automatic update check/installation timer
//! (the list is fetched when the panel is shown or "check for updates" is
//! clicked); the download progress of a library is only "running" or
//! "done" (the installer of `librepcb-network` downloads, and falls back
//! to cloning the repository with git).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_core::fileio::FilePath;
use librepcb_core::types::{Uuid, Version};
use librepcb_core::workspace::{ElementKind, LibraryDb};
use librepcb_i18n::tr;
use librepcb_network::Library as OnlineLibrary;

use crate::icons::decode_png;
use crate::models::UiModel;

/// The libraries of one section of the libraries panel (upstream
/// `LibrariesModel`): the rows shown (after filtering) and their state.
pub struct LibrariesModel {
    remote: bool,
    initialized: bool,
    installed: Vec<ui::LibraryInfoData>,
    installed_dirs: HashMap<Uuid, HashSet<FilePath>>,
    installed_errors: Vec<String>,
    online: BTreeMap<Uuid, OnlineLibrary>,
    online_errors: Vec<String>,
    /// Running library list requests (a generation counter: results of
    /// canceled requests are ignored).
    fetching: bool,
    fetch_generation: u64,
    check_states: HashMap<Uuid, bool>,
    merged: Vec<ui::LibraryInfoData>,
    icons: HashMap<Uuid, slint::Image>,
    requested_icons: HashSet<Uuid>,
    downloads: HashSet<Uuid>,
    highlighted: Option<FilePath>,
    filter: String,
    /// Indices into `merged` of the rows of `model`.
    visible: Vec<usize>,
    model: Rc<UiModel<ui::LibraryInfoData>>,
}

/// The changes to apply (upstream `applyChanges()`).
#[derive(Debug, Clone, Default)]
pub struct PendingChanges {
    /// Library directories to remove.
    pub uninstall: Vec<FilePath>,
    /// Libraries to install or update.
    pub install: Vec<OnlineLibrary>,
    /// Existing directories of the libraries to install (replaced).
    pub existing: HashMap<Uuid, HashSet<FilePath>>,
}

impl LibrariesModel {
    /// A model for the local (`remote == false`) or remote libraries.
    pub fn new(remote: bool) -> Self {
        Self {
            remote,
            initialized: false,
            installed: Vec::new(),
            installed_dirs: HashMap::new(),
            installed_errors: Vec::new(),
            online: BTreeMap::new(),
            online_errors: Vec::new(),
            fetching: false,
            fetch_generation: 0,
            check_states: HashMap::new(),
            merged: Vec::new(),
            icons: HashMap::new(),
            requested_icons: HashSet::new(),
            downloads: HashSet::new(),
            highlighted: None,
            filter: String::new(),
            visible: Vec::new(),
            model: UiModel::shared(Vec::new()),
        }
    }

    /// The model of the shown rows (`Data.local-libraries` or
    /// `Data.remote-libraries`).
    pub fn model(&self) -> &Rc<UiModel<ui::LibraryInfoData>> {
        &self.model
    }

    /// All libraries (unfiltered).
    pub fn libraries(&self) -> &[ui::LibraryInfoData] {
        &self.merged
    }

    /// Whether the online library list is being fetched.
    pub fn is_fetching(&self) -> bool {
        self.fetching
    }

    /// Whether the online libraries were fetched (or failed).
    pub fn has_online_libraries(&self) -> bool {
        !self.online.is_empty()
    }

    /// Whether libraries are being installed.
    pub fn is_installing(&self) -> bool {
        !self.downloads.is_empty()
    }

    /// Highlights the library of `dir` after the next rescan (upstream
    /// `highlightLibraryOnNextRescan()`).
    pub fn highlight_on_next_rescan(&mut self, dir: FilePath) {
        self.highlighted = Some(dir);
    }

    /// Reloads the installed libraries from the library database (upstream
    /// `updateLibraries()`).
    pub fn update_installed(&mut self, db: &LibraryDb, remote_dir: &FilePath, locales: &[String]) {
        self.installed.clear();
        self.installed_dirs.clear();
        self.installed_errors.clear();
        match db.all(ElementKind::Library, None, None) {
            Ok(libraries) => {
                let mut uuids = HashSet::new();
                // Latest versions first, so older duplicates are marked.
                for (_, dir) in libraries.iter().rev() {
                    let Ok(Some(info)) = db.metadata(ElementKind::Library, dir) else {
                        continue;
                    };
                    let icon = db
                        .library_metadata(dir)
                        .ok()
                        .flatten()
                        .filter(|m| !m.icon_png.is_empty())
                        .and_then(|m| decode_png(&m.icon_png));
                    if let Some(icon) = &icon {
                        self.icons.insert(info.uuid, icon.clone());
                    }
                    let is_remote = dir.as_path().starts_with(remote_dir.as_path());
                    if is_remote != self.remote {
                        continue;
                    }
                    let texts = db
                        .translations(ElementKind::Library, dir, locales)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let duplicate = uuids.contains(&info.uuid);
                    self.installed.push(ui::LibraryInfoData {
                        uuid: info.uuid.to_string().into(),
                        path: dir.to_native().into(),
                        icon: icon.unwrap_or_default(),
                        name: texts.name.into(),
                        description: texts.description.into(),
                        installed_version: info.version.to_string().into(),
                        duplicate,
                        checked: !duplicate,
                        highlight: self.highlighted.as_ref() == Some(dir),
                        ..Default::default()
                    });
                    self.installed_dirs
                        .entry(info.uuid)
                        .or_default()
                        .insert(dir.clone());
                    uuids.insert(info.uuid);
                }
            }
            Err(e) => {
                log::error!("Failed to update the library list: {e}");
                self.installed_errors.push(e.to_string());
            }
        }
        self.installed.sort_by(|a, b| a.name.cmp(&b.name));
        self.initialized = true;
        self.highlighted = None;
        self.update_merged();
    }

    /// Starts fetching the online libraries: returns the generation to pass
    /// to [`set_online()`](Self::set_online) (upstream
    /// `requestOnlineLibraries()`).
    pub fn start_fetch(&mut self) -> u64 {
        self.fetch_generation += 1;
        self.fetching = true;
        self.online.clear();
        self.online_errors.clear();
        self.update_merged();
        self.fetch_generation
    }

    /// Cancels fetching the online libraries (upstream
    /// `cancelUpdateCheck()`).
    pub fn cancel_fetch(&mut self) {
        self.fetch_generation += 1;
        self.fetching = false;
        self.online_errors.clear();
    }

    /// The online libraries of a fetch were received (or failed with the
    /// errors of the API endpoints); ignored if the fetch was canceled.
    /// Returns whether they were applied.
    pub fn set_online(
        &mut self,
        generation: u64,
        libraries: Vec<OnlineLibrary>,
        errors: Vec<String>,
    ) -> bool {
        if generation != self.fetch_generation {
            return false;
        }
        self.fetching = false;
        for lib in libraries {
            self.online.insert(lib.uuid, lib);
        }
        self.online_errors = errors;
        self.update_merged();
        true
    }

    /// The online versions of libraries (for the local libraries, upstream
    /// `setOnlineVersions()`).
    pub fn set_online_versions(&mut self, versions: &HashMap<Uuid, Version>) {
        for (i, lib) in self.merged.iter_mut().enumerate() {
            let Some(version) = lib.uuid.parse::<Uuid>().ok().and_then(|u| versions.get(&u)) else {
                continue;
            };
            let online = version.to_string();
            if lib.online_version != online.as_str() {
                lib.online_version = online.into();
                lib.outdated = lib
                    .installed_version
                    .parse::<Version>()
                    .is_ok_and(|v| *version > v);
                if let Some(row) = self.visible.iter().position(|v| *v == i) {
                    self.model.set(row, lib.clone());
                }
            }
        }
    }

    /// The online libraries without icon (to request them).
    pub fn missing_icons(&mut self) -> Vec<(Uuid, librepcb_network::Url)> {
        let mut result = Vec::new();
        for lib in self.online.values() {
            if !self.icons.contains_key(&lib.uuid) && !self.requested_icons.contains(&lib.uuid) {
                if let Some(url) = &lib.icon_url {
                    result.push((lib.uuid, url.clone()));
                }
                self.requested_icons.insert(lib.uuid);
            }
        }
        result
    }

    /// An icon of an online library was received (PNG data).
    pub fn set_icon(&mut self, uuid: Uuid, png: &[u8]) {
        let Some(icon) = decode_png(png) else {
            return;
        };
        self.icons.insert(uuid, icon.clone());
        let uuid = uuid.to_string();
        for (i, lib) in self.merged.iter_mut().enumerate() {
            if lib.uuid == uuid.as_str() {
                lib.icon = icon.clone();
                if let Some(row) = self.visible.iter().position(|v| *v == i) {
                    self.model.set(row, lib.clone());
                }
            }
        }
    }

    /// Merges the installed and the online libraries (upstream
    /// `updateMergedLibraries()`).
    fn update_merged(&mut self) {
        let mut merged = self.installed.clone();
        for lib in self.online.values() {
            let uuid = lib.uuid.to_string();
            let mut is_installed = false;
            for installed in merged.iter_mut().filter(|l| l.uuid == uuid.as_str()) {
                installed.online_version = lib.version.to_string().into();
                installed.outdated = installed
                    .installed_version
                    .parse::<Version>()
                    .is_ok_and(|v| lib.version > v);
                installed.recommended = lib.recommended;
                is_installed = true;
            }
            if !is_installed {
                merged.push(ui::LibraryInfoData {
                    uuid: uuid.into(),
                    icon: self.icons.get(&lib.uuid).cloned().unwrap_or_default(),
                    name: lib.name.as_str().into(),
                    description: lib.description.as_str().into(),
                    online_version: lib.version.to_string().into(),
                    recommended: lib.recommended,
                    ..Default::default()
                });
            }
        }
        self.merged = merged;
        self.check_missing_dependencies();
        self.update_check_states();
        self.refresh_model();
    }

    /// Sets the filter of the shown rows (case-insensitive substring of
    /// the name).
    pub fn set_filter(&mut self, filter: &str) {
        if filter != self.filter {
            self.filter = filter.to_owned();
            self.refresh_model();
        }
    }

    fn refresh_model(&mut self) {
        let filter = self.filter.to_lowercase();
        self.visible = self
            .merged
            .iter()
            .enumerate()
            .filter(|(_, l)| filter.is_empty() || l.name.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect();
        self.model.replace_all(
            self.visible
                .iter()
                .map(|i| self.merged[*i].clone())
                .collect(),
        );
    }

    fn update_model_rows(&self) {
        for (row, i) in self.visible.iter().enumerate() {
            if self.model.get(row).as_ref() != Some(&self.merged[*i]) {
                self.model.set(row, self.merged[*i].clone());
            }
        }
    }

    /// The UI toggled the switch of a shown row (upstream
    /// `set_row_data()`).
    pub fn row_written(&mut self, row: usize, data: &ui::LibraryInfoData) {
        let Some(i) = self.visible.get(row).copied() else {
            return;
        };
        self.set_checked(i, data.checked);
    }

    /// Checks or unchecks a library (by index into
    /// [`libraries()`](Self::libraries)), with its dependencies.
    pub fn set_checked(&mut self, index: usize, checked: bool) {
        let Some(lib) = self.merged.get(index) else {
            return;
        };
        let Ok(uuid) = lib.uuid.parse::<Uuid>() else {
            return;
        };
        if checked == lib.checked {
            return;
        }
        self.check_states.insert(uuid, checked);
        if checked {
            self.check_missing_dependencies();
        } else {
            self.uncheck_libs_with_unmet_dependencies();
        }
        self.update_check_states();
        self.update_model_rows();
    }

    /// Checks all libraries if at most half are checked, otherwise
    /// unchecks all (upstream `toggleAll()`).
    pub fn toggle_all(&mut self) {
        if self.merged.is_empty() {
            return;
        }
        let checked = self.merged.iter().filter(|l| l.checked).count();
        let new_state = checked <= self.merged.len() / 2;
        for lib in &self.merged {
            if let Ok(uuid) = lib.uuid.parse::<Uuid>() {
                self.check_states.insert(uuid, new_state);
            }
        }
        if new_state {
            self.check_missing_dependencies();
        } else {
            self.uncheck_libs_with_unmet_dependencies();
        }
        self.update_check_states();
        self.update_model_rows();
    }

    /// Discards the pending changes (upstream `cancel()`; running
    /// operations are not aborted).
    pub fn cancel(&mut self) {
        self.check_states.clear();
        self.update_merged();
    }

    fn is_checked(&self, lib: &ui::LibraryInfoData) -> bool {
        lib.uuid.parse::<Uuid>().is_ok_and(|uuid| {
            !lib.duplicate
                && self
                    .check_states
                    .get(&uuid)
                    .copied()
                    .unwrap_or(!lib.installed_version.is_empty())
        })
    }

    fn update_check_states(&mut self) {
        let states: Vec<bool> = self.merged.iter().map(|l| self.is_checked(l)).collect();
        for (lib, checked) in self.merged.iter_mut().zip(states) {
            lib.checked = checked;
        }
    }

    fn check_missing_dependencies(&mut self) {
        let mut to_install: HashSet<Uuid> = self
            .merged
            .iter()
            .filter(|l| self.is_checked(l))
            .filter_map(|l| l.uuid.parse().ok())
            .collect();
        loop {
            let count = to_install.len();
            for lib in self.online.values() {
                if to_install.contains(&lib.uuid) {
                    to_install.extend(lib.dependencies.iter().copied());
                }
            }
            if to_install.len() == count {
                break;
            }
        }
        for uuid in to_install {
            self.check_states.insert(uuid, true);
        }
    }

    fn uncheck_libs_with_unmet_dependencies(&mut self) {
        let mut to_uninstall: HashSet<Uuid> = self
            .merged
            .iter()
            .filter(|l| !self.is_checked(l))
            .filter_map(|l| l.uuid.parse().ok())
            .collect();
        loop {
            let count = to_uninstall.len();
            for lib in self.online.values() {
                if lib.dependencies.iter().any(|d| to_uninstall.contains(d)) {
                    to_uninstall.insert(lib.uuid);
                }
            }
            if to_uninstall.len() == count {
                break;
            }
        }
        for uuid in to_uninstall {
            self.check_states.insert(uuid, false);
        }
    }

    /// The `LibraryListData` (upstream `getUiData()`).
    pub fn ui_data(&self) -> ui::LibraryListData {
        let mut data = ui::LibraryListData {
            refreshing: !self.initialized || self.fetching,
            refreshing_error: self
                .installed_errors
                .iter()
                .chain(&self.online_errors)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n")
                .into(),
            count: self.merged.len() as i32,
            installed: self.installed.len() as i32,
            operation_in_progress: !self.downloads.is_empty(),
            ..Default::default()
        };
        for lib in &self.merged {
            if lib.duplicate {
                data.pending_uninstalls += 1;
                continue;
            }
            if lib.outdated {
                data.outdated += 1;
            }
            if is_marked_for_install(lib) {
                data.pending_installs += 1;
            } else if is_marked_for_update(lib) {
                data.pending_updates += 1;
            } else if is_marked_for_uninstall(lib) {
                data.pending_uninstalls += 1;
                if lib.online_version.is_empty() {
                    data.pending_oneway_uninstalls += 1;
                }
            }
        }
        data.all_up_to_date = data.installed > 0 && data.outdated == 0 && !self.online.is_empty();
        data
    }

    /// The changes to apply and marks the libraries to install as being
    /// installed (upstream `applyChanges()`).
    pub fn take_pending_changes(&mut self) -> PendingChanges {
        let mut changes = PendingChanges::default();
        for lib in &self.merged {
            let Ok(uuid) = lib.uuid.parse::<Uuid>() else {
                continue;
            };
            if is_marked_for_uninstall(lib) {
                if let Some(fp) = FilePath::new(lib.path.as_str()) {
                    changes.uninstall.push(fp.clone());
                    if let Some(dirs) = self.installed_dirs.get_mut(&uuid) {
                        dirs.remove(&fp);
                    }
                }
            } else if (is_marked_for_install(lib) || is_marked_for_update(lib))
                && let Some(online) = self.online.get(&uuid)
            {
                changes.install.push(online.clone());
                changes.existing.insert(
                    uuid,
                    self.installed_dirs.get(&uuid).cloned().unwrap_or_default(),
                );
            }
        }
        for lib in &changes.install {
            self.downloads.insert(lib.uuid);
            self.set_progress(lib.uuid, 1);
        }
        changes
    }

    fn set_progress(&mut self, uuid: Uuid, percent: i32) {
        let uuid = uuid.to_string();
        for lib in self.merged.iter_mut().filter(|l| l.uuid == uuid.as_str()) {
            lib.progress = percent;
        }
        self.update_model_rows();
    }

    /// An installation finished (successfully or not); returns whether all
    /// installations are done.
    pub fn install_finished(&mut self, uuid: Uuid) -> bool {
        self.downloads.remove(&uuid);
        self.set_progress(uuid, 0);
        self.downloads.is_empty()
    }
}

fn is_marked_for_install(lib: &ui::LibraryInfoData) -> bool {
    lib.installed_version.is_empty() && lib.checked && !lib.duplicate
}

fn is_marked_for_update(lib: &ui::LibraryInfoData) -> bool {
    !lib.installed_version.is_empty() && lib.outdated && lib.checked && !lib.duplicate
}

fn is_marked_for_uninstall(lib: &ui::LibraryInfoData) -> bool {
    (!lib.installed_version.is_empty() && !lib.checked) || lib.duplicate
}

/// The status bar message after fetching the online libraries (upstream
/// `apiEndpointOperationFinished()`).
pub fn update_check_message(data: &ui::LibraryListData) -> Option<String> {
    if data.all_up_to_date {
        Some(tr!(
            "librepcb::editor::LibrariesModel",
            "All libraries are up-to-date"
        ))
    } else if data.pending_updates > 0 {
        Some(librepcb_i18n::trn!(
            "librepcb::editor::LibrariesModel",
            "Update available for {n} libraries",
            "Update available for {n} libraries",
            data.pending_updates as u64
        ))
    } else {
        None
    }
}

/// The key handling of the libraries panel filter (upstream
/// `SlintKeyEventTextBuilder`): printable text is appended, Backspace
/// removes the last character, Escape clears it. Returns the new filter
/// if the key was handled.
pub fn filter_key_pressed(filter: &str, text: &str, control: bool) -> Option<String> {
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if c == '\u{8}' {
        let mut f = filter.to_owned();
        f.pop()?;
        return Some(f);
    }
    if c == '\u{1b}' {
        return (!filter.is_empty()).then(String::new);
    }
    if control || c.is_control() || (c as u32) >= 0xF700 && (c as u32) <= 0xF8FF {
        return None;
    }
    if filter.is_empty() && c.is_whitespace() {
        return None;
    }
    Some(format!("{filter}{c}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn online(uuid: &str, name: &str, version: &str, deps: &[&str]) -> OnlineLibrary {
        OnlineLibrary {
            uuid: uuid.parse().unwrap(),
            name: name.into(),
            description: String::new(),
            author: String::new(),
            version: version.parse().unwrap(),
            recommended: false,
            dependencies: deps.iter().map(|d| d.parse().unwrap()).collect(),
            icon_url: None,
            download_url: None,
            download_size: None,
            download_sha256: String::new(),
            repository_url: None,
        }
    }

    const BASE: &str = "11111111-1111-4111-8111-111111111111";
    const CONN: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn install_with_dependencies_and_update() {
        let mut m = LibrariesModel::new(true);
        m.installed.push(ui::LibraryInfoData {
            uuid: BASE.into(),
            path: "/ws/data/libraries/remote/base.lplib".into(),
            name: "Base".into(),
            installed_version: "0.1".into(),
            checked: true,
            ..Default::default()
        });
        m.initialized = true;
        let generation = m.start_fetch();
        assert!(m.ui_data().refreshing);
        assert!(m.set_online(
            generation,
            vec![
                online(BASE, "Base", "0.2", &[]),
                online(CONN, "Connectors", "1.0", &[BASE]),
            ],
            Vec::new(),
        ));
        let data = m.ui_data();
        assert!(!data.refreshing);
        assert_eq!(data.count, 2);
        assert_eq!(data.outdated, 1);
        assert_eq!(data.pending_updates, 1, "outdated and checked");
        assert_eq!(data.pending_installs, 0);
        // Checking "Connectors" keeps its dependency checked.
        let conn = m.libraries().iter().position(|l| l.uuid == CONN).unwrap();
        m.set_checked(conn, true);
        assert_eq!(m.ui_data().pending_installs, 1);
        // Unchecking "Base" unchecks "Connectors" (unmet dependency).
        let base = m.libraries().iter().position(|l| l.uuid == BASE).unwrap();
        m.set_checked(base, false);
        let data = m.ui_data();
        assert_eq!(data.pending_installs, 0);
        assert_eq!(data.pending_uninstalls, 1);
        // Cancel restores the defaults.
        m.cancel();
        let data = m.ui_data();
        assert_eq!((data.pending_installs, data.pending_uninstalls), (0, 0));
        assert_eq!(data.pending_updates, 1);
        // Toggle all: all checked (install "Connectors").
        m.toggle_all();
        assert_eq!(m.ui_data().pending_installs, 1);
        let changes = m.take_pending_changes();
        assert_eq!(changes.install.len(), 2, "update and install");
        assert!(m.ui_data().operation_in_progress);
        assert!(!m.install_finished(BASE.parse().unwrap()));
        assert!(m.install_finished(CONN.parse().unwrap()));
        // A canceled fetch is ignored.
        let generation = m.start_fetch();
        m.cancel_fetch();
        assert!(!m.set_online(generation, Vec::new(), Vec::new()));
    }

    #[test]
    fn filter() {
        let mut m = LibrariesModel::new(false);
        for name in ["Base", "Connectors"] {
            m.installed.push(ui::LibraryInfoData {
                uuid: if name == "Base" { BASE } else { CONN }.into(),
                name: name.into(),
                installed_version: "1".into(),
                checked: true,
                ..Default::default()
            });
        }
        m.update_merged();
        assert_eq!(m.model().len(), 2);
        m.set_filter("conn");
        assert_eq!(m.model().len(), 1);
        assert_eq!(filter_key_pressed("", "a", false).as_deref(), Some("a"));
        assert_eq!(
            filter_key_pressed("ab", "\u{8}", false).as_deref(),
            Some("a")
        );
        assert_eq!(
            filter_key_pressed("ab", "\u{1b}", false).as_deref(),
            Some("")
        );
        assert_eq!(filter_key_pressed("", "\u{1b}", false), None);
        assert_eq!(filter_key_pressed("", "a", true), None);
    }
}
