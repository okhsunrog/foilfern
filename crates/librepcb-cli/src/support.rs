//! Shared CLI path and check formatting from apps/librepcb-cli/commandlineinterface.cpp.
use crate::error::{Error, Result};
use librepcb_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils},
    rule_check::RuleCheckMessage,
    serialization::{Mode, SExpression},
};
use librepcb_i18n::tr;
use std::{collections::BTreeSet, path::Path};

pub fn path(value: &str) -> Result<FilePath> {
    FilePath::new(std::env::current_dir()?.join(value)).ok_or_else(|| {
        Error::Message(tr!(
            "CommandLineInterface",
            "Invalid file path: '{0}'",
            value
        ))
    })
}
pub fn pretty(path: &FilePath, style: &str) -> String {
    if Path::new(style).is_absolute() {
        path.to_native()
    } else {
        std::env::current_dir()
            .ok()
            .and_then(FilePath::new)
            .map(|cwd| {
                if *path == cwd {
                    path.file_name().to_owned()
                } else {
                    path.to_relative_native(&cwd)
                }
            })
            .unwrap_or_else(|| path.to_native())
    }
}
pub fn open_directory(path: &FilePath, save: bool) -> Result<TransactionalDirectory> {
    let fs = if save {
        TransactionalFileSystem::open_rw(path)?
    } else {
        TransactionalFileSystem::open_ro(path)?
    };
    Ok(TransactionalDirectory::new(std::sync::Arc::new(fs), ""))
}
pub fn parse_file(value: &str) -> Result<SExpression> {
    let path = path(value)?;
    Ok(SExpression::parse(
        &file_utils::read_file(&path)?,
        Some(path.as_ref()),
        Mode::LibrePcb,
    )?)
}
pub fn findings(
    messages: impl IntoIterator<Item = RuleCheckMessage>,
    approvals: &BTreeSet<SExpression>,
) -> (usize, Vec<String>) {
    let mut messages: Vec<_> = messages.into_iter().collect();
    messages.sort_by(|a, b| {
        b.severity()
            .cmp(&a.severity())
            .then_with(|| librepcb_core::utils::toolbox::compare_numeric(a.message(), b.message()))
    });
    let mut approved = 0;
    let mut remaining = Vec::new();
    for m in messages {
        if approvals.contains(m.approval()) {
            approved += 1;
        } else {
            remaining.push(format!(
                "[{}] {}",
                m.severity().name_tr().to_uppercase(),
                m.message()
            ));
        }
    }
    (approved, remaining)
}
pub fn summary(indent: &str, approved: usize, non_approved: usize) {
    println!(
        "{indent}{}",
        tr!("CommandLineInterface", "Approved messages: {0}", approved)
    );
    println!(
        "{indent}{}",
        tr!(
            "CommandLineInterface",
            "Non-approved messages: {0}",
            non_approved
        )
    );
}

/// Upstream executes deprecated options but reports failure unless explicitly
/// suppressed, allowing build scripts to migrate without losing their output.
pub fn deprecations(options: &crate::ProjectOptions) -> bool {
    if std::env::var("LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS").is_ok_and(|v| v == "1") {
        return false;
    }
    let mut used = false;
    for (enabled, old, new) in [
        (
            options.bom_attributes.is_some(),
            "bom-attributes",
            "run-jobs",
        ),
        (
            !options.export_board_bom.is_empty(),
            "export-board-bom",
            "run-jobs",
        ),
        (!options.export_bom.is_empty(), "export-bom", "run-jobs"),
        (
            !options.export_netlist.is_empty(),
            "export-netlist",
            "run-jobs",
        ),
        (
            !options.export_schematics.is_empty(),
            "export-schematics",
            "run-jobs",
        ),
        (
            options.export_pcb_fabrication_data,
            "export-pcb-fabrication-data",
            "run-jobs",
        ),
        (
            !options.export_pnp_bottom.is_empty(),
            "export-pnp-bottom",
            "run-jobs",
        ),
        (
            !options.export_pnp_top.is_empty(),
            "export-pnp-top",
            "run-jobs",
        ),
        (
            options.pcb_fabrication_settings.is_some(),
            "pcb-fabrication-settings",
            "jobs",
        ),
    ] {
        if enabled {
            used = true;
            eprintln!(
                "{}: {} {} {}",
                tr!("CommandLineInterface", "Warning").to_uppercase(),
                tr!(
                    "CommandLineInterface",
                    "The command or option '{0}' is deprecated and will be removed in a future release.",
                    format!("--{old}")
                ),
                tr!(
                    "CommandLineInterface",
                    "Please see '{0}' for a possible replacement.",
                    format!("--{new}")
                ),
                tr!(
                    "CommandLineInterface",
                    "For now, the command will be executed, but the CLI will return with a nonzero exit code. As a temporary workaround, this warning and the nonzero exit code can be suppressed with the environment variable '{0}'.",
                    "LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS=1"
                )
            );
        }
    }
    used
}
