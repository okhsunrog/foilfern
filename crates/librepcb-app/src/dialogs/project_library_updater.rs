//! The project library updater.
//!
//! Port of libs/librepcb/editor/workspace/projectlibraryupdater/
//! (`projectlibraryupdater.{h,cpp,ui}`): replaces all library elements of
//! a project which exist in the workspace libraries by their latest
//! version, checks that the project can still be opened and saves it.
//!
//! Upstream closes the project first (asking to save it) and reopens it
//! afterwards. Here the application does the same around
//! [`update_project_library()`] when the update button is clicked
//! ([`AppRequest::UpdateProjectLibrary`]); a project with unsaved changes
//! is not closed (the update is aborted, see COMPAT.md). The dialog is then
//! shown again with the log.

use std::sync::Arc;

use librepcb_core::application;
use librepcb_core::fileio::{
    FilePath, RestoreMode, TransactionalDirectory, TransactionalFileSystem, VersionFile,
};
use librepcb_core::project::ProjectLoader;
use librepcb_core::types::LengthUnit;
use librepcb_core::workspace::{ElementKind, LibraryDb};
use librepcb_i18n::tr;

use super::{
    AppRequest, Applied, ButtonResult, DialogContext, DialogOptions, Form, FormDialog, ListButtons,
    ListItem,
};

const CTX: &str = "librepcb::editor::ProjectLibraryUpdater";

/// Upstream `ProjectLibraryUpdater::log()`-style messages.
fn log_line(log: &mut Vec<String>, msg: String) {
    log::info!("{msg}");
    log.push(msg);
}

/// Updates the library elements of the (closed) project `project` from
/// the workspace library database (upstream `btnUpdateClicked()` after
/// closing the project); returns whether it succeeded. Messages are
/// appended to `log`.
pub fn update_project_library(db: &LibraryDb, project: &FilePath, log: &mut Vec<String>) -> bool {
    match update_impl(db, project, log) {
        Ok(()) => {
            log_line(log, tr!(CTX, "[SUCCESS] All library elements updated."));
            true
        }
        Err(e) => {
            log_line(log, tr!(CTX, "[ERROR] {0}", e));
            false
        }
    }
}

fn update_impl(db: &LibraryDb, project: &FilePath, log: &mut Vec<String>) -> Result<(), String> {
    let dir = project.parent_dir().ok_or_else(|| project.to_native())?;
    log_line(log, tr!(CTX, "Open project file system..."));
    let fs = Arc::new(
        TransactionalFileSystem::open(&dir, true, RestoreMode::Abort, None)
            .map_err(|e| e.to_string())?,
    );
    // Abort if the file format is outdated because it would lead to errors
    // when library elements with a higher file format version get copied
    // into the project.
    let version =
        VersionFile::from_bytes(&fs.read(".librepcb-project").map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if *version.version() < application::file_format_version() {
        return Err(tr!(
            CTX,
            "The project uses an outdated file format.\nPlease upgrade it to the latest file format first, review the upgrade messages and then save the project.\nAfterwards the project library can be updated."
        ));
    }
    for (kind, dir_name) in [
        (ElementKind::Component, "cmp"),
        (ElementKind::Device, "dev"),
        (ElementKind::Package, "pkg"),
        (ElementKind::Symbol, "sym"),
    ] {
        let dir_path = format!("library/{dir_name}");
        for name in fs.dirs(&dir_path) {
            let src = name
                .parse()
                .ok()
                .and_then(|uuid| db.latest(kind, uuid).ok().flatten());
            let dst = format!("{dir_path}/{name}");
            let dst_has_files = !fs.files(&dst).is_empty();
            match src {
                Some(src) if dst_has_files => {
                    log_line(log, tr!(CTX, "Update {0}...", dst));
                    let src_fs = Arc::new(
                        TransactionalFileSystem::open_ro(&src).map_err(|e| e.to_string())?,
                    );
                    let src_dir = TransactionalDirectory::new(src_fs, "");
                    fs.remove_dir_recursively(&dst).map_err(|e| e.to_string())?;
                    let mut dst_dir = TransactionalDirectory::new(Arc::clone(&fs), &dst);
                    src_dir.copy_to(&mut dst_dir).map_err(|e| e.to_string())?;
                }
                _ => log_line(log, tr!(CTX, "Skip {0}...", dst)),
            }
        }
    }
    // Check whether the project can still be opened.
    let result = (|| -> Result<(), String> {
        log_line(log, tr!(CTX, "Open project {0}...", project.file_name()));
        let mut loader = ProjectLoader::new();
        loader.set_auto_assign_device_models(true); // Make use of new 3D models.
        let mut p = loader
            .open(
                TransactionalDirectory::new(Arc::clone(&fs), ""),
                project.file_name(),
            )
            .map_err(|e| e.to_string())?;
        log_line(log, tr!(CTX, "Save project {0}...", project.file_name()));
        p.save().map_err(|e| e.to_string())?; // Force upgrading the file format.
        fs.save().map_err(|e| e.to_string())
    })();
    if let Err(e) = result {
        // Something is broken: the modifications are discarded.
        log_line(log, tr!(CTX, "[ERROR] {0}", e));
        return Err(tr!(
            CTX,
            "Failed to update library elements! Probably there were breaking changes in some library elements."
        ));
    }
    Ok(())
}

/// The project library updater dialog (upstream `ProjectLibraryUpdater`).
pub struct ProjectLibraryUpdaterDialog {
    form: Form,
    project: FilePath,
}

impl ProjectLibraryUpdaterDialog {
    /// The dialog for a project file, with the log of the last update.
    pub fn new(project: FilePath, log: &[String]) -> Self {
        let mut form = Form::new(LengthUnit::Millimeters);
        let items: Vec<ListItem> = log.iter().map(ListItem::text).collect();
        form.list("log", "", &[], &items, 15, ListButtons::default());
        if !items.is_empty() {
            form.set_items("log", &items, Some(items.len() - 1));
        }
        Self { form, project }
    }
}

impl FormDialog for ProjectLibraryUpdaterDialog {
    fn title(&self) -> String {
        tr!(CTX, "Project Library Updater")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            cancel: false,
            ok_text: Some(tr!("ProjectLibraryTab", "Close")),
            extra_buttons: vec![tr!(
                CTX,
                "Update library of \"{0}\"!",
                self.project.basename()
            )],
            width: 650.0,
            label_width: 0.0,
            ..DialogOptions::default()
        }
    }

    fn button(&mut self, _ctx: &DialogContext<'_>, _index: usize) -> Result<ButtonResult, String> {
        Ok(ButtonResult::App(AppRequest::UpdateProjectLibrary(
            self.project.clone(),
        )))
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        Ok(Applied::Nothing)
    }
}
