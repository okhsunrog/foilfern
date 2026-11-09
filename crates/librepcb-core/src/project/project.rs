//! Port of libs/librepcb/core/project/project.{h,cpp}.
//!
//! Differences to upstream: the project is a plain data tree (see
//! `docs/project-model-design.md`); all modifications go through
//! [`Project::apply()`] (in `mutation/`), the Qt signals are replaced by
//! the change journal ([`Project::changes_since()`]), and the reverse
//! relations upstream keeps as registration lists are answered by the
//! private reverse index ([`Project::is_net_signal_used()`], ...).
//! `create()` does not copy the stroke fonts of the
//! application resources yet.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::board::Board;
use super::change::{Change, ChangeLog, ChangesSince};
use super::circuit::{AssemblyVariant, Circuit, NetClass};
use super::error::{Error, Result};
use super::id::{BoardId, BusId, BusSegmentId, ComponentInstanceId, NetSignalId, SchematicId};
use super::library::ProjectLibrary;
use super::ref_index::{ComponentUses, NetUse, RefIndex};
use super::schematic::Schematic;
use crate::application;
use crate::attribute::AttributeList;
use crate::fileio::{FilePath, FileSystem, TransactionalDirectory, VersionFile};
use crate::font::StrokeFontPool;
use crate::job::OutputJobList;
use crate::serialization::{List, Mode, SExpression, SerializeObject};
use crate::types::{ElementName, FileProofName, Uuid};

/// Content of the `*.lpp` project file.
const PROJECT_FILE_CONTENT: &[u8] = b"LIBREPCB-PROJECT";

/// The metadata of a project (`project/metadata.lp` except the UUID).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectMetadata {
    /// The name.
    pub name: ElementName,
    /// The author.
    pub author: String,
    /// The version.
    pub version: FileProofName,
    /// Date/time of creation.
    pub created: DateTime<Utc>,
    /// The attributes.
    pub attributes: AttributeList,
}

/// The settings of a project (`project/settings.lp`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectSettings {
    /// Locales in order of preference (for library element names).
    pub locale_order: Vec<String>,
    /// Norms in order of preference (for symbol variants).
    pub norm_order: Vec<String>,
    /// Attribute keys of custom BOM columns.
    pub custom_bom_attributes: Vec<String>,
    /// Default for
    /// [`ComponentInstance::lock_assembly()`](super::circuit::ComponentInstance::lock_assembly)
    /// of new components.
    pub default_lock_component_assembly: bool,
}

/// A LibrePCB project: metadata, settings, project library, circuit,
/// schematics and boards, backed by a transactional project directory.
#[derive(Debug)]
pub struct Project {
    pub(crate) directory: TransactionalDirectory,
    pub(crate) file_name: String,
    pub(crate) uuid: Uuid,
    pub(crate) metadata: ProjectMetadata,
    pub(crate) settings: ProjectSettings,
    /// Date/time of the last open or save.
    pub(crate) date_time: DateTime<Utc>,
    /// Content of `project/jobs.lp`.
    pub(crate) output_jobs: OutputJobList,
    pub(crate) stroke_fonts: StrokeFontPool,
    pub(crate) library: ProjectLibrary,
    pub(crate) circuit: Circuit,
    pub(crate) erc_approvals: BTreeSet<SExpression>,
    /// In page order.
    pub(crate) schematics: Vec<Schematic>,
    /// The first one is the primary board.
    pub(crate) boards: Vec<Board>,
    pub(crate) refs: RefIndex,
    pub(crate) journal: ChangeLog,
}

static_assertions::assert_impl_all!(Project: Send, Sync);

/// Read-only context for item computations which need the project library
/// and the circuit (items have no back-pointers, e.g.
/// [`SchematicSymbol::pins()`](super::schematic::SchematicSymbol::pins)).
#[derive(Debug, Clone, Copy)]
pub struct ProjectView<'a> {
    /// The project library.
    pub library: &'a ProjectLibrary,
    /// The circuit.
    pub circuit: &'a Circuit,
}

impl Project {
    /// Creates a default initialized project in `directory` (upstream
    /// constructor): name "Unnamed", version "v1", no content.
    ///
    /// Fails if `file_name` does not end with `.lpp`.
    pub fn new(
        mut directory: TransactionalDirectory,
        file_name: impl Into<String>,
        uuid: Uuid,
    ) -> Result<Self> {
        let file_name = file_name.into();
        if !file_name.ends_with(".lpp") {
            return Err(Error::InvalidProjectFileSuffix);
        }
        let fonts_dir = "resources/fontobene";
        let mut fonts = Vec::new();
        for name in directory.files(fonts_dir) {
            fonts.push((
                name.clone(),
                directory.read(&format!("{fonts_dir}/{name}"))?,
            ));
        }
        let library = ProjectLibrary::new(directory.subdir("library"));
        // Second precision, like the file format.
        let now = chrono::SubsecRound::trunc_subsecs(Utc::now(), 0);
        Ok(Self {
            directory,
            file_name,
            uuid,
            metadata: ProjectMetadata {
                name: ElementName::new("Unnamed")?,
                author: String::new(),
                version: FileProofName::new("v1")?,
                created: now,
                attributes: AttributeList::new(),
            },
            settings: ProjectSettings::default(),
            date_time: now,
            output_jobs: OutputJobList::new(),
            stroke_fonts: StrokeFontPool::from_files(fonts),
            library,
            circuit: Circuit::new(),
            erc_approvals: BTreeSet::new(),
            schematics: Vec::new(),
            boards: Vec::new(),
            refs: RefIndex::default(),
            journal: ChangeLog::default(),
        })
    }

    /// Creates a new project with the default assembly variant "Std" and
    /// the net class "default" (upstream static `create()`).
    ///
    /// Fails if `directory` already contains a project.
    pub fn create(
        directory: TransactionalDirectory,
        file_name: impl Into<String>,
        mut create_uuid: impl FnMut() -> Uuid,
    ) -> Result<Self> {
        let file_name = file_name.into();
        if directory.file_exists(".librepcb-project") || directory.file_exists(&file_name) {
            return Err(Error::ProjectDirectoryNotEmpty(directory.abs_path("")));
        }
        let mut project = Self::new(directory, file_name, create_uuid())?;
        project.add_assembly_variant(
            AssemblyVariant::new(
                create_uuid(),
                FileProofName::new("Std")?,
                "Standard assembly",
            ),
            None,
        )?;
        project.add_net_class(NetClass::new(create_uuid(), ElementName::new("default")?))?;
        project.journal.reset();
        Ok(project)
    }

    /// Returns the file name of the project file (`*.lpp`, without path).
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Returns the absolute path of the project file, if the directory is
    /// on disk.
    pub fn file_path(&self) -> Option<FilePath> {
        self.directory.abs_path(&self.file_name)
    }

    /// Returns the absolute path of the project directory, if on disk.
    pub fn path(&self) -> Option<FilePath> {
        self.directory.abs_path("")
    }

    /// Returns the project directory.
    pub fn directory(&self) -> &TransactionalDirectory {
        &self.directory
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns the metadata.
    pub fn metadata(&self) -> &ProjectMetadata {
        &self.metadata
    }

    /// Returns the settings.
    pub fn settings(&self) -> &ProjectSettings {
        &self.settings
    }

    /// Returns the date/time of the last open or save.
    pub fn date_time(&self) -> DateTime<Utc> {
        self.date_time
    }

    /// Returns the output jobs (upstream `getOutputJobs()`); modified with
    /// [`Mutation::SetOutputJobs`](super::Mutation::SetOutputJobs).
    pub fn output_jobs(&self) -> &OutputJobList {
        &self.output_jobs
    }

    /// Returns the stroke fonts of the project.
    pub fn stroke_fonts(&self) -> &StrokeFontPool {
        &self.stroke_fonts
    }

    /// Returns the project library.
    pub fn library(&self) -> &ProjectLibrary {
        &self.library
    }

    /// Returns the circuit.
    pub fn circuit(&self) -> &Circuit {
        &self.circuit
    }

    /// Returns the ERC message approvals.
    pub fn erc_approvals(&self) -> &BTreeSet<SExpression> {
        &self.erc_approvals
    }

    /// Returns the schematics in page order.
    pub fn schematics(&self) -> &[Schematic] {
        &self.schematics
    }

    /// Returns the schematics with their identifiers, in page order.
    pub fn schematics_with_ids(&self) -> impl Iterator<Item = (SchematicId, &Schematic)> {
        self.schematics.iter().map(|s| (s.id(), s))
    }

    /// Returns the page index of a schematic.
    pub fn schematic_index(&self, id: SchematicId) -> Option<usize> {
        self.schematics.iter().position(|s| s.uuid == id.0)
    }

    /// Returns the schematic with the given identifier.
    pub fn schematic(&self, id: SchematicId) -> Option<&Schematic> {
        self.schematics.iter().find(|s| s.uuid == id.0)
    }

    /// Returns the schematic with the given name.
    pub fn schematic_by_name(&self, name: &str) -> Option<&Schematic> {
        self.schematics.iter().find(|s| s.name().as_str() == name)
    }

    /// Returns the boards; the first one is the primary board.
    pub fn boards(&self) -> &[Board] {
        &self.boards
    }

    /// Returns the boards with their identifiers.
    pub fn boards_with_ids(&self) -> impl Iterator<Item = (BoardId, &Board)> {
        self.boards.iter().map(|b| (b.id(), b))
    }

    /// Returns the index of a board.
    pub fn board_index(&self, id: BoardId) -> Option<usize> {
        self.boards.iter().position(|b| b.uuid == id.0)
    }

    /// Returns the board with the given identifier.
    pub fn board(&self, id: BoardId) -> Option<&Board> {
        self.boards.iter().find(|b| b.uuid == id.0)
    }

    /// Returns the board with the given name.
    pub fn board_by_name(&self, name: &str) -> Option<&Board> {
        self.boards.iter().find(|b| b.name().as_str() == name)
    }

    /// Returns the primary board (the first one), if any.
    pub fn primary_board(&self) -> Option<&Board> {
        self.boards.first()
    }

    // --- Reverse index queries ---

    /// Returns all uses of a net signal.
    pub fn net_signal_uses(&self, id: NetSignalId) -> impl Iterator<Item = &NetUse> {
        self.refs.net_uses(id)
    }

    /// Whether a net signal is referenced by component signals, segments
    /// or planes (upstream `NetSignal::isUsed()`).
    pub fn is_net_signal_used(&self, id: NetSignalId) -> bool {
        self.refs.is_net_used(id)
    }

    /// Returns the net signal with the most uses (upstream
    /// `getNetSignalWithMostElements()`); ties are resolved by UUID order.
    pub fn net_signal_with_most_elements(&self) -> Option<NetSignalId> {
        self.circuit
            .net_signals
            .keys()
            .max_by_key(|id| (self.refs.net_use_count(**id), std::cmp::Reverse(**id)))
            .copied()
    }

    /// Returns the bus segments of a bus (upstream
    /// `Bus::getSchematicBusSegments()`).
    pub fn bus_uses(&self, id: BusId) -> impl Iterator<Item = &(SchematicId, BusSegmentId)> {
        self.refs.bus_uses(id)
    }

    /// Returns the symbols and devices of a component instance.
    pub fn component_uses(&self, id: ComponentInstanceId) -> Option<&ComponentUses> {
        self.refs.component_uses(id)
    }

    /// Whether a component instance has symbols or devices (upstream
    /// `ComponentInstance::isUsed()`).
    pub fn is_component_used(&self, id: ComponentInstanceId) -> bool {
        self.refs.component_uses(id).is_some_and(|u| u.count() > 0)
    }

    /// Rebuilds the reverse index from scratch and compares it with the
    /// incrementally maintained one (consistency check for tests and debug
    /// assertions; linear in the project size).
    pub fn is_ref_index_consistent(&self) -> bool {
        RefIndex::build(self) == self.refs
            && self
                .schematics
                .iter()
                .all(|s| s.is_anchor_index_consistent(self.view()))
            && self
                .boards
                .iter()
                .all(|b| b.is_pad_index_consistent(&self.library))
    }

    /// Returns the read-only context for item computations.
    pub fn view(&self) -> ProjectView<'_> {
        ProjectView {
            library: &self.library,
            circuit: &self.circuit,
        }
    }

    // --- Change notification ---

    /// Returns the revision: the sequence number of the last change (0
    /// right after opening).
    pub fn revision(&self) -> u64 {
        self.journal.revision()
    }

    /// Returns the changes after `revision`, oldest first, or
    /// [`ChangesSince::Resync`] if the journal no longer holds them.
    pub fn changes_since(&self, revision: u64) -> ChangesSince<'_> {
        self.journal.changes_since(revision)
    }

    pub(crate) fn record(&mut self, change: Change) {
        self.journal.push(change);
    }

    // --- Saving ---

    /// Writes all project files into the transactional file system
    /// (upstream `Project::save()`); call
    /// [`TransactionalFileSystem::save()`](crate::fileio::TransactionalFileSystem::save)
    /// afterwards to commit them to disk.
    pub fn save(&mut self) -> Result<()> {
        let dir = &mut self.directory;
        dir.write(
            ".librepcb-project",
            &VersionFile::new(application::file_format_version()).to_bytes(),
        )?;
        dir.write(&self.file_name, PROJECT_FILE_CONTENT)?;
        {
            let mut root = List::new("librepcb_project_metadata");
            root.append_value(&self.uuid);
            root.ensure_line_break();
            root.append_child("name", &self.metadata.name);
            root.ensure_line_break();
            root.append_child("author", &self.metadata.author);
            root.ensure_line_break();
            root.append_child("version", &self.metadata.version);
            root.ensure_line_break();
            root.append_child("created", &self.metadata.created);
            root.ensure_line_break();
            self.metadata.attributes.serialize(&mut root);
            root.ensure_line_break();
            dir.write("project/metadata.lp", &to_bytes(root)?)?;
        }
        {
            let mut root = List::new("librepcb_project_settings");
            root.ensure_line_break();
            {
                let node = root.append_list("library_locale_order");
                for locale in &self.settings.locale_order {
                    node.ensure_line_break();
                    node.append_child("locale", locale);
                }
                node.ensure_line_break();
            }
            root.ensure_line_break();
            {
                let node = root.append_list("library_norm_order");
                for norm in &self.settings.norm_order {
                    node.ensure_line_break();
                    node.append_child("norm", norm);
                }
                node.ensure_line_break();
            }
            root.ensure_line_break();
            {
                let node = root.append_list("custom_bom_attributes");
                for key in &self.settings.custom_bom_attributes {
                    node.ensure_line_break();
                    node.append_child("attribute", key);
                }
                node.ensure_line_break();
            }
            root.ensure_line_break();
            root.append_child(
                "default_lock_component_assembly",
                &self.settings.default_lock_component_assembly,
            );
            root.ensure_line_break();
            dir.write("project/settings.lp", &to_bytes(root)?)?;
        }
        {
            let mut root = List::new("librepcb_project_user_settings");
            root.ensure_line_break();
            dir.write("project/settings.user.lp", &to_bytes(root)?)?;
        }
        {
            let mut root = List::new("librepcb_jobs");
            self.output_jobs.serialize(&mut root);
            dir.write("project/jobs.lp", &to_bytes(root)?)?;
        }
        {
            let mut root = List::new("librepcb_circuit");
            self.circuit.serialize(&mut root);
            dir.write("circuit/circuit.lp", &to_bytes(root)?)?;
        }
        {
            let mut root = List::new("librepcb_erc");
            for node in &self.erc_approvals {
                root.ensure_line_break();
                root.push(node.clone());
            }
            root.ensure_line_break();
            dir.write("circuit/erc.lp", &to_bytes(root)?)?;
        }
        {
            let mut root = List::new("librepcb_schematics");
            for schematic in &self.schematics {
                root.ensure_line_break();
                let path = format!("schematics/{}", schematic.directory_name());
                root.append_child("schematic", &format!("{path}/schematic.lp"));
                schematic.save(&mut dir.subdir(&path))?;
            }
            root.ensure_line_break();
            dir.write("schematics/schematics.lp", &to_bytes(root)?)?;
        }
        {
            let mut root = List::new("librepcb_boards");
            for board in &self.boards {
                root.ensure_line_break();
                let path = format!("boards/{}", board.directory_name());
                root.append_child("board", &format!("{path}/board.lp"));
                board.save(&mut dir.subdir(&path))?;
            }
            root.ensure_line_break();
            dir.write("boards/boards.lp", &to_bytes(root)?)?;
        }
        self.date_time = Utc::now();
        Ok(())
    }

    /// Whether `dir` contains a project (upstream `isProjectDirectory()`).
    pub fn is_project_directory(dir: &FilePath) -> bool {
        dir.path_to(".librepcb-project").is_existing_file()
    }

    /// Whether `file` is a project file in a project directory (upstream
    /// `isProjectFile()`).
    pub fn is_project_file(file: &FilePath) -> bool {
        file.suffix() == "lpp"
            && file.is_existing_file()
            && file
                .parent_dir()
                .is_some_and(|d| Self::is_project_directory(&d))
    }

    /// Whether `path` is located inside a project directory (upstream
    /// `isFilePathInsideProjectDirectory()`).
    pub fn is_file_path_inside_project_directory(path: &FilePath) -> bool {
        let mut parent = path.parent_dir();
        while let Some(dir) = parent {
            if Self::is_project_directory(&dir) {
                return true;
            }
            if dir.is_root() {
                return false;
            }
            parent = dir.parent_dir();
        }
        false
    }
}

fn to_bytes(root: List) -> crate::serialization::Result<Vec<u8>> {
    SExpression::from(root).to_byte_array(Mode::LibrePcb)
}
