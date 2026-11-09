//! Graphics output jobs from libs/librepcb/core/project/outputjobrunner.cpp.
//! Generates encoded files; the caller's indexed writer owns all disk writes.
use crate::{
    Error, GraphicsExport, GraphicsFile, GraphicsPage, GraphicsSettings, Result, board_drawing,
    realistic_board_drawing, schematic_drawing,
};
use librepcb_core::{
    fileio::{CleanFileNameOptions, FilePath},
    job::{GraphicsContentType, GraphicsOutputJob},
    project::{
        Project, ProjectAttributeLookup, board::BoardPhysicalGeometry, select_job_boards,
        select_job_variants,
    },
};
use std::collections::{BTreeMap, BTreeSet};

/// The shared output-job adapter used by CLI, MCP and the desktop application.
pub struct ProjectGraphicsExporter {
    creator: String,
}
impl ProjectGraphicsExporter {
    pub fn new(creator: String) -> Self {
        Self { creator }
    }
}
impl ProjectGraphicsExporter {
    fn encoded(
        &self,
        project: &Project,
        pages: &[librepcb_core::project::GraphicsPage],
        file_path: &FilePath,
        document_name: &str,
    ) -> Result<Vec<librepcb_core::project::GraphicsJobFile>> {
        use librepcb_core::project::{GraphicsJobFile, GraphicsPageContent};
        let pages: Vec<_> = pages
            .iter()
            .map(|page| {
                let settings = GraphicsSettings::from_export_settings(&page.settings);
                let drawing = match page.content {
                    GraphicsPageContent::Schematic(id) => schematic_drawing(
                        project,
                        project
                            .schematic(id)
                            .ok_or_else(|| Error::Model(format!("Missing schematic {id}")))?,
                        &settings,
                    )?,
                    GraphicsPageContent::Board(id) => board_drawing(
                        project,
                        project
                            .board(id)
                            .ok_or_else(|| Error::Model(format!("Missing board {id}")))?,
                        &settings,
                    )?,
                    GraphicsPageContent::BoardRendering(id) => {
                        let board = project
                            .board(id)
                            .ok_or_else(|| Error::Model(format!("Missing board {id}")))?;
                        realistic_board_drawing(
                            &BoardPhysicalGeometry::from_project(project, board, None)?,
                            &settings,
                        )
                    }
                };
                Ok(GraphicsPage { drawing, settings })
            })
            .collect::<Result<_>>()?;
        let exporter = GraphicsExport {
            title: document_name.to_owned(),
            creator: self.creator.clone(),
            ..Default::default()
        };
        let files = exporter.generate(&pages, file_path.as_str())?;
        files
            .into_iter()
            .map(|file| {
                Ok(GraphicsJobFile {
                    path: FilePath::new(file.name).ok_or(Error::Dimensions)?,
                    contents: file.contents,
                })
            })
            .collect()
    }
}
impl librepcb_core::project::GraphicsExporter for ProjectGraphicsExporter {
    fn prepare(
        &self,
        project: &Project,
        pages: &[librepcb_core::project::GraphicsPage],
        file_path: &FilePath,
        document_name: &str,
    ) -> Option<librepcb_core::project::OutputJobResult<Vec<librepcb_core::project::GraphicsJobFile>>>
    {
        Some(
            self.encoded(project, pages, file_path, document_name)
                .map_err(|error| {
                    librepcb_core::project::OutputJobError::Graphics(error.to_string())
                }),
        )
    }
    fn export(
        &self,
        project: &Project,
        pages: &[librepcb_core::project::GraphicsPage],
        file_path: &FilePath,
        document_name: &str,
    ) -> librepcb_core::project::GraphicsExportResult {
        use librepcb_core::{fileio::file_utils, project::GraphicsExportResult};
        let result = (|| -> Result<Vec<FilePath>> {
            let files = self.encoded(project, pages, file_path, document_name)?;
            let mut written = Vec::new();
            for file in files {
                file_utils::write_file(&file.path, &file.contents)?;
                written.push(file.path);
            }
            Ok(written)
        })();
        match result {
            Ok(written_files) => GraphicsExportResult {
                written_files,
                errors: Vec::new(),
            },
            Err(error) => GraphicsExportResult {
                errors: vec![error.to_string()],
                ..Default::default()
            },
        }
    }
}

fn single<T: Copy + Ord>(set: &BTreeSet<Option<T>>) -> Option<T> {
    if set.len() == 1 {
        set.first().copied().flatten()
    } else {
        None
    }
}

/// Encodes every configured page. Board/variant contexts affect filenames and
/// document titles; like upstream, they do not filter schematic components.
pub fn graphics_job_files(project: &Project, job: &GraphicsOutputJob) -> Result<Vec<GraphicsFile>> {
    let mut pages = Vec::new();
    let mut physical = BTreeMap::new();
    let mut all_boards = BTreeSet::new();
    let mut all_variants = BTreeSet::new();
    for content in &job.content {
        let settings = GraphicsSettings::from_content(content);
        let boards = select_job_boards(project, &content.boards)?;
        let variants = select_job_variants(project, &content.assembly_variants)?;
        all_boards.extend(boards.iter().copied());
        all_variants.extend(variants.iter().copied());
        match content.content_type {
            GraphicsContentType::Schematic => {
                for _ in &variants {
                    for _ in &boards {
                        for schematic in project.schematics() {
                            pages.push(GraphicsPage {
                                drawing: schematic_drawing(project, schematic, &settings)?,
                                settings: settings.clone(),
                            });
                        }
                    }
                }
            }
            GraphicsContentType::Board => {
                for id in boards {
                    let board = id.and_then(|id| project.board(id)).ok_or_else(|| {
                        Error::Model("Board graphics require a board selection".into())
                    })?;
                    for _ in &variants {
                        pages.push(GraphicsPage {
                            drawing: board_drawing(project, board, &settings)?,
                            settings: settings.clone(),
                        });
                    }
                }
            }
            GraphicsContentType::AssemblyGuide => {
                return Err(Error::UnsupportedContent("assembly_guide".into()));
            }
            GraphicsContentType::BoardRendering => {
                for id in boards {
                    let board = id.and_then(|id| project.board(id)).ok_or_else(|| {
                        Error::Model("Board rendering requires a board selection".into())
                    })?;
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        physical.entry(board.id())
                    {
                        entry.insert(BoardPhysicalGeometry::from_project(project, board, None)?);
                    }
                    for _ in &variants {
                        pages.push(GraphicsPage {
                            drawing: realistic_board_drawing(&physical[&board.id()], &settings),
                            settings: settings.clone(),
                        });
                    }
                }
            }
        }
    }
    let board = single(&all_boards).and_then(|id| project.board(id));
    let variant = single(&all_variants).and_then(|id| project.circuit().assembly_variant(id));
    let lookup = if let Some(board) = board {
        ProjectAttributeLookup::for_board(project, board, variant)
    } else {
        ProjectAttributeLookup::for_project(project, variant)
    };
    let output = librepcb_core::attribute::substitute(
        &job.output_path,
        |key| lookup.value(key),
        Some(&mut |value| {
            FilePath::clean_file_name(
                value,
                CleanFileNameOptions {
                    replace_spaces: true,
                    ..Default::default()
                },
                120,
            )
        }),
    );
    let title = if job.document_title.as_str().is_empty() {
        "{{PROJECT}} {{VERSION}}"
    } else {
        job.document_title.as_str()
    };
    let exporter = GraphicsExport {
        title: lookup
            .substitute(title)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        ..Default::default()
    };
    exporter.generate(&pages, &output)
}
