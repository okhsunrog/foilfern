//! Port of libs/librepcb/core/fileio/outputdirectorywriter.{h,cpp}.
//!
//! Keeps track of the files written into an output directory by output
//! jobs, in the index file `.librepcb-output` (one line
//! `<relative path> | <job uuid>` per file, sorted, `\n` terminated). This
//! allows detecting files written multiple times, and removing obsolete or
//! unknown files later.
//!
//! The upstream signals `aboutToWriteFile()` / `aboutToRemoveFile()` are
//! replaced by an optional observer callback (used for logging by the job
//! runner and the UI).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use super::error::{Error, Result};
use super::file_path::FilePath;
use super::file_utils;
use crate::types::Uuid;

/// File operation reported to the observer of an [`OutputDirectoryWriter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputDirectoryEvent<'a> {
    /// A file is about to be written.
    AboutToWriteFile(&'a FilePath),
    /// A file is about to be removed.
    AboutToRemoveFile(&'a FilePath),
}

/// Observer callback of an [`OutputDirectoryWriter`].
pub type OutputDirectoryObserver = Box<dyn FnMut(OutputDirectoryEvent<'_>) + Send>;

/// Writer for output job files (see the [module docs](self)).
pub struct OutputDirectoryWriter {
    dir_path: FilePath,
    index_file_path: FilePath,
    index: BTreeMap<FilePath, Uuid>,
    index_loaded: bool,
    index_modified: bool,
    written_files: HashMap<Uuid, Vec<FilePath>>,
    observer: Option<OutputDirectoryObserver>,
}

impl OutputDirectoryWriter {
    /// Creates a writer for the given output directory.
    pub fn new(dir_path: &FilePath) -> Self {
        Self {
            dir_path: dir_path.clone(),
            index_file_path: dir_path.path_to(".librepcb-output"),
            index: BTreeMap::new(),
            index_loaded: false,
            index_modified: false,
            written_files: HashMap::new(),
            observer: None,
        }
    }

    /// Sets the observer which is notified about file operations.
    pub fn set_observer(&mut self, observer: Option<OutputDirectoryObserver>) {
        self.observer = observer;
    }

    /// Returns the output directory.
    pub fn directory_path(&self) -> &FilePath {
        &self.dir_path
    }

    /// Returns all files written so far, by job.
    pub fn written_files(&self) -> &HashMap<Uuid, Vec<FilePath>> {
        &self.written_files
    }

    fn notify(&mut self, event: OutputDirectoryEvent<'_>) {
        if let Some(observer) = &mut self.observer {
            observer(event);
        }
    }

    /// Loads the index file (if it exists).
    ///
    /// Returns `false` if the index could not be loaded (the error is
    /// logged); the writer is usable anyway.
    pub fn load_index(&mut self) -> bool {
        self.index.clear();
        let success = match self.read_index() {
            Ok(()) => true,
            Err(e) => {
                log::error!("{e}");
                false
            }
        };
        self.index_loaded = true;
        self.index_modified = false;
        success
    }

    fn read_index(&mut self) -> Result<()> {
        if self.index_file_path.is_existing_file() {
            let content = file_utils::read_file(&self.index_file_path)?;
            let content = String::from_utf8_lossy(&content);
            for line in content.split('\n').filter(|l| !l.is_empty()) {
                let values: Vec<&str> = line.split(" | ").collect();
                if values.len() >= 2 {
                    let uuid: Uuid = values[1].parse()?;
                    self.index.insert(self.dir_path.path_to(values[0]), uuid);
                }
            }
        }
        Ok(())
    }

    /// Writes the index file (only entries of existing files).
    pub fn store_index(&mut self) -> Result<()> {
        let mut lines: Vec<String> = self
            .index
            .iter()
            .filter(|(fp, _)| fp.is_existing_file())
            .map(|(fp, uuid)| format!("{} | {uuid}", fp.to_relative(&self.dir_path)))
            .collect();
        lines.sort();
        let content = lines.join("\n") + "\n";
        file_utils::write_file(&self.index_file_path, content.as_bytes())?;
        self.index_modified = false;
        Ok(())
    }

    /// Registers a file to be written by `job` and returns its absolute path.
    ///
    /// Fails if the path is invalid, absolute, outside the output directory,
    /// or was already written before by any job.
    pub fn begin_writing_file(&mut self, job: &Uuid, rel_path: &str) -> Result<FilePath> {
        let fp = self.dir_path.path_to(rel_path);
        self.notify(OutputDirectoryEvent::AboutToWriteFile(&fp));

        if !self.index_loaded {
            return Err(Error::IndexNotLoaded);
        }

        // Throw a proper error if the given path is not valid.
        if fp == self.dir_path {
            return Err(Error::InvalidOutputPath(rel_path.to_owned()));
        }

        // For security reasons, do not allow to write any files outside the
        // output directory!
        if !fp.is_located_in_dir(&self.dir_path) {
            return Err(Error::OutputPathOutsideDirectory(rel_path.to_owned()));
        }

        // Explicitly handle absolute file paths because `fp` might be a valid
        // path even though `rel_path` was absolute.
        let rel = Path::new(rel_path);
        if rel.is_absolute() || rel.has_root() {
            return Err(Error::AbsoluteOutputPath(rel_path.to_owned()));
        }

        // Special cases, unlikely to happen.
        if fp == self.index_file_path {
            return Err(Error::OverwriteOutputIndex(
                self.index_file_path.file_name().to_owned(),
            ));
        }
        if rel_path.contains('|') {
            return Err(Error::PipeInOutputPath);
        }

        // The main purpose of this whole class: detect if a file is written
        // multiple times.
        if self.written_files.values().flatten().any(|f| *f == fp) {
            return Err(Error::OutputFileWrittenMultipleTimes(
                fp.to_relative_native(&self.dir_path),
            ));
        }

        self.index.insert(fp.clone(), *job);
        self.index_modified = true;
        self.written_files.entry(*job).or_default().push(fp.clone());
        Ok(fp)
    }

    /// Removes files which were previously written by `job` but not in this
    /// run.
    pub fn remove_obsolete_files(&mut self, job: &Uuid) -> Result<()> {
        let obsolete: Vec<FilePath> = self
            .index
            .iter()
            .filter(|(fp, uuid)| {
                *uuid == job
                    && !self
                        .written_files
                        .get(job)
                        .is_some_and(|files| files.contains(fp))
            })
            .map(|(fp, _)| fp.clone())
            .collect();
        for fp in obsolete {
            self.notify(OutputDirectoryEvent::AboutToRemoveFile(&fp));
            if fp.is_existing_file() {
                file_utils::remove_file(&fp)?;
            }
            self.index.remove(&fp);
            // upstream: does not set mIndexModified here either.
        }
        Ok(())
    }

    /// Returns all (non-hidden) files in the output directory which are
    /// neither the index file nor written by one of `known_jobs`.
    pub fn find_unknown_files(&self, known_jobs: &HashSet<Uuid>) -> Result<Vec<FilePath>> {
        if !self.index_loaded {
            return Err(Error::IndexNotLoaded);
        }
        if !self.dir_path.is_existing_dir() {
            return Ok(Vec::new());
        }
        // Note: Ignore hidden files such as .DS_Store or Thumbs.db.
        let mut result = file_utils::files_in_directory(&self.dir_path, &[], true, true)?;
        result.retain(|fp| {
            *fp != self.index_file_path
                && !self
                    .index
                    .get(fp)
                    .is_some_and(|uuid| known_jobs.contains(uuid))
        });
        Ok(result)
    }

    /// Removes the given files and their parent directories if they became
    /// empty (up to, but excluding, the output directory).
    pub fn remove_unknown_files(&mut self, files: &[FilePath]) -> Result<()> {
        if !self.index_loaded {
            return Err(Error::IndexNotLoaded);
        }
        for fp in files {
            self.notify(OutputDirectoryEvent::AboutToRemoveFile(fp));
            file_utils::remove_file(fp)?;
            // Remove empty parents (upstream: QDir::rmpath()).
            let mut dir = fp.parent_dir();
            while let Some(d) = dir {
                if !d.is_located_in_dir(&self.dir_path) || std::fs::remove_dir(&d).is_err() {
                    break;
                }
                dir = d.parent_dir();
            }
        }
        Ok(())
    }
}

impl Drop for OutputDirectoryWriter {
    fn drop(&mut self) {
        if self.index_modified
            && let Err(e) = self.store_index()
        {
            log::error!("Failed to automatically store output directory index: {e}");
        }
    }
}

impl std::fmt::Debug for OutputDirectoryWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputDirectoryWriter")
            .field("dir_path", &self.dir_path)
            .field("index", &self.index)
            .field("written_files", &self.written_files)
            .finish_non_exhaustive()
    }
}
