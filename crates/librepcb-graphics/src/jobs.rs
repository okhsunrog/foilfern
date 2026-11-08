//! Graphics output jobs from libs/librepcb/core/project/outputjobrunner.cpp.
//! Generates encoded files; the caller's indexed writer owns all disk writes.
use crate::{
    Error, GraphicsExport, GraphicsFile, GraphicsPage, GraphicsSettings, Result, board_drawing,
    realistic_board_drawing, schematic_drawing,
};
use librepcb_core::{
    fileio::{CleanFileNameOptions, FilePath},
    job::{GraphicsContentType, GraphicsJob},
    project::{
        Project, ProjectAttributeLookup, board::BoardPhysicalGeometry, select_job_boards,
        select_job_variants,
    },
};
use std::collections::{BTreeMap, BTreeSet};
fn single<T: Copy + Ord>(set: &BTreeSet<Option<T>>) -> Option<T> {
    if set.len() == 1 {
        set.first().copied().flatten()
    } else {
        None
    }
}

/// Encodes every configured page. Board/variant contexts affect filenames and
/// document titles; like upstream, they do not filter schematic components.
pub fn graphics_job_files(project: &Project, job: &GraphicsJob) -> Result<Vec<GraphicsFile>> {
    let mut pages = Vec::new();
    let mut physical = BTreeMap::new();
    let mut all_boards = BTreeSet::new();
    let mut all_variants = BTreeSet::new();
    for content in &job.content {
        let settings = GraphicsSettings::from_content(content);
        let boards = select_job_boards(project, &content.boards)?;
        let variants = select_job_variants(project, &content.variants)?;
        all_boards.extend(boards.iter().copied());
        all_variants.extend(variants.iter().copied());
        match content.kind {
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
        ProjectAttributeLookup::board(project, board, variant)
    } else {
        ProjectAttributeLookup::project(project, variant)
    };
    let mut error = None;
    let output = librepcb_core::attribute::substitute(
        &job.output,
        |key| match lookup.value(key) {
            Ok(value) => value,
            Err(e) => {
                error = Some(e);
                None
            }
        },
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
    if let Some(error) = error {
        return Err(error.into());
    }
    let title = if job.title.as_str().is_empty() {
        "{{PROJECT}} {{VERSION}}"
    } else {
        job.title.as_str()
    };
    let exporter = GraphicsExport {
        title: lookup
            .substitute(title)?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        ..Default::default()
    };
    exporter.generate(&pages, &output)
}
