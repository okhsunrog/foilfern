//! Port of apps/librepcb-cli (main.cpp, commandlineinterface.{h,cpp}): the
//! LibrePCB command line interface.
//!
//! The output (messages, formatting, exit codes) is identical to upstream
//! where possible, since scripts and the upstream CLI tests (`tests/cli`)
//! compare it verbatim. Differences (see COMPAT.md):
//! - Graphics exports (schematics, symbols, footprints, graphics output
//!   jobs) use the Rust graphics backends; files differ from Qt's bytes.
//! - Mechanical STEP jobs and standalone STEP operations use the shared
//!   pure-Rust CAD backend, without OpenCascade.
//! - `--version` prints no Qt and OpenCascade versions.
//! - `--verbose` logging uses the `log`/`env_logger` format.

mod args;
mod drc;
mod error;
mod help;
mod library;
mod output;
mod project;
mod step;

use std::process::ExitCode;

use clap::FromArgMatches;
use clap::parser::ValueSource;
use librepcb_core::application;
use librepcb_i18n::tr;

use crate::args::{COMMANDS, Cli, Command, TR};
use crate::output::{print, print_err};

/// Version of this application (upstream `Application::getVersion()`).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    ExitCode::from(execute(&args))
}

/// Returns the command (the first positional argument), like the first
/// parse pass of upstream (which only knows the global flags, so every
/// argument not starting with `-` is positional).
fn find_command(args: &[String]) -> Option<usize> {
    let mut options_ended = false;
    for (i, arg) in args.iter().enumerate().skip(1) {
        if options_ended || arg == "-" || !arg.starts_with('-') {
            return Some(i);
        }
        if arg == "--" {
            options_ended = true;
        }
    }
    None
}

/// Runs the CLI and returns the exit code (upstream `execute()`):
/// 0 = success, 1 = errors, 2 = warnings (deprecated options used).
fn execute(args: &[String]) -> u8 {
    let executable = args.first().map(String::as_str).unwrap_or("librepcb-cli");
    let command_index = find_command(args);
    let command = command_index.map_or("", |i| args[i].as_str());

    // Build help texts.
    let main_help = {
        let mut text = help::help_text(
            executable,
            &tr!(TR, "LibrePCB Command Line Interface"),
            &args::global_options(),
            &args::help_positionals(""),
        );
        text.push('\n');
        text.push_str(&tr!(TR, "Commands:"));
        text.push('\n');
        for name in COMMANDS {
            text.push_str(&format!(
                "  {name:<15}{}\n",
                args::command_description(name)
            ));
        }
        text.push('\n');
        text.push_str(&tr!(TR, "List command-specific options:"));
        text.push_str(&format!("\n  {executable} <command> --help"));
        text
    };
    let usage = |help: &str| help.lines().next().unwrap_or_default().to_owned();
    let help_command_prefix = format!("{} ", tr!(TR, "Help:"));

    if !command.is_empty() && !COMMANDS.contains(&command) {
        print_err(&tr!(TR, "Unknown command '{0}'.", command));
        print_err(&usage(&main_help));
        print_err(&format!("{help_command_prefix}{executable} --help"));
        return 1;
    }

    // Make the help texts command-specific.
    let (help_text, help_command_text) = if command.is_empty() {
        (
            main_help,
            format!("{help_command_prefix}{executable} --help"),
        )
    } else {
        let mut options = args::global_options();
        options.extend(args::command_options(command));
        let text = help::help_text(
            executable,
            &tr!(TR, "LibrePCB Command Line Interface"),
            &options,
            &args::help_positionals(command),
        );
        (
            text.trim().to_owned(),
            format!("{help_command_prefix}{executable} {command} --help"),
        )
    };
    let usage_text = usage(&help_text);

    // Parse the arguments. Upstream accepts options before the command
    // name, so the command is moved to the front for clap.
    let mut clap_args: Vec<String> = args.to_vec();
    if let Some(i) = command_index {
        let cmd = clap_args.remove(i);
        clap_args.insert(1.min(clap_args.len()), cmd);
    }
    let parse_result = args::command()
        .try_get_matches_from(&clap_args)
        .and_then(|m| Cli::from_arg_matches(&m).map(|cli| (cli, m)));
    let (cli, matches) = match parse_result {
        Ok(result) => result,
        Err(e) => {
            // Report the error like Qt (e.g. all unknown options at once).
            let mut options = args::global_options();
            options.extend(args::command_options(command));
            let text =
                help::qt_parse_error(args, &options).unwrap_or_else(|| help::parse_error_text(&e));
            print_err(&text);
            print_err(&usage_text);
            print_err(&help_command_text);
            return 1;
        }
    };

    // --verbose
    init_logging(cli.verbose);

    // --help (also shown if no arguments supplied)
    if cli.help || args.len() <= 1 {
        print(&help_text);
        return 0;
    }

    // --version
    if cli.version {
        // Note: Not translated to be deterministic.
        print(&format!("LibrePCB CLI Version {APP_VERSION}"));
        print(&format!(
            "File Format {} {}",
            application::file_format_version(),
            if application::is_file_format_stable() {
                "(stable)"
            } else {
                "(unstable)"
            }
        ));
        print(&format!(
            "Git Revision {}",
            option_env!("LIBREPCB_GIT_REVISION").unwrap_or("unknown")
        ));
        print("Implementation librepcb-rs (Rust, no Qt)");
        print("OpenCascade N/A");
        return 0;
    }

    // Check the number of positional arguments.
    let positionals: &[String] = match &cli.command {
        Some(Command::Project(a)) => &a.positionals,
        Some(Command::Library(a)) => &a.positionals,
        Some(Command::Symbol(a)) => &a.positionals,
        Some(Command::Package(a)) => &a.positionals,
        Some(Command::Step(a)) => &a.positionals,
        None => &[],
    };
    let required: Vec<&str> = if command.is_empty() {
        vec!["command"]
    } else {
        vec![args::command_positional(command).0]
    };
    if positionals.len() < required.len() {
        print_err(&format!(
            "{} {}",
            tr!(TR, "Missing arguments:"),
            required[positionals.len()..].join(" ")
        ));
        print_err(&usage_text);
        print_err(&help_command_text);
        return 1;
    } else if positionals.len() > required.len() {
        print_err(&format!(
            "{} {}",
            tr!(TR, "Unknown arguments:"),
            positionals[required.len()..].join(" ")
        ));
        print_err(&usage_text);
        print_err(&help_command_text);
        return 1;
    }

    // Check for deprecated options.
    let mut used_deprecated = false;
    if !output::suppress_deprecation_warnings()
        && let Some(("open-project", sub)) = matches.subcommand()
    {
        for (name, replacement) in args::deprecated_options() {
            // The clap argument IDs are the field names.
            let id = name.replace('-', "_");
            if sub.value_source(&id) == Some(ValueSource::CommandLine) {
                output::print_deprecation_warning(
                    &format!("--{name}"),
                    &format!("--{replacement}"),
                );
                used_deprecated = true;
            }
        }
    }

    // Execute command.
    let success = match &cli.command {
        Some(Command::Project(a)) => project::open_project(a),
        Some(Command::Library(a)) => library::open_library(a),
        Some(Command::Symbol(a)) => library::open_symbol(a),
        Some(Command::Package(a)) => library::open_package(a),
        Some(Command::Step(a)) => library::open_step(a),
        None => {
            print_err("Internal failure."); // No tr() because this cannot occur.
            false
        }
    };

    // Report status with the exit code.
    if !success {
        print(&tr!(TR, "Finished with errors!"));
        1
    } else if used_deprecated {
        print(&tr!(TR, "Finished with warnings!"));
        2
    } else {
        print(&tr!(TR, "SUCCESS"));
        0
    }
}

/// Logging goes to stderr, only with `--verbose` (upstream silences all
/// but fatal messages, since the output may be parsed by scripts).
fn init_logging(verbose: bool) {
    let level = if verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Off
    };
    env_logger::Builder::new()
        .filter_level(level)
        .target(env_logger::Target::Stderr)
        .init();
}
