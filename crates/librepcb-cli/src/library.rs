//! The `open-library`, `open-symbol`, `open-package` and `open-step`
//! commands (upstream `CommandLineInterface::openLibrary()`,
//! `processLibraryElement()`, `openSymbol()`, `openPackage()`,
//! `openStep()`).
//!
//! STEP model operations use the shared pure-Rust CAD backend.

use std::collections::BTreeMap;
use std::sync::Arc;

use librepcb_core::export::GraphicsExportSettings;
use librepcb_core::fileio::{
    FilePath, FileSystem, RestoreMode, TransactionalDirectory, TransactionalFileSystem,
};
use librepcb_core::library::cat::{ComponentCategory, PackageCategory};
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::org::Organization;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::{Library, LibraryBaseElement};
use librepcb_i18n::tr;
use librepcb_scene::export::{ExportPage, GraphicsExport, footprint_drawing, symbol_drawing};

use crate::args::{OpenLibraryArgs, OpenPackageArgs, OpenStepArgs, OpenSymbolArgs, TR};
use crate::error::CliResult;
use crate::output::{
    absolute_path, fail_if_file_format_unstable, format_check_summary, prepare_rule_check_messages,
    pretty_path, print, print_err,
};
use crate::project::{clean_file_name, graphics_creator};

/// Settings of symbol and footprint exports: the defaults without page
/// margins.
fn library_export_settings() -> GraphicsExportSettings {
    let zero = librepcb_core::types::UnsignedLength::ZERO;
    GraphicsExportSettings {
        margin_left: zero,
        margin_top: zero,
        margin_right: zero,
        margin_bottom: zero,
        ..GraphicsExportSettings::default()
    }
}

/// Options of [`process_element()`].
#[derive(Debug, Clone, Copy)]
struct ProcessOptions {
    check: bool,
    minify_step: bool,
    save: bool,
    strict: bool,
}

/// Runs `open-library`, returns whether it succeeded.
pub fn open_library(a: &OpenLibraryArgs) -> bool {
    match open_library_impl(a) {
        Ok(success) => success,
        Err(e) => {
            print_err(&tr!(TR, "ERROR: {0}", e));
            false
        }
    }
}

fn open_fs(
    fp: &librepcb_core::fileio::FilePath,
    writable: bool,
) -> CliResult<Arc<TransactionalFileSystem>> {
    Ok(Arc::new(TransactionalFileSystem::open(
        fp,
        writable,
        RestoreMode::No,
        None,
    )?))
}

fn open_library_impl(a: &OpenLibraryArgs) -> CliResult<bool> {
    let lib_dir = a
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    let opts = ProcessOptions {
        check: a.check,
        minify_step: a.minify_step,
        save: a.save,
        strict: a.strict,
    };
    let mut success = true;

    // Open library.
    let lib_fp = absolute_path(lib_dir);
    print(&tr!(
        TR,
        "Open library '{0}'...",
        pretty_path(&lib_fp, lib_dir)
    ));
    let lib_fs = open_fs(&lib_fp, a.save)?;
    let mut lib = Library::open(TransactionalDirectory::new(Arc::clone(&lib_fs), ""))?;
    process_element(lib_dir, &lib_fs, &mut lib, opts, false, &mut success)?;

    if a.all {
        process_elements::<ComponentCategory>(&lib, lib_dir, opts, &mut success, |n| {
            tr!(TR, "Process {0} component categories...", n)
        })?;
        process_elements::<PackageCategory>(&lib, lib_dir, opts, &mut success, |n| {
            tr!(TR, "Process {0} package categories...", n)
        })?;
        process_elements::<Symbol>(&lib, lib_dir, opts, &mut success, |n| {
            tr!(TR, "Process {0} symbols...", n)
        })?;
        process_elements::<Package>(&lib, lib_dir, opts, &mut success, |n| {
            tr!(TR, "Process {0} packages...", n)
        })?;
        process_elements::<Component>(&lib, lib_dir, opts, &mut success, |n| {
            tr!(TR, "Process {0} components...", n)
        })?;
        process_elements::<Device>(&lib, lib_dir, opts, &mut success, |n| {
            tr!(TR, "Process {0} devices...", n)
        })?;
        process_elements::<Organization>(&lib, lib_dir, opts, &mut success, |n| {
            tr!(TR, "Process {0} organizations...", n)
        })?;
    }
    Ok(success)
}

/// Opens and processes all elements of type `E` of the library.
fn process_elements<E: LibraryBaseElement>(
    lib: &Library,
    lib_dir: &str,
    opts: ProcessOptions,
    success: &mut bool,
    header: impl Fn(usize) -> String,
) -> CliResult<()> {
    let mut elements = lib.search_for_elements::<E>();
    elements.sort(); // For deterministic console output.
    print(&header(elements.len()));
    let lib_fp = absolute_path(lib_dir);
    for dir in &elements {
        let fp = lib_fp.path_to(dir);
        log::info!("{}", tr!(TR, "Open '{0}'...", pretty_path(&fp, lib_dir)));
        let fs = open_fs(&fp, opts.save)?;
        let mut element = E::open(TransactionalDirectory::new(Arc::clone(&fs), ""))?;
        let is_package = E::SHORT_ELEMENT_NAME == Package::SHORT_ELEMENT_NAME;
        process_element(lib_dir, &fs, &mut element, opts, is_package, success)?;
    }
    Ok(())
}

/// Upstream `processLibraryElement()`.
fn process_element<E: LibraryBaseElement>(
    lib_dir: &str,
    fs: &TransactionalFileSystem,
    element: &mut E,
    opts: ProcessOptions,
    is_package: bool,
    success: &mut bool,
) -> CliResult<()> {
    // Keep track of whether the error header was already printed.
    let mut header_printed = false;
    let header = format!(
        "  - {} ({}):",
        element.metadata().name(),
        element.metadata().uuid()
    );
    let mut print_error_header_once = || {
        if !header_printed {
            print_err(&header);
            header_printed = true;
        }
    };
    let element_path = pretty_path(fs.path(), lib_dir);

    // Save element to transactional file system, if needed.
    if opts.strict || opts.save {
        element.save()?;
    }

    // Minify STEP files, if needed.
    if opts.minify_step && is_package {
        for file in fs.files("").into_iter().filter(|f| f.ends_with(".step")) {
            let fp = pretty_path(&fs.path().path_to(&file), lib_dir);
            log::info!("{}", tr!(TR, "Minify STEP model '{0}'...", fp));
            let result = (|| -> CliResult<()> {
                let content = fs.read(&file)?;
                let minified = librepcb_step::minify_step(&content)?;
                if minified != content {
                    print(&tr!(
                        TR,
                        "  - Minified '{0}' from {1} to {2} bytes",
                        fp,
                        content.len(),
                        minified.len()
                    ));
                    librepcb_step::StepModel::from_step(&minified)?;
                    element.directory_mut().write(&file, &minified)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                print_error_header_once();
                print_err(&format!(
                    "    - Failed to minify STEP model '{fp}': {error}"
                ));
                *success = false;
            }
        }
    }

    // Check for non-canonical files (strict mode).
    if opts.strict {
        log::info!(
            "{}",
            tr!(TR, "Check '{0}' for non-canonical files...", element_path)
        );
        let mut paths = fs.check_for_modifications()?;
        if !paths.is_empty() {
            // Sort file paths to increase readability of console output.
            paths.sort();
            print_error_header_once();
            for path in &paths {
                print_err(&format!(
                    "    - Non-canonical file: '{}'",
                    pretty_path(&fs.path().path_to(path), lib_dir)
                ));
            }
            *success = false;
        }
    }

    // Run library element check, if needed. We do not check deprecated
    // elements since this would require people to keep maintenance for
    // deprecated elements ongoing with every new LibrePCB release, which
    // makes no sense.
    if opts.check {
        if element.metadata().is_deprecated() {
            log::info!(
                "{}",
                tr!(TR, "Skip checks for '{0}' (deprecated)", element_path)
            );
        } else {
            log::info!("{}", tr!(TR, "Run checks for '{0}'...", element_path));
            let (non_approved, approved) = gather_check_messages(element)?;
            for line in format_check_summary(approved, non_approved.len(), "  ") {
                log::info!("{line}");
            }
            for msg in &non_approved {
                print_error_header_once();
                print_err(&format!("    - {msg}"));
                *success = false;
            }
        }
    }

    // Save element to file system, if needed.
    if opts.save {
        log::info!("{}", tr!(TR, "Save '{0}'...", element_path));
        if fail_if_file_format_unstable() {
            *success = false;
        } else {
            fs.save()?;
        }
    }

    // Do not propagate changes in the transactional file system to the
    // following checks.
    fs.discard_changes();
    Ok(())
}

/// Runs the checks of an element, returns the non-approved messages and
/// the number of approved messages (upstream
/// `gatherElementCheckMessages()`).
fn gather_check_messages<E: LibraryBaseElement>(element: &E) -> CliResult<(Vec<String>, usize)> {
    let messages = element
        .run_checks()?
        .iter()
        .map(|m| m.to_message())
        .collect();
    Ok(prepare_rule_check_messages(
        messages,
        element.metadata().message_approvals(),
    ))
}

/// Prints the check results of a single element (upstream
/// `formatLibraryElementCheckSummary()` and the non-approved messages).
fn print_check_results<E: LibraryBaseElement>(element: &E, success: &mut bool) -> CliResult<()> {
    let (non_approved, approved) = gather_check_messages(element)?;
    print(&tr!(TR, "Run checks..."));
    for line in format_check_summary(approved, non_approved.len(), "  ") {
        print(&line);
    }
    for msg in &non_approved {
        print_err(&format!("    - {msg}"));
        *success = false;
    }
    Ok(())
}

/// Runs `open-symbol`, returns whether it succeeded.
pub fn open_symbol(a: &OpenSymbolArgs) -> bool {
    let result = (|| -> CliResult<bool> {
        let mut success = true;
        let symbol_file = a
            .positionals
            .first()
            .map(String::as_str)
            .unwrap_or_default();
        let symbol_fp = absolute_path(symbol_file);
        print(&tr!(
            TR,
            "Open symbol '{0}'...",
            pretty_path(&symbol_fp, symbol_file)
        ));
        let fs = open_fs(&symbol_fp, false)?;
        let symbol = Symbol::open(TransactionalDirectory::new(fs, ""))?;
        log::info!(
            "{}",
            tr!(TR, "Opened symbol: {0}", symbol.metadata().name())
        );

        // Process the symbol element (validation checks).
        if a.check {
            print_check_results(&symbol, &mut success)?;
        }

        // Export symbol to graphics file.
        if let Some(export) = a.export.as_deref().filter(|s| !s.is_empty()) {
            print(&tr!(TR, "Export symbol to '{0}'...", export));
            let name = symbol.metadata().name().to_string();
            let uuid = symbol.metadata().uuid().to_string();
            let dest = librepcb_core::attribute::substitute(
                export,
                |key| match key {
                    "SYMBOL" => Some(name.clone()),
                    "SYMBOL_UUID" => Some(uuid.clone()),
                    _ => None,
                },
                Some(&mut clean_file_name),
            );
            let fp = absolute_path(&dest);
            let settings = library_export_settings();
            let font = librepcb_scene::default_stroke_font();
            let page = ExportPage {
                drawing: symbol_drawing(&symbol, font.as_ref(), &settings),
                settings,
            };
            let mut export = GraphicsExport::new(graphics_creator());
            export.set_document_name(symbol.metadata().name().as_str());
            let result = export.export(&[page], &fp);
            for written in &result.written_files {
                print(&format!("  => '{}'", pretty_path(written, &dest)));
            }
            for error in &result.errors {
                print_err(&format!("  {}: {}", tr!(TR, "ERROR"), error));
                success = false;
            }
        }
        Ok(success)
    })();
    result.unwrap_or_else(|e| {
        print_err(&tr!(TR, "ERROR: {0}", e));
        false
    })
}

/// Runs `open-package`, returns whether it succeeded.
pub fn open_package(a: &OpenPackageArgs) -> bool {
    let result = (|| -> CliResult<bool> {
        let mut success = true;
        let package_file = a
            .positionals
            .first()
            .map(String::as_str)
            .unwrap_or_default();
        let package_fp = absolute_path(package_file);
        print(&tr!(
            TR,
            "Open package '{0}'...",
            pretty_path(&package_fp, package_file)
        ));
        let fs = open_fs(&package_fp, false)?;
        let package = Package::open(TransactionalDirectory::new(fs, ""))?;
        log::info!(
            "{}",
            tr!(TR, "Opened package: {0}", package.metadata().name())
        );

        // Process the package element (validation checks).
        if a.check {
            print_check_results(&package, &mut success)?;
        }

        // Export footprints to graphics files.
        if let Some(export) = a.export.as_deref().filter(|s| !s.is_empty()) {
            print(&tr!(TR, "Export footprint(s) to '{0}'...", export));
            let name = package.metadata().name().to_string();
            let uuid = package.metadata().uuid().to_string();
            let font = librepcb_scene::default_stroke_font();
            let mut written_files: BTreeMap<FilePath, usize> = BTreeMap::new();
            for (index, footprint) in package.footprints().iter().enumerate() {
                let footprint_name = footprint.names().default_value().to_string();
                let footprint_uuid = footprint.uuid().to_string();
                let dest = librepcb_core::attribute::substitute(
                    export,
                    |key| match key {
                        "PACKAGE" => Some(name.clone()),
                        "PACKAGE_UUID" => Some(uuid.clone()),
                        "FOOTPRINT" => Some(footprint_name.clone()),
                        "FOOTPRINT_UUID" => Some(footprint_uuid.clone()),
                        "FOOTPRINT_INDEX" => Some((index + 1).to_string()),
                        _ => None,
                    },
                    Some(&mut clean_file_name),
                );
                let fp = absolute_path(&dest);
                let settings = library_export_settings();
                let page = ExportPage {
                    drawing: footprint_drawing(footprint, font.as_ref(), &settings),
                    settings,
                };
                let mut export = GraphicsExport::new(graphics_creator());
                export.set_document_name(format!("{name} ({footprint_name})"));
                let result = export.export(&[page], &fp);
                for written in &result.written_files {
                    print(&format!("  => '{}'", pretty_path(written, &dest)));
                    *written_files.entry(written.clone()).or_default() += 1;
                }
                for error in &result.errors {
                    print_err(&format!("  {}: {}", tr!(TR, "ERROR"), error));
                    success = false;
                }
            }

            // Fail if some files were written multiple times.
            let mut files_overwritten = false;
            for (fp, count) in &written_files {
                if *count > 1 {
                    files_overwritten = true;
                    print_err(&tr!(
                        TR,
                        "ERROR: The file '{0}' was written multiple times!",
                        pretty_path(fp, package_file)
                    ));
                }
            }
            if files_overwritten {
                print_err(&tr!(
                    TR,
                    "NOTE: To avoid writing files multiple times, make sure to pass unique filepaths to all export functions. For footprint output files, you could add a placeholder like '{0}' to the path.",
                    "{{FOOTPRINT}}"
                ));
                success = false;
            }
        }
        Ok(success)
    })();
    result.unwrap_or_else(|e| {
        print_err(&tr!(TR, "ERROR: {0}", e));
        false
    })
}

/// Runs standalone STEP operations through the shared CAD backend.
pub fn open_step(a: &OpenStepArgs) -> bool {
    crate::step::run(a).unwrap_or_else(|error| {
        print_err(&tr!(TR, "ERROR: {0}", error));
        false
    })
}
