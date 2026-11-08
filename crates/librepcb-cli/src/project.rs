//! Project command from apps/librepcb-cli/commandlineinterface.cpp.
use crate::{
    ProjectOptions,
    error::{Error, Result},
    support::*,
};
use librepcb_core::project::board::drc::{BoardDesignRuleCheck, BoardDrcData};
use librepcb_core::{
    export::{BoardSide, BomCsvWriter, PickPlaceCsvWriter, PickPlaceSides, Timestamp},
    fileio::{CleanFileNameOptions, FilePath, OutputDirectoryEvent, file_utils},
    job::OutputJobList,
    project::{
        AssemblyVariantId, BoardId, BomGenerator, Mutation, OutputJobEvent, OutputJobRunner,
        Project, ProjectAttributeLookup, ProjectLoader,
        board::{
            BoardD356NetlistExport, BoardFabricationOutputSettings, BoardGerberExport,
            BoardPickPlaceGenerator,
        },
        erc::ElectricalRuleCheck,
    },
    serialization::DeserializeObject,
};
use librepcb_i18n::tr;
use std::{collections::BTreeMap, path::Path};
const VERSION: &str = env!("CARGO_PKG_VERSION");
fn selected<I: Copy + Eq>(
    names: &[String],
    indices: &[String],
    items: &[(I, String)],
    kind: &str,
) -> (Vec<I>, bool) {
    let mut result = Vec::new();
    let mut success = true;
    for name in names {
        if let Some((id, _)) = items.iter().find(|(_, n)| n == name) {
            if !result.contains(id) {
                result.push(*id);
            }
        } else {
            eprintln!(
                "{}",
                if kind == "board" {
                    tr!(
                        "CommandLineInterface",
                        "ERROR: No board with the name '{0}' found.",
                        name
                    )
                } else {
                    tr!(
                        "CommandLineInterface",
                        "ERROR: No assembly variant with the name '{0}' found.",
                        name
                    )
                }
            );
            success = false;
        }
    }
    for index in indices {
        if let Some((id, _)) = index
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|i| items.get(i))
        {
            if !result.contains(id) {
                result.push(*id);
            }
        } else {
            let title = if kind == "board" {
                tr!(
                    "CommandLineInterface",
                    "ERROR: Board index '{0}' is invalid.",
                    index
                )
            } else {
                tr!(
                    "CommandLineInterface",
                    "ERROR: Assembly variant index '{0}' is invalid.",
                    index
                )
            };
            eprintln!("{title}");
            success = false;
        }
    }
    if names.is_empty() && indices.is_empty() {
        result = items.iter().map(|(id, _)| *id).collect();
    }
    (result, success)
}
fn lookup<'a>(
    project: &'a Project,
    board: Option<BoardId>,
    variant: Option<AssemblyVariantId>,
) -> ProjectAttributeLookup<'a> {
    let variant = variant.and_then(|v| project.circuit().assembly_variant(v));
    if let Some(b) = board.and_then(|b| project.board(b)) {
        ProjectAttributeLookup::board(project, b, variant)
    } else {
        ProjectAttributeLookup::project(project, variant)
    }
}
fn substitute(
    project: &Project,
    template: &str,
    board: Option<BoardId>,
    variant: Option<AssemblyVariantId>,
) -> Result<String> {
    let lookup = lookup(project, board, variant);
    let mut error = None;
    let value = librepcb_core::attribute::substitute(
        template,
        |key| match lookup.value(key) {
            Ok(v) => v,
            Err(e) => {
                error = Some(e);
                None
            }
        },
        Some(&mut |s| {
            FilePath::clean_file_name(
                s,
                CleanFileNameOptions {
                    replace_spaces: true,
                    ..Default::default()
                },
                120,
            )
        }),
    );
    if let Some(e) = error {
        return Err(e.into());
    }
    Ok(value)
}
fn write(path: &str, data: &[u8], counts: &mut BTreeMap<FilePath, usize>) -> Result<()> {
    let fp = crate::support::path(path)?;
    file_utils::write_file(&fp, data)?;
    *counts.entry(fp).or_default() += 1;
    Ok(())
}
fn extension(value: &str) -> String {
    Path::new(value)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase()
}
fn board_name(project: &Project, board: BoardId) -> Result<String> {
    project
        .board(board)
        .map(|b| b.name().to_string())
        .ok_or_else(|| {
            Error::Message(tr!(
                "CommandLineInterface",
                "Board does not exist: {0}",
                board
            ))
        })
}
pub fn run(options: ProjectOptions) -> Result<bool> {
    println!(
        "{}",
        tr!(
            "CommandLineInterface",
            "Open project '{0}'...",
            pretty(&path(&options.project)?, &options.project)
        )
    );
    let fp = path(&options.project)?;
    let zipped = fp.suffix() == "lppz";
    let parent = fp.parent_dir().ok_or_else(|| {
        Error::Message(tr!(
            "CommandLineInterface",
            "Invalid project path: '{0}'",
            options.project
        ))
    })?;
    let directory = open_directory(&parent, options.save && !zipped)?;
    let fs = directory.file_system().clone();
    let filename = if zipped {
        fs.remove_dir_recursively("")?;
        fs.load_from_zip(&fp)?;
        fs.files("")
            .into_iter()
            .rfind(|f| f.ends_with(".lpp"))
            .unwrap_or_default()
    } else {
        fp.file_name().to_owned()
    };
    let mut loader = ProjectLoader::new();
    loader.set_application_version(VERSION);
    let mut project = loader.open(directory, &filename)?;
    if let Some(log) = loader.migration_log() {
        println!(
            "{}",
            tr!(
                "CommandLineInterface",
                "Attention: Project has been migrated to a newer file format!"
            )
        );
        for msg in &log.messages {
            let multiplier = if msg.affected_items.is_some_and(|n| n > 0) {
                format!(" ({}x)", msg.affected_items.unwrap_or_default())
            } else {
                String::new()
            };
            println!(
                " - {}{multiplier}: {}",
                msg.severity.to_tr_string(),
                msg.message
            );
        }
    }
    let mut success = true;
    if let Some(name) = &options.set_default_variant {
        println!(
            "{}",
            tr!(
                "CommandLineInterface",
                "Set default assembly variant to '{0}'...",
                name
            )
        );
        if let Some(variant) = project.circuit().assembly_variants().by_name(name, true) {
            project.apply(Mutation::MoveAssemblyVariant {
                variant: AssemblyVariantId(variant.uuid()),
                index: 0,
            })?;
        } else {
            eprintln!(
                "{}",
                tr!(
                    "CommandLineInterface",
                    "ERROR: No assembly variant with the name '{0}' found.",
                    name
                )
            );
            success = false;
        }
    }
    let (variants, ok) = selected(
        &options.variant,
        &options.variant_index,
        &project
            .circuit()
            .assembly_variants()
            .iter()
            .map(|v| (AssemblyVariantId(v.uuid()), v.name().to_string()))
            .collect::<Vec<_>>(),
        "assembly variant",
    );
    success &= ok;
    let (mut boards, ok) = selected(
        &options.board,
        &options.board_index,
        &project
            .boards()
            .iter()
            .map(|b| (b.id(), b.name().to_string()))
            .collect::<Vec<_>>(),
        "board",
    );
    success &= ok;
    if options.remove_other_boards {
        println!("{}", tr!("CommandLineInterface", "Remove other boards..."));
        let keep = if options.board.is_empty() && options.board_index.is_empty() {
            vec![]
        } else {
            boards.clone()
        };
        let remove: Vec<_> = project
            .boards()
            .iter()
            .filter(|b| !keep.contains(&b.id()))
            .map(|b| (b.id(), b.name().to_string()))
            .collect();
        for (id, name) in remove {
            println!("  - '{name}'");
            project.remove_board(id)?;
        }
        boards = keep;
    }
    if options.strict {
        println!(
            "{}",
            tr!("CommandLineInterface", "Check for non-canonical files...")
        );
        if zipped {
            eprintln!(
                "  {}",
                tr!(
                    "CommandLineInterface",
                    "ERROR: The option '--strict' is not available for *.lppz files!"
                )
            );
            success = false;
        } else {
            project.save()?;
            for name in fs
                .check_for_modifications()?
                .into_iter()
                .filter(|n| !n.contains(".user.lp"))
            {
                eprintln!(
                    "    - {}",
                    tr!(
                        "CommandLineInterface",
                        "Non-canonical file: '{0}'",
                        pretty(&parent.path_to(&name), &options.project)
                    )
                );
                success = false;
            }
        }
    }
    if options.erc {
        println!("{}", tr!("CommandLineInterface", "Run ERC..."));
        let messages = ElectricalRuleCheck::new(&project).run_checks()?;
        let (approved, remaining) = findings(
            messages.iter().map(|m| m.rule_message().clone()),
            project.erc_approvals(),
        );
        summary("  ", approved, remaining.len());
        for message in remaining {
            eprintln!("    - {message}");
            success = false;
        }
    }
    if options.drc {
        println!("{}", tr!("CommandLineInterface", "Run DRC..."));
        let custom = options
            .drc_settings
            .as_deref()
            .map(parse_file)
            .transpose()
            .and_then(|n| {
                n.map(|n| {
                    librepcb_core::library::org::BoardDesignRuleCheckSettings::deserialize(&n)
                        .map_err(Error::from)
                })
                .transpose()
            });
        match custom {
            Err(e) => {
                eprintln!(
                    "{}",
                    tr!(
                        "CommandLineInterface",
                        "ERROR: Failed to load custom settings: {0}",
                        e
                    )
                );
                success = false;
            }
            Ok(custom) => {
                for id in &boards {
                    let board = project.board(*id).ok_or_else(|| {
                        Error::Message(tr!("CommandLineInterface", "Board does not exist: {0}", id))
                    })?;
                    println!(
                        "  {}",
                        tr!("CommandLineInterface", "Board '{0}':", board.name())
                    );
                    let settings = custom.as_ref().unwrap_or(&board.settings().drc_settings);
                    match BoardDrcData::extract(&project, board, settings)
                        .map_err(Error::from)
                        .and_then(|d| BoardDesignRuleCheck::run(&d).map_err(Error::from))
                    {
                        Ok(result) => {
                            let (approved, remaining) = findings(
                                result.messages().iter().map(|m| m.rule_message().clone()),
                                board.drc_approvals(),
                            );
                            summary("    ", approved, remaining.len());
                            for message in remaining {
                                eprintln!("      - {message}");
                                success = false;
                            }
                        }
                        Err(e) => {
                            eprintln!("{}", tr!("CommandLineInterface", "FATAL ERROR: {0}", e));
                            success = false;
                        }
                    }
                }
            }
        }
    }
    let mut counts = BTreeMap::new();
    if options.run_jobs || !options.run_job.is_empty() {
        let jobs = options
            .jobs
            .as_deref()
            .map(|p| OutputJobList::deserialize(&parse_file(p)?).map_err(Error::from))
            .transpose();
        match jobs {
            Err(e) => {
                eprintln!(
                    "{}",
                    tr!(
                        "CommandLineInterface",
                        "ERROR: Failed to load custom output jobs: {0}",
                        e
                    )
                );
                success = false;
            }
            Ok(jobs) => {
                let all = jobs.as_ref().unwrap_or(project.output_jobs());
                let jobs = if options.run_jobs {
                    all.as_slice().to_vec()
                } else {
                    let mut selected = vec![];
                    for name in &options.run_job {
                        if let Some(job) = all.by_name(name, true) {
                            selected.push(job.clone());
                        } else {
                            eprintln!(
                                "{}",
                                tr!(
                                    "CommandLineInterface",
                                    "ERROR: No output job with the name '{0}' found.",
                                    name
                                )
                            );
                            success = false;
                        }
                    }
                    selected
                };
                let output = if let Some(output) = &options.outdir {
                    path(output)?
                } else {
                    parent.path_to(&substitute(&project, "output/{{VERSION}}", None, None)?)
                };
                let mut runner =
                    OutputJobRunner::new(&mut project, &output, VERSION, Timestamp::now());
                let style = options.project.clone();
                runner.set_file_observer(Some(Box::new(move |event| {
                    if let OutputDirectoryEvent::AboutToWriteFile(fp) = event {
                        println!("  => '{}'", pretty(fp, &style));
                    }
                })));
                if let Err(e) = runner.run_with_observer(&jobs, |event| match event {
                    OutputJobEvent::Started(id) => {
                        if let Some(job) = jobs.iter().find(|j| j.uuid() == id) {
                            println!(
                                "{}",
                                tr!(
                                    "CommandLineInterface",
                                    "Run output job '{0}'...",
                                    job.name()
                                )
                            );
                        }
                    }
                    OutputJobEvent::Warning(_) => {}
                }) {
                    eprintln!("{}", tr!("CommandLineInterface", "ERROR: {0}", e));
                    success = false;
                }
                for files in runner.written_files().values() {
                    for fp in files {
                        *counts.entry(fp.clone()).or_default() += 1;
                    }
                }
            }
        }
    }
    for template in &options.export_schematics {
        println!(
            "{}",
            tr!(
                "CommandLineInterface",
                "Export schematics to '{0}'...",
                template
            )
        );
        let dest = substitute(&project, template, None, None)?;
        let output = path(&dest)?;
        let settings = librepcb_graphics::GraphicsSettings::default();
        let pages = project
            .schematics()
            .iter()
            .map(|schematic| {
                Ok(librepcb_graphics::GraphicsPage {
                    drawing: librepcb_graphics::schematic_drawing(&project, schematic, &settings)?,
                    settings: settings.clone(),
                })
            })
            .collect::<librepcb_graphics::Result<Vec<_>>>();
        let exporter = librepcb_graphics::GraphicsExport {
            title: project.metadata().name.to_string(),
            creator: format!("LibrePCB {VERSION}"),
            ..Default::default()
        };
        match pages.and_then(|pages| exporter.write(&pages, &output)) {
            Ok(files) => {
                for file in files {
                    println!("  => '{}'", pretty(&file, &dest));
                    *counts.entry(file).or_default() += 1;
                }
            }
            Err(e) => {
                eprintln!("  {}", tr!("CommandLineInterface", "ERROR: {0}", e));
                success = false;
            }
        }
    }
    success &= legacy_exports(&mut project, &options, &boards, &variants, &mut counts)?;
    if options.save {
        println!("{}", tr!("CommandLineInterface", "Save project..."));
        project.save()?;
        if zipped {
            file_utils::write_file(&fp, &fs.export_to_zip(None)?)?;
        } else {
            fs.save()?;
        }
    }
    let mut overwritten = false;
    for (fp, count) in counts {
        if count > 1 {
            eprintln!(
                "{}",
                tr!(
                    "CommandLineInterface",
                    "ERROR: The file '{0}' was written multiple times!",
                    pretty(&fp, &options.project)
                )
            );
            overwritten = true;
        }
    }
    if overwritten {
        eprintln!(
            "{}",
            tr!(
                "CommandLineInterface",
                "NOTE: To avoid writing files multiple times, make sure to pass unique filepaths to all export functions. For board output files, you could either add the placeholder '{0}' to the path or specify the boards to export with the '{1}' argument.",
                "{{BOARD}}",
                "--board"
            )
        );
        success = false;
    }
    Ok(success)
}
#[allow(clippy::result_large_err)] // The core export callback requires project::Error.
fn legacy_exports(
    project: &mut Project,
    options: &ProjectOptions,
    boards: &[BoardId],
    variants: &[AssemblyVariantId],
    counts: &mut BTreeMap<FilePath, usize>,
) -> Result<bool> {
    let mut success = true;
    let timestamp = Timestamp::now();
    let attributes = options
        .bom_attributes
        .as_deref()
        .map(|a| {
            a.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| project.settings().custom_bom_attributes.clone());
    for (paths, specific) in [
        (&options.export_bom, false),
        (&options.export_board_bom, true),
    ] {
        for template in paths {
            println!(
                "{}",
                if specific {
                    tr!(
                        "CommandLineInterface",
                        "Export board-specific BOM to '{0}'...",
                        template
                    )
                } else {
                    tr!(
                        "CommandLineInterface",
                        "Export generic BOM to '{0}'...",
                        template
                    )
                }
            );
            let targets: Vec<_> = if specific {
                boards.iter().copied().map(Some).collect()
            } else {
                vec![None]
            };
            for board in targets {
                for variant in variants {
                    let dest = substitute(project, template, board, Some(*variant))?;
                    if let Some(board) = board {
                        println!(
                            "  - '{}' => '{}'",
                            board_name(project, board)?,
                            pretty(&path(&dest)?, template)
                        );
                    } else {
                        println!("  => '{}'", pretty(&path(&dest)?, template));
                    }
                    if extension(template) != "csv" {
                        eprintln!(
                            "  {}",
                            tr!(
                                "CommandLineInterface",
                                "ERROR: Unknown extension '{0}'.",
                                extension(template)
                            )
                        );
                        success = false;
                        continue;
                    }
                    let mut generator = BomGenerator::new(project);
                    generator.set_additional_attributes(attributes.clone());
                    let bom = generator.generate(board.and_then(|b| project.board(b)), *variant)?;
                    let csv = BomCsvWriter::new(&bom).generate_csv()?.to_csv_string()?;
                    write(&dest, csv.as_bytes(), counts)?;
                }
            }
        }
    }
    if options.export_pcb_fabrication_data {
        println!(
            "{}",
            tr!("CommandLineInterface", "Export PCB fabrication data...")
        );
        let custom = options
            .pcb_fabrication_settings
            .as_deref()
            .map(|p| {
                BoardFabricationOutputSettings::deserialize(&parse_file(p)?).map_err(Error::from)
            })
            .transpose();
        match custom {
            Err(e) => {
                eprintln!(
                    "{}",
                    tr!(
                        "CommandLineInterface",
                        "ERROR: Failed to load custom settings: {0}",
                        e
                    )
                );
                success = false;
            }
            Ok(custom) => {
                for board in boards {
                    println!(
                        "  {}",
                        tr!(
                            "CommandLineInterface",
                            "Board '{0}':",
                            board_name(project, *board)?
                        )
                    );
                    project.rebuild_planes(*board, None)?;
                    let board = project.board(*board).ok_or_else(|| {
                        Error::from(librepcb_core::project::Error::NotFound {
                            kind: librepcb_core::project::EntityKind::Board,
                            uuid: board.0,
                        })
                    })?;
                    let base = project.path().ok_or_else(|| {
                        Error::Message(tr!(
                            "CommandLineInterface",
                            "Project directory is not available."
                        ))
                    })?;
                    let exporter = BoardGerberExport::new(project, board, VERSION, timestamp);
                    let generated = exporter.generate_pcb_layers_with_callback(
                        custom
                            .as_ref()
                            .unwrap_or(&board.settings().fabrication_output_settings),
                        |name, data| {
                            let fp = base.path_to(name);
                            // Normal loaded projects always have a filesystem path.
                            println!("    => '{}'", pretty(&fp, &options.project));
                            file_utils::write_file(&fp, data.as_bytes())?;
                            *counts.entry(fp).or_default() += 1;
                            Ok(())
                        },
                    )?;
                    for name in exporter.disabled_pcb_output_paths(
                        custom
                            .as_ref()
                            .unwrap_or(&board.settings().fabrication_output_settings),
                    )? {
                        let fp = base.path_to(&name);
                        if !generated.keys().any(|written| base.path_to(written) == fp)
                            && fp.is_existing_file()
                        {
                            file_utils::remove_file(&fp)?;
                        }
                    }
                }
            }
        }
    }
    for (paths, side) in [
        (&options.export_pnp_top, BoardSide::Top),
        (&options.export_pnp_bottom, BoardSide::Bottom),
    ] {
        for template in paths {
            println!(
                "{}",
                tr!(
                    "CommandLineInterface",
                    "Export {0} assembly data to '{1}'...",
                    if side == BoardSide::Top {
                        "top"
                    } else {
                        "bottom"
                    },
                    template
                )
            );
            for id in boards {
                for variant in variants {
                    let dest = substitute(project, template, Some(*id), Some(*variant))?;
                    println!(
                        "  - '{}' => '{}'",
                        board_name(project, *id)?,
                        pretty(&path(&dest)?, template)
                    );
                    let board = project.board(*id).ok_or_else(|| {
                        Error::from(librepcb_core::project::Error::NotFound {
                            kind: librepcb_core::project::EntityKind::Board,
                            uuid: id.0,
                        })
                    })?;
                    let data = match extension(template).as_str() {
                        "csv" => {
                            let data = BoardPickPlaceGenerator::new(project, board, *variant)
                                .generate()?;
                            let mut writer = PickPlaceCsvWriter::new(&data, VERSION);
                            writer.set_generation_date(timestamp);
                            writer.set_board_sides(if side == BoardSide::Top {
                                PickPlaceSides::Top
                            } else {
                                PickPlaceSides::Bottom
                            });
                            writer.generate_csv()?.to_csv_string()?
                        }
                        "gbr" => BoardGerberExport::new(project, board, VERSION, timestamp)
                            .generate_component_layer(side, *variant)?,
                        _ => {
                            eprintln!(
                                "  {}",
                                tr!(
                                    "CommandLineInterface",
                                    "ERROR: Unknown extension '{0}'.",
                                    extension(template)
                                )
                            );
                            success = false;
                            continue;
                        }
                    };
                    write(&dest, data.as_bytes(), counts)?;
                }
            }
        }
    }
    for template in &options.export_netlist {
        println!(
            "{}",
            tr!(
                "CommandLineInterface",
                "Export netlist to '{0}'...",
                template
            )
        );
        for id in boards {
            let dest = substitute(project, template, Some(*id), None)?;
            println!(
                "  - '{}' => '{}'",
                board_name(project, *id)?,
                pretty(&path(&dest)?, template)
            );
            if extension(template) != "d356" {
                eprintln!(
                    "  {}",
                    tr!(
                        "CommandLineInterface",
                        "ERROR: Unknown extension '{0}'.",
                        extension(template)
                    )
                );
                success = false;
                continue;
            }
            let board = project.board(*id).ok_or_else(|| {
                Error::from(librepcb_core::project::Error::NotFound {
                    kind: librepcb_core::project::EntityKind::Board,
                    uuid: id.0,
                })
            })?;
            let data =
                BoardD356NetlistExport::new(project, board, VERSION, timestamp).generate()?;
            write(&dest, data.as_bytes(), counts)?;
        }
    }
    Ok(success)
}
