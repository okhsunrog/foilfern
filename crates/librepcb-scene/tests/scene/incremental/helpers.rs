//! Shared helpers: opening projects, canonical scene dumps and the
//! [`Harness`] comparing incrementally updated scenes with fresh builds.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::{BoardId, Project, ProjectLoader, SchematicId};
use librepcb_core::types::{Length, Point};
use librepcb_editor::{DirectoryLibrarySource, ProjectEditor};
use librepcb_scene::librepcb_canvas::Scene;
use librepcb_scene::{BoardScene, BoardSide, ColorScheme, SceneSync, SchematicScene};

/// Upstream test data directory.
pub fn test_data_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

/// All upstream test projects.
pub fn projects() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(test_data_dir().join("projects"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join(".librepcb-project").exists())
        .collect();
    dirs.sort();
    dirs
}

/// Opens an upstream test project read-only (upgrading older formats in
/// memory).
pub fn open_upstream(dir: &Path) -> Project {
    let lpp = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".lpp"))
        .expect("*.lpp file");
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).unwrap()).unwrap();
    let directory = TransactionalDirectory::new(Arc::new(fs), "");
    ProjectLoader::new()
        .open(directory, &lpp)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
}

/// UUIDs of elements of the upstream "Populated Library".
pub mod lib {
    use librepcb_core::types::Uuid;

    fn uuid(s: &str) -> Uuid {
        s.parse().expect("valid UUID")
    }

    /// Component "Resistor".
    pub fn resistor() -> Uuid {
        uuid("ef80cd5e-2689-47ee-8888-31d04fc99174")
    }
    /// Device "R-0805".
    pub fn r0805() -> Uuid {
        uuid("078650d3-483c-4b9e-a848-b14f1aad2edc")
    }
    /// Device "R-0603".
    pub fn r0603() -> Uuid {
        uuid("483a71eb-318e-448e-82ff-f02efc4821aa")
    }
    /// Component "Capacitor Bipolar".
    pub fn capacitor() -> Uuid {
        uuid("d167e0e3-6a92-4b76-b013-77b9c230e5f1")
    }
    /// Device "C-0805".
    pub fn c0805() -> Uuid {
        uuid("c139e505-592b-46ba-bdf2-acb7383ea0cd")
    }
}

/// A library element source over the upstream "Populated Library".
pub fn library_source() -> DirectoryLibrarySource {
    let dir = FilePath::new(test_data_dir().join("libraries/Populated Library.lplib"))
        .expect("absolute path");
    DirectoryLibrarySource::from_libraries([&dir]).expect("library scan")
}

/// Creates an editor for a new project in `dir`.
pub fn create_editor(dir: &Path) -> ProjectEditor {
    let fs = TransactionalFileSystem::open_rw(&FilePath::new(dir).expect("absolute path"))
        .expect("open file system");
    let directory = TransactionalDirectory::new(Arc::new(fs), "");
    let fonts =
        FilePath::new(Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("share/librepcb/fontobene"))
            .expect("absolute path");
    let project = librepcb_editor::create_project(directory, "test.lpp", Some(&fonts))
        .expect("create project");
    ProjectEditor::with_source(project, Arc::new(library_source()))
}

/// An editor for an upstream test project (edited in memory).
pub fn upstream_editor(dir: &Path) -> ProjectEditor {
    ProjectEditor::with_source(open_upstream(dir), Arc::new(library_source()))
}

/// A point in millimeters.
pub fn mm(x: f64, y: f64) -> Point {
    Point::new(Length::from_mm(x).unwrap(), Length::from_mm(y).unwrap())
}

/// Canonical dump of a scene: one line per item (model object, layer, z,
/// style, geometry), sorted, then the layers.
fn dump_scene<O: std::fmt::Debug>(
    scene: &Scene,
    object: impl Fn(librepcb_scene::librepcb_canvas::ItemId) -> Option<O>,
) -> Vec<String> {
    let mut lines: Vec<String> = scene
        .items()
        .map(|(id, item)| {
            format!(
                "{:?} | layer {:?} z {} | {:?} | {:?}",
                object(id),
                item.layer,
                item.z,
                item.style,
                item.geometry
            )
        })
        .collect();
    lines.sort();
    let mut layers: Vec<String> = scene
        .layers()
        .map(|(id, l)| format!("layer {id:?}: {l:?}"))
        .collect();
    layers.sort();
    lines.extend(layers);
    lines
}

pub fn dump_schematic(scene: &SchematicScene) -> Vec<String> {
    let mut lines = dump_scene(scene.scene(), |id| scene.object(id));
    lines.extend(scene.warnings().iter().map(|w| format!("warning: {w}")));
    lines
}

pub fn dump_board(scene: &BoardScene) -> Vec<String> {
    let mut lines = dump_scene(scene.scene(), |id| scene.object(id));
    lines.extend(scene.warnings().iter().map(|w| format!("warning: {w}")));
    lines
}

/// Asserts that two dumps are equal, printing the differing lines.
#[track_caller]
pub fn assert_same(what: &str, incremental: &[String], fresh: &[String]) {
    if incremental == fresh {
        return;
    }
    let mut a: Vec<&String> = incremental.iter().collect();
    let mut b: Vec<&String> = fresh.iter().collect();
    a.sort();
    b.sort();
    let only_a: Vec<&&String> = a.iter().filter(|l| b.binary_search(l).is_err()).collect();
    let only_b: Vec<&&String> = b.iter().filter(|l| a.binary_search(l).is_err()).collect();
    let cut = |l: &str| l.chars().take(300).collect::<String>();
    let mut msg = format!(
        "{what}: incremental scene differs from a fresh build ({} vs {} lines)\n",
        incremental.len(),
        fresh.len()
    );
    for l in only_a.iter().take(10) {
        msg += &format!("  only incremental: {}\n", cut(l));
    }
    for l in only_b.iter().take(10) {
        msg += &format!("  only fresh:       {}\n", cut(l));
    }
    panic!("{msg}");
}

/// Statistics of a [`Harness`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// Steps checked.
    pub steps: usize,
    /// Incremental updates (per scene).
    pub incremental: usize,
    /// Full rebuilds (per scene).
    pub rebuilds: usize,
}

/// Keeps scenes of all schematics and boards of an editor's
/// project up to date with [`SceneSync`] and compares them with fresh
/// builds after every step.
pub struct Harness {
    pub editor: ProjectEditor,
    schematics: Vec<(SchematicId, SchematicScene, SceneSync)>,
    boards: Vec<(BoardId, BoardSide, BoardScene, SceneSync)>,
    pub stats: Stats,
    /// Recalculate the planes after every n-th step (0: never; slow in
    /// debug builds).
    pub plane_interval: usize,
}

impl Harness {
    pub fn new(editor: ProjectEditor) -> Self {
        let mut h = Self {
            editor,
            schematics: Vec::new(),
            boards: Vec::new(),
            stats: Stats::default(),
            plane_interval: 4,
        };
        h.update_derived();
        h.add_new_scenes();
        h
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    /// Updates air wires and planes like the editor would after a command.
    fn update_derived(&mut self) {
        let boards: Vec<BoardId> = self.project().boards().iter().map(|b| b.id()).collect();
        let planes =
            self.plane_interval > 0 && self.stats.steps.is_multiple_of(self.plane_interval);
        self.editor
            .update_derived_data(|p| {
                for b in &boards {
                    p.rebuild_air_wires(*b).unwrap();
                    if planes {
                        p.rebuild_planes(*b, None).unwrap();
                        p.rebuild_air_wires(*b).unwrap();
                    }
                }
            })
            .unwrap();
    }

    /// Builds scenes for schematics and boards which have none yet.
    fn add_new_scenes(&mut self) {
        let project = self.editor.project();
        for sch in project.schematics() {
            if !self.schematics.iter().any(|(id, ..)| *id == sch.id()) {
                let scene = SchematicScene::build(project, sch.id(), &ColorScheme::SCHEMATIC_LIGHT)
                    .unwrap();
                self.schematics
                    .push((sch.id(), scene, SceneSync::new(project)));
            }
        }
        for (index, board) in project.boards().iter().enumerate() {
            // Both sides of the first board, the top of the others.
            let sides: &[BoardSide] = if index == 0 {
                &[BoardSide::Top, BoardSide::Bottom]
            } else {
                &[BoardSide::Top]
            };
            for &side in sides {
                if !self
                    .boards
                    .iter()
                    .any(|(id, s, ..)| *id == board.id() && *s == side)
                {
                    let scene =
                        BoardScene::build(project, board.id(), side, &ColorScheme::BOARD_DARK)
                            .unwrap();
                    self.boards
                        .push((board.id(), side, scene, SceneSync::new(project)));
                }
            }
        }
    }

    /// Syncs all scenes and compares them with fresh builds; returns the
    /// number of full rebuilds.
    #[track_caller]
    pub fn check(&mut self, step: &str) -> usize {
        self.update_derived();
        let project = self.editor.project();
        let mut rebuilds = 0;
        self.schematics
            .retain(|(id, ..)| project.schematic(*id).is_some());
        self.boards.retain(|(id, ..)| project.board(*id).is_some());
        for (id, scene, sync) in &mut self.schematics {
            if sync.sync(project, scene).unwrap() {
                rebuilds += 1;
            } else {
                self.stats.incremental += 1;
            }
            let fresh = SchematicScene::build(project, *id, &ColorScheme::SCHEMATIC_LIGHT).unwrap();
            assert_same(
                &format!("{step}: schematic {}", id.0),
                &dump_schematic(scene),
                &dump_schematic(&fresh),
            );
        }
        for (id, side, scene, sync) in &mut self.boards {
            if sync.sync(project, scene).unwrap() {
                rebuilds += 1;
            } else {
                self.stats.incremental += 1;
            }
            let fresh = BoardScene::build(project, *id, *side, &ColorScheme::BOARD_DARK).unwrap();
            assert_same(
                &format!("{step}: board {} {side:?}", id.0),
                &dump_board(scene),
                &dump_board(&fresh),
            );
        }
        self.stats.rebuilds += rebuilds;
        self.stats.steps += 1;
        self.add_new_scenes();
        rebuilds
    }

    /// Runs a command and checks the scenes; panics if it fails.
    #[track_caller]
    pub fn run<C: librepcb_editor::Command + std::fmt::Debug>(&mut self, command: C) -> C::Output {
        let text = describe(&command);
        let output = self
            .editor
            .execute(command)
            .unwrap_or_else(|e| panic!("{text}: {e}"));
        self.check(&text);
        output
    }

    /// Runs a command which may fail and checks the scenes; returns
    /// whether it succeeded.
    #[track_caller]
    pub fn try_run<C: librepcb_editor::Command + std::fmt::Debug>(&mut self, command: C) -> bool {
        let text = describe(&command);
        let ok = self.editor.execute(command).is_ok();
        self.check(&text);
        ok
    }

    pub fn undo(&mut self) -> bool {
        let done = self.editor.undo().unwrap();
        self.check("undo");
        done
    }

    pub fn redo(&mut self) -> bool {
        let done = self.editor.redo().unwrap();
        self.check("redo");
        done
    }
}

/// A short description of a command for failure messages.
fn describe(command: &impl std::fmt::Debug) -> String {
    format!("{command:?}").chars().take(160).collect()
}
