//! Library commands from apps/librepcb-cli/commandlineinterface.cpp.
use crate::{ElementOptions, LibraryOptions, error::Result, support::*};
use librepcb_core::{
    fileio::{CleanFileNameOptions, FilePath, FileSystem},
    library::{
        Library, LibraryBaseElement,
        cat::{ComponentCategory, PackageCategory},
        cmp::Component,
        dev::Device,
        org::Organization,
        pkg::Package,
        sym::Symbol,
    },
};
use librepcb_i18n::tr;
fn process<E: LibraryBaseElement>(element: &mut E, options: &LibraryOptions) -> Result<bool> {
    let mut success = true;
    let mut header = false;
    let heading = format!(
        "  - {} ({}):",
        element.metadata().names().default_value(),
        element.metadata().uuid()
    );
    if options.save || options.strict {
        element.save()?;
    }
    if options.strict {
        for name in element
            .directory()
            .file_system()
            .check_for_modifications()?
        {
            if !header {
                eprintln!("{heading}");
                header = true;
            }
            if let Some(fp) = element.directory().abs_path(&name) {
                eprintln!(
                    "    - {}",
                    tr!(
                        "CommandLineInterface",
                        "Non-canonical file: '{0}'",
                        pretty(&fp, &options.library)
                    )
                );
            }
            success = false;
        }
    }
    if options.check && !element.metadata().is_deprecated() {
        let (_, messages) = findings(
            element.run_checks()?.iter().map(|m| m.to_message()),
            element.metadata().message_approvals(),
        );
        if !messages.is_empty() && !header {
            eprintln!("{heading}");
        }
        for message in messages {
            eprintln!("    - {message}");
            success = false;
        }
    }
    if options.save {
        element.directory().file_system().save()?;
    }
    Ok(success)
}
fn children<E: LibraryBaseElement>(
    library: &Library,
    options: &LibraryOptions,
    label: impl FnOnce(usize) -> String,
) -> Result<bool> {
    let mut success = true;
    let names = library.search_for_elements::<E>();
    // `label` is translated at each call site using the upstream string.
    println!("{}", label(names.len()));
    let root = path(&options.library)?;
    for name in names {
        let mut element = E::open(open_directory(&root.path_to(&name), options.save)?)?;
        success &= process(&mut element, options)?;
    }
    Ok(success)
}
pub fn run(options: LibraryOptions) -> Result<bool> {
    println!(
        "{}",
        tr!(
            "CommandLineInterface",
            "Open library '{0}'...",
            pretty(&path(&options.library)?, &options.library)
        )
    );
    let mut library = Library::open(open_directory(&path(&options.library)?, options.save)?)?;
    let mut success = process(&mut library, &options)?;
    if options.all {
        success &= children::<ComponentCategory>(&library, &options, |n| {
            tr!(
                "CommandLineInterface",
                "Process {0} component categories...",
                n
            )
        })?;
        success &= children::<PackageCategory>(&library, &options, |n| {
            tr!(
                "CommandLineInterface",
                "Process {0} package categories...",
                n
            )
        })?;
        success &= children::<Symbol>(&library, &options, |n| {
            tr!("CommandLineInterface", "Process {0} symbols...", n)
        })?;
        success &= children::<Package>(&library, &options, |n| {
            tr!("CommandLineInterface", "Process {0} packages...", n)
        })?;
        success &= children::<Component>(&library, &options, |n| {
            tr!("CommandLineInterface", "Process {0} components...", n)
        })?;
        success &= children::<Device>(&library, &options, |n| {
            tr!("CommandLineInterface", "Process {0} devices...", n)
        })?;
        success &= children::<Organization>(&library, &options, |n| {
            tr!("CommandLineInterface", "Process {0} organizations...", n)
        })?;
    }
    Ok(success)
}
fn element<E: LibraryBaseElement>(element: &E, check: bool) -> Result<bool> {
    let mut success = true;
    if check {
        println!("{}", tr!("CommandLineInterface", "Run checks..."));
        let (approved, messages) = findings(
            element.run_checks()?.iter().map(|m| m.to_message()),
            element.metadata().message_approvals(),
        );
        summary("  ", approved, messages.len());
        for message in messages {
            eprintln!("    - {message}");
            success = false;
        }
    }
    Ok(success)
}
fn filename(template: &str, lookup: impl FnMut(&str) -> Option<String>) -> String {
    librepcb_core::attribute::substitute(
        template,
        lookup,
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
    )
}
fn export_drawing(
    drawing: librepcb_graphics::Drawing,
    settings: librepcb_graphics::GraphicsSettings,
    title: String,
    dest: &str,
) -> Result<Vec<FilePath>> {
    let exporter = librepcb_graphics::GraphicsExport {
        title,
        creator: format!("LibrePCB {}", env!("CARGO_PKG_VERSION")),
        ..Default::default()
    };
    Ok(exporter.write(
        &[librepcb_graphics::GraphicsPage { drawing, settings }],
        &path(dest)?,
    )?)
}
pub fn symbol(options: ElementOptions) -> Result<bool> {
    println!(
        "{}",
        tr!(
            "CommandLineInterface",
            "Open symbol '{0}'...",
            pretty(&path(&options.element)?, &options.element)
        )
    );
    let symbol = Symbol::open(open_directory(&path(&options.element)?, false)?)?;
    let mut success = element(&symbol, options.check)?;
    if let Some(template) = &options.export {
        println!(
            "{}",
            tr!(
                "CommandLineInterface",
                "Export symbol to '{0}'...",
                template
            )
        );
        let dest = filename(template, |key| match key {
            "SYMBOL" => Some(symbol.metadata().names().default_value().to_string()),
            "SYMBOL_UUID" => Some(symbol.metadata().uuid().to_string()),
            _ => None,
        });
        let settings = librepcb_graphics::GraphicsSettings {
            margins: [0.; 4],
            ..Default::default()
        };
        let result = librepcb_graphics::symbol_drawing(&symbol, &settings)
            .map_err(crate::error::Error::from)
            .and_then(|drawing| {
                export_drawing(
                    drawing,
                    settings,
                    symbol.metadata().names().default_value().to_string(),
                    &dest,
                )
            });
        match result {
            Ok(files) => {
                for file in files {
                    println!("  => '{}'", pretty(&file, &dest));
                }
            }
            Err(e) => {
                eprintln!("  {}", tr!("CommandLineInterface", "ERROR: {0}", e));
                success = false;
            }
        }
    }
    Ok(success)
}
pub fn package(options: ElementOptions) -> Result<bool> {
    println!(
        "{}",
        tr!(
            "CommandLineInterface",
            "Open package '{0}'...",
            pretty(&path(&options.element)?, &options.element)
        )
    );
    let package = Package::open(open_directory(&path(&options.element)?, false)?)?;
    let mut success = element(&package, options.check)?;
    let mut written = std::collections::BTreeMap::new();
    if let Some(template) = &options.export {
        println!(
            "{}",
            tr!(
                "CommandLineInterface",
                "Export footprint(s) to '{0}'...",
                template
            )
        );
        for (index, footprint) in package.footprints().iter().enumerate() {
            let dest = filename(template, |key| match key {
                "PACKAGE" => Some(package.metadata().names().default_value().to_string()),
                "PACKAGE_UUID" => Some(package.metadata().uuid().to_string()),
                "FOOTPRINT" => Some(footprint.names().default_value().to_string()),
                "FOOTPRINT_UUID" => Some(footprint.uuid().to_string()),
                "FOOTPRINT_INDEX" => Some((index + 1).to_string()),
                _ => None,
            });
            let settings = librepcb_graphics::GraphicsSettings {
                margins: [0.; 4],
                ..Default::default()
            };
            let result = librepcb_graphics::footprint_drawing(footprint, &settings)
                .map_err(crate::error::Error::from)
                .and_then(|drawing| {
                    export_drawing(
                        drawing,
                        settings,
                        format!(
                            "{} ({})",
                            package.metadata().names().default_value(),
                            footprint.names().default_value()
                        ),
                        &dest,
                    )
                });
            match result {
                Ok(files) => {
                    for file in files {
                        println!("  => '{}'", pretty(&file, &dest));
                        *written.entry(file).or_insert(0) += 1;
                    }
                }
                Err(e) => {
                    eprintln!("  {}", tr!("CommandLineInterface", "ERROR: {0}", e));
                    success = false;
                }
            }
        }
    }
    let mut overwritten = false;
    for (file, count) in written {
        if count > 1 {
            eprintln!(
                "{}",
                tr!(
                    "CommandLineInterface",
                    "ERROR: The file '{0}' was written multiple times!",
                    pretty(&file, &options.element)
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
                "NOTE: To avoid writing files multiple times, make sure to pass unique filepaths to all export functions. For footprint output files, you could add a placeholder like '{0}' to the path.",
                "{{FOOTPRINT}}"
            )
        );
        success = false;
    }
    Ok(success)
}
