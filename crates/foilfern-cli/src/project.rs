//! The `open-project` command (upstream
//! `CommandLineInterface::openProject()`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use foilfern_core::export::BomCsvWriter;
use foilfern_core::export::{BoardSide, GraphicsExportSettings, PickPlaceSides, Timestamp};
use foilfern_core::fileio::{
    CleanFileNameOptions, FileNameCase, FilePath, RestoreMode, TransactionalDirectory,
    TransactionalFileSystem, file_utils,
};
use foilfern_core::job::OutputJobList;
use foilfern_core::library::org::BoardDesignRuleCheckSettings;
use foilfern_core::project::board::{
    BoardFabricationOutputSettings, BoardGerberExport, ExportInfo, export_component_layer,
    export_d356_netlist, export_pick_place_csv,
};
use foilfern_core::project::{
    AssemblyVariantId, BoardId, BomGenerator, GraphicsExporter, GraphicsPage, GraphicsPageContent,
    Mutation, OutputJobEvent, OutputJobRunner, ProjectAttributeLookup, ProjectLoader, erc,
};
use foilfern_core::serialization::{DeserializeObject, Mode, SExpression};
use foilfern_graphics::ProjectGraphicsExporter;
use foilfern_i18n::tr;

use crate::APP_VERSION;
use crate::args::{OpenProjectArgs, TR};
use crate::drc;
use crate::error::CliResult;
use crate::output::{
    absolute_path, current_dir, fail_if_file_format_unstable, format_check_summary,
    prepare_rule_check_messages, pretty_path, print, print_err,
};

/// Filter for substituted attribute values in output paths.
pub fn clean_file_name(value: &str) -> String {
    FilePath::clean_file_name(
        value,
        CleanFileNameOptions::new(true, FileNameCase::Keep),
        120,
    )
}

/// Returns the lower case suffix after the last `.` of a path given by the
/// user (upstream `destStr.split('.').last().toLower()`).
fn user_suffix(path: &str) -> String {
    path.rsplit('.').next().unwrap_or_default().to_lowercase()
}

/// Reads and parses an S-expression file given on the command line.
fn read_sexpression_file(path: &str) -> CliResult<SExpression> {
    let fp = absolute_path(path);
    let content = file_utils::read_file(&fp)?;
    Ok(SExpression::parse(
        &content,
        Some(fp.as_path()),
        Mode::LibrePcb,
    )?)
}

/// The creator written into exported PDF files (upstream
/// `"LibrePCB " + Application::getVersion()`).
pub fn graphics_creator() -> String {
    format!("LibrePCB {APP_VERSION}")
}

/// Runs `open-project`, returns whether it succeeded.
pub fn open_project(args: &OpenProjectArgs) -> bool {
    match open_project_impl(args) {
        Ok(success) => success,
        Err(e) => {
            print_err(&tr!(TR, "ERROR: {0}", e));
            false
        }
    }
}

fn open_project_impl(a: &OpenProjectArgs) -> CliResult<bool> {
    let project_file = a
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    let mut success = true;
    let mut written_files: BTreeMap<FilePath, usize> = BTreeMap::new();
    let written_job_files: Arc<Mutex<BTreeMap<FilePath, usize>>> = Arc::default();
    let info = ExportInfo::now(APP_VERSION);

    // Open project.
    let project_fp = absolute_path(project_file);
    print(&tr!(
        TR,
        "Open project '{0}'...",
        pretty_path(&project_fp, project_file)
    ));
    let is_lppz = project_fp.suffix() == "lppz";
    let parent_dir = project_fp.parent_dir().unwrap_or_else(current_dir);
    let (fs, project_file_name) = if is_lppz {
        let fs = TransactionalFileSystem::open_ro(&parent_dir)?;
        fs.remove_dir_recursively("")?; // 1) get a clean initial state
        fs.load_from_zip(&project_fp)?; // 2) load files from ZIP
        let name = fs
            .files("")
            .into_iter()
            .rfind(|f| f.ends_with(".lpp"))
            .unwrap_or_default();
        (Arc::new(fs), name)
    } else {
        let fs = TransactionalFileSystem::open(&parent_dir, a.save, RestoreMode::No, None)?;
        (Arc::new(fs), project_fp.file_name().to_owned())
    };
    let mut loader = ProjectLoader::new();
    loader.set_application_version(APP_VERSION);
    let mut project = loader.open(
        TransactionalDirectory::new(Arc::clone(&fs), ""),
        &project_file_name,
    )?;
    if let Some(log) = loader.migration_log() {
        print(&tr!(
            TR,
            "Attention: Project has been migrated to a newer file format!"
        ));
        for msg in &log.messages {
            let multiplier = match msg.affected_items {
                Some(n) if n > 0 => format!(" ({n}x)"),
                _ => String::new(),
            };
            print(&format!(
                " - {}{multiplier}: {}",
                msg.severity.to_tr_string(),
                msg.message
            ));
        }
    }

    // Set the default assembly variant.
    if let Some(name) = a.set_default_variant.as_deref().filter(|s| !s.is_empty()) {
        print(&tr!(TR, "Set default assembly variant to '{0}'...", name));
        let variants = project.circuit().assembly_variants();
        match variants.iter().position(|av| av.name().as_str() == name) {
            Some(0) => {}
            Some(index) => {
                let variant = variants.iter().nth(index).cloned();
                if let Some(variant) = variant {
                    project.apply(Mutation::MoveAssemblyVariant {
                        variant: AssemblyVariantId(variant.uuid()),
                        index: 0,
                    })?;
                }
            }
            None => {
                print_err(&tr!(
                    TR,
                    "ERROR: No assembly variant with the name '{0}' found.",
                    name
                ));
                success = false;
            }
        }
    }

    // Parse list of assembly variants.
    let mut assembly_variants: Vec<AssemblyVariantId> = Vec::new();
    let all_variants: Vec<(AssemblyVariantId, String)> = project
        .circuit()
        .assembly_variants()
        .iter()
        .map(|av| (AssemblyVariantId(av.uuid()), av.name().to_string()))
        .collect();
    for name in &a.variant {
        match all_variants.iter().find(|(_, n)| n == name) {
            Some((id, _)) => {
                if !assembly_variants.contains(id) {
                    assembly_variants.push(*id);
                }
            }
            None => {
                print_err(&tr!(
                    TR,
                    "ERROR: No assembly variant with the name '{0}' found.",
                    name
                ));
                success = false;
            }
        }
    }
    for index in &a.variant_index {
        let variant = parse_index(index).and_then(|i| all_variants.get(i));
        match variant {
            Some((id, _)) => {
                if !assembly_variants.contains(id) {
                    assembly_variants.push(*id);
                }
            }
            None => {
                print_err(&tr!(
                    TR,
                    "ERROR: Assembly variant index '{0}' is invalid.",
                    index
                ));
                success = false;
            }
        }
    }

    // If no assembly variants are specified, export all variants.
    if a.variant.is_empty() && a.variant_index.is_empty() {
        assembly_variants = all_variants.iter().map(|(id, _)| *id).collect();
    }

    // Parse list of boards.
    let mut boards: Vec<BoardId> = Vec::new();
    for name in &a.board {
        match project.board_by_name(name) {
            Some(board) => {
                if !boards.contains(&board.id()) {
                    boards.push(board.id());
                }
            }
            None => {
                print_err(&tr!(TR, "ERROR: No board with the name '{0}' found.", name));
                success = false;
            }
        }
    }
    for index in &a.board_index {
        let board = parse_index(index).and_then(|i| project.boards().get(i));
        match board {
            Some(board) => {
                if !boards.contains(&board.id()) {
                    boards.push(board.id());
                }
            }
            None => {
                print_err(&tr!(TR, "ERROR: Board index '{0}' is invalid.", index));
                success = false;
            }
        }
    }

    // Remove other boards (note: do this at the very beginning to make all
    // the other commands, e.g. the ERC, working without the removed boards).
    if a.remove_other_boards {
        print(&tr!(TR, "Remove other boards..."));
        let others: Vec<(BoardId, String)> = project
            .boards()
            .iter()
            .filter(|b| !boards.contains(&b.id()))
            .map(|b| (b.id(), b.properties().name.to_string()))
            .collect();
        for (id, name) in others {
            print(&format!("  - '{name}'"));
            project.apply(Mutation::RemoveBoard(id))?;
        }
    }

    // If no boards are specified, export all boards.
    if a.board.is_empty() && a.board_index.is_empty() {
        boards = project.boards().iter().map(|b| b.id()).collect();
    }

    // Build planes, if needed.
    let run_jobs = !a.run_job.is_empty() || a.run_jobs;
    if a.drc || a.export_pcb_fabrication_data || run_jobs {
        for board in &boards {
            if let Some(b) = project.board(*board) {
                log::info!(
                    "Rebuilding all planes of board '{}'...",
                    b.properties().name
                );
            }
            project.rebuild_planes(*board, None)?;
        }
    } else {
        log::info!("No need to rebuild planes, thus skipped.");
    }

    // Check for non-canonical files (strict mode).
    if a.strict {
        print(&tr!(TR, "Check for non-canonical files..."));
        if is_lppz {
            print_err(&format!(
                "  {}",
                tr!(
                    TR,
                    "ERROR: The option '--strict' is not available for *.lppz files!"
                )
            ));
            success = false;
        } else {
            project.save()?;
            let mut paths: Vec<String> = fs
                .check_for_modifications()?
                .into_iter()
                .filter(|p| !p.contains(".user.lp")) // Ignore user config files.
                .collect();
            // Sort file paths to increase readability of console output.
            paths.sort();
            for path in &paths {
                let fp = fs.path().path_to(path);
                print_err(&format!(
                    "    - Non-canonical file: '{}'",
                    pretty_path(&fp, project_file)
                ));
            }
            if !paths.is_empty() {
                success = false;
            }
        }
    }

    // ERC
    if a.erc {
        print(&tr!(TR, "Run ERC..."));
        let messages = erc::run_erc(&project)
            .into_iter()
            .map(|m| m.message().clone())
            .collect();
        let (non_approved, approved) =
            prepare_rule_check_messages(messages, project.erc_approvals());
        for line in format_check_summary(approved, non_approved.len(), "  ") {
            print(&line);
        }
        for msg in &non_approved {
            print_err(&format!("    - {msg}"));
            success = false;
        }
    }

    // DRC
    if a.drc {
        print(&tr!(TR, "Run DRC..."));
        let mut custom_settings: Option<BoardDesignRuleCheckSettings> = None;
        let mut boards_to_check = boards.clone();
        if let Some(path) = a.drc_settings.as_deref().filter(|p| !p.is_empty()) {
            log::debug!("Load custom DRC settings: {path}");
            let result = read_sexpression_file(path)
                .and_then(|root| Ok(BoardDesignRuleCheckSettings::deserialize(&root)?));
            match result {
                Ok(settings) => custom_settings = Some(settings),
                Err(e) => {
                    print_err(&tr!(TR, "ERROR: Failed to load custom settings: {0}", e));
                    success = false;
                    boards_to_check.clear(); // Avoid exporting any boards.
                }
            }
        }
        for board in boards_to_check {
            let Some(b) = project.board(board) else {
                continue;
            };
            print(&format!(
                "  {}",
                tr!(TR, "Board '{0}':", b.properties().name)
            ));
            let settings = custom_settings
                .clone()
                .unwrap_or_else(|| b.settings().drc_settings.clone());
            let approvals = b.drc_approvals().clone();
            match drc::run_drc(&mut project, board, &settings) {
                Ok(result) => {
                    for msg in &result.errors {
                        print_err(&format!("FATAL ERROR: {msg}"));
                        success = false;
                    }
                    let (non_approved, approved) =
                        prepare_rule_check_messages(result.messages, &approvals);
                    for line in format_check_summary(approved, non_approved.len(), "    ") {
                        print(&line);
                    }
                    for msg in &non_approved {
                        print_err(&format!("      - {msg}"));
                        success = false;
                    }
                }
                Err(e) => {
                    print_err(&format!("    {}", tr!(TR, "ERROR: {0}", e)));
                    success = false;
                }
            }
        }
    }

    // Run output jobs.
    if run_jobs {
        // Determine jobs.
        let all_jobs: Option<OutputJobList> =
            match a.jobs.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                Some(path) => {
                    log::debug!("Load custom output jobs: {path}");
                    let result = read_sexpression_file(path)
                        .and_then(|root| Ok(OutputJobList::deserialize(&root)?));
                    match result {
                        Ok(jobs) => Some(jobs),
                        Err(e) => {
                            print_err(&tr!(TR, "ERROR: Failed to load custom output jobs: {0}", e));
                            success = false;
                            None
                        }
                    }
                }
                None => Some(project.output_jobs().clone()),
            };
        if let Some(all_jobs) = all_jobs {
            let mut jobs = Vec::new();
            if a.run_jobs {
                jobs = all_jobs.iter().cloned().collect();
            } else {
                for name in &a.run_job {
                    match all_jobs.iter().find(|job| job.name().as_str() == name) {
                        Some(job) => jobs.push(job.clone()),
                        None => {
                            print_err(&tr!(
                                TR,
                                "ERROR: No output job with the name '{0}' found.",
                                name
                            ));
                            success = false;
                        }
                    }
                }
            }
            let result = (|| -> CliResult<()> {
                let mut runner = OutputJobRunner::new(&mut project, info.clone())?;
                runner.set_step_exporter(Some(foilfern_step::board_exporter()));
                runner.set_graphics_exporter(Some(Arc::new(ProjectGraphicsExporter::new(
                    graphics_creator(),
                ))));
                let counter = Arc::clone(&written_job_files);
                let style = project_file.to_owned();
                runner.set_observer(Some(Box::new(move |event| match event {
                    OutputJobEvent::JobStarted(job) => {
                        print(&tr!(TR, "Run output job '{0}'...", job.name()));
                    }
                    OutputJobEvent::AboutToWriteFile(fp) => {
                        print(&format!("  => '{}'", pretty_path(fp, &style)));
                        *counter
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .entry(fp.clone())
                            .or_default() += 1;
                    }
                    OutputJobEvent::Warning(message)
                        if message.starts_with("STEP export approximated ") =>
                    {
                        print_err(&format!("WARNING: {message}"));
                    }
                    OutputJobEvent::AboutToRemoveFile(_) | OutputJobEvent::Warning(_) => {}
                })));
                if let Some(dir) = a.outdir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                    runner.set_output_directory(&absolute_path(dir));
                }
                log::debug!(
                    "Using output base directory: {}",
                    runner.output_directory().to_native()
                );
                runner.run(&jobs)?;
                Ok(())
            })();
            if let Err(e) = result {
                print_err(&format!("{} {e}", tr!(TR, "ERROR:")));
                success = false;
            }
        }
    }

    // Export schematics.
    for dest in &a.export_schematics {
        print(&tr!(TR, "Export schematics to '{0}'...", dest));
        let lookup = ProjectAttributeLookup::for_project(&project, None);
        let dest_path = lookup.substitute_filtered(dest, &mut clean_file_name);
        let fp = absolute_path(&dest_path);
        let settings = GraphicsExportSettings::default();
        let pages: Vec<GraphicsPage> = project
            .schematics()
            .iter()
            .map(|s| GraphicsPage {
                content: GraphicsPageContent::Schematic(s.id()),
                settings: settings.clone(),
            })
            .collect();
        let result = ProjectGraphicsExporter::new(graphics_creator()).export(
            &project,
            &pages,
            &fp,
            project.metadata().name.as_str(),
        );
        for written in &result.written_files {
            print(&format!("  => '{}'", pretty_path(written, &dest_path)));
            *written_files.entry(written.clone()).or_default() += 1;
        }
        for error in &result.errors {
            print_err(&format!("  {}: {}", tr!(TR, "ERROR"), error));
            success = false;
        }
    }

    // Export BOM.
    if !a.export_bom.is_empty() || !a.export_board_bom.is_empty() {
        let jobs = a
            .export_bom
            .iter()
            .map(|fp| (fp, false))
            .chain(a.export_board_bom.iter().map(|fp| (fp, true)));
        let attributes: Vec<String> = match a.bom_attributes.as_deref() {
            None | Some("") => project.settings().custom_bom_attributes.clone(),
            Some(attributes) => attributes
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        };
        for (dest, board_specific) in jobs {
            let boards_to_export: Vec<Option<BoardId>> = if board_specific {
                print(&tr!(TR, "Export board-specific BOM to '{0}'...", dest));
                boards.iter().copied().map(Some).collect()
            } else {
                print(&tr!(TR, "Export generic BOM to '{0}'...", dest));
                vec![None]
            };
            for board in &boards_to_export {
                let board = board.and_then(|id| project.board(id));
                for av in &assembly_variants {
                    let variant = project.circuit().assembly_variant(*av);
                    let lookup = match board {
                        Some(b) => ProjectAttributeLookup::for_board(&project, b, variant),
                        None => ProjectAttributeLookup::for_project(&project, variant),
                    };
                    let dest_path = lookup.substitute_filtered(dest, &mut clean_file_name);
                    let fp = absolute_path(&dest_path);
                    let mut generator = BomGenerator::new(&project);
                    generator.set_additional_attributes(attributes.clone());
                    let bom = generator.generate(board, *av);
                    match board {
                        Some(b) => print(&format!(
                            "  - '{}' => '{}'",
                            b.properties().name,
                            pretty_path(&fp, &dest_path)
                        )),
                        None => print(&format!("  => '{}'", pretty_path(&fp, &dest_path))),
                    }
                    let suffix = user_suffix(dest);
                    if suffix == "csv" {
                        BomCsvWriter::new(&bom).generate_csv()?.save_to_file(&fp)?;
                        *written_files.entry(fp).or_default() += 1;
                    } else {
                        print_err(&format!(
                            "  {}",
                            tr!(TR, "ERROR: Unknown extension '{0}'.", suffix)
                        ));
                        success = false;
                    }
                }
            }
        }
    }

    // Export PCB fabrication data.
    if a.export_pcb_fabrication_data {
        print(&tr!(TR, "Export PCB fabrication data..."));
        let mut custom_settings: Option<BoardFabricationOutputSettings> = None;
        let mut boards_to_export = boards.clone();
        if let Some(path) = a
            .pcb_fabrication_settings
            .as_deref()
            .filter(|p| !p.is_empty())
        {
            log::debug!("Load custom fabrication output settings: {path}");
            let result = read_sexpression_file(path)
                .and_then(|root| Ok(BoardFabricationOutputSettings::deserialize(&root)?));
            match result {
                Ok(settings) => custom_settings = Some(settings),
                Err(e) => {
                    print_err(&tr!(TR, "ERROR: Failed to load custom settings: {0}", e));
                    success = false;
                    boards_to_export.clear(); // Avoid exporting any boards.
                }
            }
        }
        for board in &boards_to_export {
            let Some(b) = project.board(*board) else {
                continue;
            };
            print(&format!(
                "  {}",
                tr!(TR, "Board '{0}':", b.properties().name)
            ));
            let settings = custom_settings
                .clone()
                .unwrap_or_else(|| b.settings().fabrication_output_settings.clone());
            let mut export = BoardGerberExport::new(&project, *board, &info)?;
            export.export_pcb_layers(&settings)?;
            for fp in export.written_files() {
                print(&format!("    => '{}'", pretty_path(fp, project_file)));
                *written_files.entry(fp.clone()).or_default() += 1;
            }
        }
    }

    // Export pick&place files.
    let pnp_jobs = a
        .export_pnp_top
        .iter()
        .map(|dest| (tr!(TR, "top"), PickPlaceSides::Top, BoardSide::Top, dest))
        .chain(a.export_pnp_bottom.iter().map(|dest| {
            (
                tr!(TR, "bottom"),
                PickPlaceSides::Bottom,
                BoardSide::Bottom,
                dest,
            )
        }));
    for (side_str, csv_side, gbr_side, dest) in pnp_jobs {
        print(&tr!(
            TR,
            "Export {0} assembly data to '{1}'...",
            side_str,
            dest
        ));
        for board in &boards {
            let Some(b) = project.board(*board) else {
                continue;
            };
            for av in &assembly_variants {
                let variant = project.circuit().assembly_variant(*av);
                let lookup = ProjectAttributeLookup::for_board(&project, b, variant);
                let dest_path = lookup.substitute_filtered(dest, &mut clean_file_name);
                let fp = absolute_path(&dest_path);
                print(&format!(
                    "  - '{}' => '{}'",
                    b.properties().name,
                    pretty_path(&fp, &dest_path)
                ));
                let suffix = user_suffix(dest);
                if suffix == "csv" {
                    export_pick_place_csv(
                        &project,
                        *board,
                        *av,
                        csv_side,
                        &fp,
                        APP_VERSION,
                        Timestamp::now(),
                    )?;
                    *written_files.entry(fp).or_default() += 1;
                } else if suffix == "gbr" {
                    export_component_layer(&project, *board, gbr_side, *av, &fp, &info)?;
                    *written_files.entry(fp).or_default() += 1;
                } else {
                    print_err(&format!(
                        "  {}",
                        tr!(TR, "ERROR: Unknown extension '{0}'.", suffix)
                    ));
                    success = false;
                }
            }
        }
    }

    // Export netlist files.
    for dest in &a.export_netlist {
        print(&tr!(TR, "Export netlist to '{0}'...", dest));
        for board in &boards {
            let Some(b) = project.board(*board) else {
                continue;
            };
            let lookup = ProjectAttributeLookup::for_board(&project, b, None);
            let dest_path = lookup.substitute_filtered(dest, &mut clean_file_name);
            let fp = absolute_path(&dest_path);
            print(&format!(
                "  - '{}' => '{}'",
                b.properties().name,
                pretty_path(&fp, &dest_path)
            ));
            let suffix = user_suffix(dest);
            if suffix == "d356" {
                export_d356_netlist(&project, *board, &fp, &info)?;
                *written_files.entry(fp).or_default() += 1;
            } else {
                print_err(&format!(
                    "  {}",
                    tr!(TR, "ERROR: Unknown extension '{0}'.", suffix)
                ));
                success = false;
            }
        }
    }

    // Save project.
    if a.save {
        print(&tr!(TR, "Save project..."));
        if fail_if_file_format_unstable() {
            success = false;
        } else {
            project.save()?;
            if is_lppz {
                fs.export_to_zip_file(&project_fp, None)?;
            } else {
                fs.save()?;
            }
        }
    }

    // Fail if some files were written multiple times.
    let job_files = written_job_files
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let mut files_overwritten = false;
    for (fp, count) in &written_files {
        let total = count + job_files.get(fp).copied().unwrap_or(0);
        if total > 1 {
            files_overwritten = true;
            print_err(&tr!(
                TR,
                "ERROR: The file '{0}' was written multiple times!",
                pretty_path(fp, project_file)
            ));
        }
    }
    if files_overwritten {
        print_err(&tr!(
            TR,
            "NOTE: To avoid writing files multiple times, make sure to pass unique filepaths to all export functions. For board output files, you could either add the placeholder '{0}' to the path or specify the boards to export with the '{1}' argument.",
            "{{BOARD}}",
            "--board"
        ));
        success = false;
    }

    Ok(success)
}

/// Parses an index given on the command line (upstream
/// `str.trimmed().toInt()`), `None` if invalid or negative.
fn parse_index(s: &str) -> Option<usize> {
    s.trim()
        .parse::<i64>()
        .ok()
        .and_then(|i| usize::try_from(i).ok())
}
