//! Library commands from apps/librepcb-cli/commandlineinterface.cpp.
use crate::{ElementOptions, LibraryOptions, error::Result, support::*};
use librepcb_core::{
    fileio::{FileSystem, TransactionalDirectory},
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
fn element<E: LibraryBaseElement>(directory: TransactionalDirectory, check: bool) -> Result<bool> {
    let element = E::open(directory)?;
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
pub fn symbol(options: ElementOptions) -> Result<bool> {
    println!(
        "{}",
        tr!(
            "CommandLineInterface",
            "Open symbol '{0}'...",
            pretty(&path(&options.element)?, &options.element)
        )
    );
    element::<Symbol>(
        open_directory(&path(&options.element)?, false)?,
        options.check,
    )
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
    element::<Package>(
        open_directory(&path(&options.element)?, false)?,
        options.check,
    )
}
