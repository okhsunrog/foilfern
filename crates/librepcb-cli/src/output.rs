//! Console output helpers of upstream `CommandLineInterface` (`print()`,
//! `printErr()`, `prettyPath()`, `prepareRuleCheckMessages()`, ...).

use std::collections::BTreeSet;
use std::path::Path;

use librepcb_core::application;
use librepcb_core::fileio::FilePath;
use librepcb_core::rule_check::RuleCheckMessage;
use librepcb_core::serialization::SExpression;
use librepcb_core::utils::toolbox::compare_numeric;
use librepcb_i18n::tr;

use crate::args::TR;

/// Prints a line to stdout.
pub fn print(s: &str) {
    println!("{s}");
}

/// Prints a line to stderr.
pub fn print_err(s: &str) {
    eprintln!("{s}");
}

/// Returns the current working directory (upstream `QDir::currentPath()`).
pub fn current_dir() -> FilePath {
    std::env::current_dir()
        .ok()
        .and_then(FilePath::new)
        .or_else(|| FilePath::new("/"))
        // Invariant: "/" is a valid absolute path.
        .expect("root is a valid path")
}

/// Returns the absolute, cleaned path of a path given on the command line
/// (upstream `QFileInfo(path).absoluteFilePath()`).
pub fn absolute_path(path: &str) -> FilePath {
    if Path::new(path).is_absolute() {
        FilePath::new(path).unwrap_or_else(current_dir)
    } else {
        current_dir().path_to(path)
    }
}

/// Formats a path for the console output in the same style (absolute or
/// relative to the current directory) as the path `style` given by the
/// user (upstream `prettyPath()`).
pub fn pretty_path(path: &FilePath, style: &str) -> String {
    let cwd = current_dir();
    if Path::new(style).is_absolute() {
        path.to_native()
    } else if *path == cwd {
        path.file_name().to_owned()
    } else {
        path.to_relative_native(&cwd)
    }
}

/// Sorts rule check messages (severity descending, then message in natural
/// order) and returns the non-approved ones formatted for the console,
/// together with the number of approved messages (upstream
/// `prepareRuleCheckMessages()`).
pub fn prepare_rule_check_messages(
    mut messages: Vec<RuleCheckMessage>,
    approvals: &BTreeSet<SExpression>,
) -> (Vec<String>, usize) {
    messages.sort_by(|a, b| {
        b.severity()
            .cmp(&a.severity())
            .then_with(|| compare_numeric(a.message(), b.message()))
    });
    let mut approved = 0;
    let mut printed = Vec::new();
    for msg in &messages {
        if approvals.contains(msg.approval()) {
            approved += 1;
        } else {
            printed.push(format!(
                "[{}] {}",
                msg.severity().name_tr().to_uppercase(),
                msg.message()
            ));
        }
    }
    (printed, approved)
}

/// Returns the summary lines of a check (upstream `formatCheckSummary()`).
pub fn format_check_summary(approved: usize, non_approved: usize, indent: &str) -> [String; 2] {
    [
        format!("{indent}{}", tr!(TR, "Approved messages: {0}", approved)),
        format!(
            "{indent}{}",
            tr!(TR, "Non-approved messages: {0}", non_approved)
        ),
    ]
}

/// Prints an error and returns `true` if saving is not allowed because the
/// file format is unstable (upstream `failIfFileFormatUnstable()`).
pub fn fail_if_file_format_unstable() -> bool {
    if !application::is_file_format_stable()
        && std::env::var("LIBREPCB_DISABLE_UNSTABLE_WARNING").as_deref() != Ok("1")
    {
        print_err(&tr!(
            TR,
            "This application version is UNSTABLE! Option '{0}' is disabled to avoid breaking projects or libraries. Please use a stable release instead.",
            "--save"
        ));
        true
    } else {
        log::info!(
            "Application version is unstable, but warning is disabled with environment variable LIBREPCB_DISABLE_UNSTABLE_WARNING."
        );
        false
    }
}

/// Whether deprecation warnings are suppressed by the environment.
pub fn suppress_deprecation_warnings() -> bool {
    std::env::var("LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS").as_deref() == Ok("1")
}

/// Prints a deprecation warning (upstream `printDeprecationWarning()`).
pub fn print_deprecation_warning(deprecated: &str, replacement: &str) {
    let mut s = format!("{}: ", tr!(TR, "Warning").to_uppercase());
    s += &tr!(
        TR,
        "The command or option '{0}' is deprecated and will be removed in a future release.",
        deprecated
    );
    if !replacement.is_empty() {
        s += " ";
        s += &tr!(
            TR,
            "Please see '{0}' for a possible replacement.",
            replacement
        );
    }
    s += " ";
    s += &tr!(
        TR,
        "For now, the command will be executed, but the CLI will return with a nonzero exit code. As a temporary workaround, this warning and the nonzero exit code can be suppressed with the environment variable '{0}'.",
        "LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS=1"
    );
    print_err(&s);
}
