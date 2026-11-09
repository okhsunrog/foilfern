//! Command line arguments (the `QCommandLineOption`s of upstream
//! apps/librepcb-cli/commandlineinterface.cpp), parsed with `clap`.
//!
//! The help strings are translated at runtime (upstream `tr()`), so they are
//! attached in [`command()`] instead of doc comments; the help output itself
//! is formatted by [`crate::help`] like Qt does.

use std::collections::BTreeMap;

use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand};
use librepcb_i18n::tr;

use crate::help::{HelpOption, HelpPositional};

/// Translation context of the upstream class.
pub const TR: &str = "CommandLineInterface";

/// Supported graphics export file extensions shown in the help texts
/// (upstream `GraphicsExport::getSupportedExtensions()`, i.e. PDF, SVG and
/// the image formats of Qt; here those of the scene crate's export).
pub const GRAPHICS_EXTENSIONS: &[&str] = librepcb_scene::export::SUPPORTED_EXTENSIONS;

/// All arguments.
#[derive(Debug, Parser)]
#[command(
    name = "librepcb-cli",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_override_self = true
)]
pub struct Cli {
    /// `-h, --help`
    #[arg(short = 'h', long = "help", global = true, action = ArgAction::SetTrue)]
    pub help: bool,
    /// `-V, --version`
    #[arg(short = 'V', long = "version", global = true, action = ArgAction::SetTrue)]
    pub version: bool,
    /// `-v, --verbose`
    #[arg(short = 'v', long = "verbose", global = true, action = ArgAction::SetTrue)]
    pub verbose: bool,
    /// The command.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// The commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// `open-project`
    #[command(name = "open-project")]
    Project(Box<OpenProjectArgs>),
    /// `open-library`
    #[command(name = "open-library")]
    Library(OpenLibraryArgs),
    /// `open-symbol`
    #[command(name = "open-symbol")]
    Symbol(OpenSymbolArgs),
    /// `open-package`
    #[command(name = "open-package")]
    Package(OpenPackageArgs),
    /// `open-step`
    #[command(name = "open-step")]
    Step(OpenStepArgs),
}

/// Options of `open-project`.
#[derive(Debug, Args)]
pub struct OpenProjectArgs {
    #[arg(long = "erc")]
    pub erc: bool,
    #[arg(long = "drc")]
    pub drc: bool,
    #[arg(long = "drc-settings", allow_hyphen_values = true)]
    pub drc_settings: Option<String>,
    #[arg(long = "run-job", allow_hyphen_values = true)]
    pub run_job: Vec<String>,
    #[arg(long = "run-jobs")]
    pub run_jobs: bool,
    #[arg(long = "jobs", allow_hyphen_values = true)]
    pub jobs: Option<String>,
    #[arg(long = "outdir", allow_hyphen_values = true)]
    pub outdir: Option<String>,
    #[arg(long = "export-schematics", allow_hyphen_values = true)]
    pub export_schematics: Vec<String>,
    #[arg(long = "export-bom", allow_hyphen_values = true)]
    pub export_bom: Vec<String>,
    #[arg(long = "export-board-bom", allow_hyphen_values = true)]
    pub export_board_bom: Vec<String>,
    #[arg(long = "bom-attributes", allow_hyphen_values = true)]
    pub bom_attributes: Option<String>,
    #[arg(long = "export-pcb-fabrication-data")]
    pub export_pcb_fabrication_data: bool,
    #[arg(long = "pcb-fabrication-settings", allow_hyphen_values = true)]
    pub pcb_fabrication_settings: Option<String>,
    #[arg(long = "export-pnp-top", allow_hyphen_values = true)]
    pub export_pnp_top: Vec<String>,
    #[arg(long = "export-pnp-bottom", allow_hyphen_values = true)]
    pub export_pnp_bottom: Vec<String>,
    #[arg(long = "export-netlist", allow_hyphen_values = true)]
    pub export_netlist: Vec<String>,
    #[arg(long = "board", allow_hyphen_values = true)]
    pub board: Vec<String>,
    #[arg(long = "board-index", allow_hyphen_values = true)]
    pub board_index: Vec<String>,
    #[arg(long = "remove-other-boards")]
    pub remove_other_boards: bool,
    #[arg(long = "variant", allow_hyphen_values = true)]
    pub variant: Vec<String>,
    #[arg(long = "variant-index", allow_hyphen_values = true)]
    pub variant_index: Vec<String>,
    #[arg(long = "set-default-variant", allow_hyphen_values = true)]
    pub set_default_variant: Option<String>,
    #[arg(long = "save")]
    pub save: bool,
    #[arg(long = "strict")]
    pub strict: bool,
    /// Positional arguments (the project file).
    #[arg(hide = true)]
    pub positionals: Vec<String>,
}

/// Options of `open-library`.
#[derive(Debug, Args)]
pub struct OpenLibraryArgs {
    #[arg(long = "all")]
    pub all: bool,
    #[arg(long = "check")]
    pub check: bool,
    #[arg(long = "minify-step")]
    pub minify_step: bool,
    #[arg(long = "save")]
    pub save: bool,
    #[arg(long = "strict")]
    pub strict: bool,
    /// Positional arguments (the library directory).
    #[arg(hide = true)]
    pub positionals: Vec<String>,
}

/// Options of `open-symbol`.
#[derive(Debug, Args)]
pub struct OpenSymbolArgs {
    #[arg(long = "check")]
    pub check: bool,
    #[arg(long = "export", allow_hyphen_values = true)]
    pub export: Option<String>,
    /// Positional arguments (the symbol directory).
    #[arg(hide = true)]
    pub positionals: Vec<String>,
}

/// Options of `open-package`.
#[derive(Debug, Args)]
pub struct OpenPackageArgs {
    #[arg(long = "check")]
    pub check: bool,
    #[arg(long = "export", allow_hyphen_values = true)]
    pub export: Option<String>,
    /// Positional arguments (the package directory).
    #[arg(hide = true)]
    pub positionals: Vec<String>,
}

/// Options of `open-step`.
#[derive(Debug, Args)]
pub struct OpenStepArgs {
    #[arg(long = "minify")]
    pub minify: bool,
    #[arg(long = "tesselate")]
    pub tesselate: bool,
    #[arg(long = "save-to", allow_hyphen_values = true)]
    pub save_to: Option<String>,
    /// Positional arguments (the STEP file).
    #[arg(hide = true)]
    pub positionals: Vec<String>,
}

/// Names of the commands, in the order of the help text (upstream `QMap`,
/// i.e. sorted).
pub const COMMANDS: &[&str] = &[
    "open-library",
    "open-package",
    "open-project",
    "open-step",
    "open-symbol",
];

/// Returns the description of a command.
pub fn command_description(command: &str) -> String {
    match command {
        "open-project" => tr!(TR, "Open a project to execute project-related tasks."),
        "open-library" => tr!(TR, "Open a library to execute library-related tasks."),
        "open-symbol" => tr!(TR, "Open a symbol to execute symbol-related tasks."),
        "open-package" => tr!(TR, "Open a package to execute package-related tasks."),
        "open-step" => tr!(
            TR,
            "Open a STEP model to execute STEP-related tasks outside of a library."
        ),
        _ => String::new(),
    }
}

/// Returns the positional argument of a command (name, description).
pub fn command_positional(command: &str) -> (&'static str, String) {
    match command {
        "open-project" => ("project", tr!(TR, "Path to project file (*.lpp[z]).")),
        "open-library" => ("library", tr!(TR, "Path to library directory (*.lplib).")),
        "open-symbol" => (
            "symbol",
            tr!(TR, "Path to symbol directory (containing *.lp)."),
        ),
        "open-package" => (
            "package",
            tr!(TR, "Path to package directory (containing *.lp)."),
        ),
        _ => ("file", tr!(TR, "Path to the STEP file ({0}).", "*.step")),
    }
}

/// Deprecated options of `open-project` and their replacements (upstream
/// `QMap`, i.e. sorted by name).
pub fn deprecated_options() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("export-schematics", "run-jobs"),
        ("export-bom", "run-jobs"),
        ("export-board-bom", "run-jobs"),
        ("bom-attributes", "run-jobs"),
        ("export-pcb-fabrication-data", "run-jobs"),
        ("pcb-fabrication-settings", "jobs"),
        ("export-pnp-top", "run-jobs"),
        ("export-pnp-bottom", "run-jobs"),
        ("export-netlist", "run-jobs"),
    ])
}

/// Returns the global options (help texts).
pub fn global_options() -> Vec<HelpOption> {
    vec![
        option(&["h", "help"], None, tr!(TR, "Print this message.")),
        option(
            &["V", "version"],
            None,
            tr!(TR, "Displays version information."),
        ),
        option(&["v", "verbose"], None, tr!(TR, "Verbose output.")),
    ]
}

fn option(names: &[&str], value_name: Option<String>, description: String) -> HelpOption {
    HelpOption {
        names: names.iter().map(|s| (*s).to_owned()).collect(),
        value_name,
        description,
    }
}

/// Returns the options of a command (help texts), in the order of the help
/// text. Their names must match the `clap` definitions above (checked by a
/// test).
pub fn command_options(command: &str) -> Vec<HelpOption> {
    let file = || Some(tr!(TR, "file"));
    let name = || Some(tr!(TR, "name"));
    let index = || Some(tr!(TR, "index"));
    let graphics_ext = GRAPHICS_EXTENSIONS.join(", ");
    let mut options = match command {
        "open-project" => vec![
            option(
                &["erc"],
                None,
                tr!(
                    TR,
                    "Run the electrical rule check, print all non-approved warnings/errors and report failure (exit code = 1) if there are non-approved messages."
                ),
            ),
            option(
                &["drc"],
                None,
                tr!(
                    TR,
                    "Run the design rule check, print all non-approved warnings/errors and report failure (exit code = 1) if there are non-approved messages."
                ),
            ),
            option(
                &["drc-settings"],
                file(),
                tr!(
                    TR,
                    "Override DRC settings by providing a *.lp file containing custom settings. If not set, the settings from the boards will be used instead."
                ),
            ),
            option(
                &["run-job"],
                name(),
                tr!(
                    TR,
                    "Run a particular output job. Can be given multiple times to run multiple jobs."
                ),
            ),
            option(
                &["run-jobs"],
                None,
                tr!(TR, "Run all existing output jobs."),
            ),
            option(
                &["jobs"],
                file(),
                tr!(
                    TR,
                    "Override output jobs with a *.lp file containing custom jobs. If not set, the jobs from the project will be used instead."
                ),
            ),
            option(
                &["outdir"],
                Some(tr!(TR, "path")),
                tr!(
                    TR,
                    "Override the output base directory of jobs. If not set, the standard output directory from the project is used."
                ),
            ),
            option(
                &["export-schematics"],
                file(),
                tr!(
                    TR,
                    "Export schematics to given file(s). Existing files will be overwritten. Supported file extensions: {0}",
                    graphics_ext
                ),
            ),
            option(
                &["export-bom"],
                file(),
                tr!(
                    TR,
                    "Export generic BOM to given file(s). Existing files will be overwritten. Supported file extensions: {0}",
                    "csv"
                ),
            ),
            option(
                &["export-board-bom"],
                file(),
                tr!(
                    TR,
                    "Export board-specific BOM to given file(s). Existing files will be overwritten. Supported file extensions: {0}",
                    "csv"
                ),
            ),
            option(
                &["bom-attributes"],
                Some(tr!(TR, "attributes")),
                tr!(
                    TR,
                    "Comma-separated list of additional attributes to be exported to the BOM. Example: \"{0}\"",
                    "SUPPLIER, SKU"
                ),
            ),
            option(
                &["export-pcb-fabrication-data"],
                None,
                tr!(
                    TR,
                    "Export PCB fabrication data (Gerber/Excellon) according the fabrication output settings of boards. Existing files will be overwritten."
                ),
            ),
            option(
                &["pcb-fabrication-settings"],
                file(),
                tr!(
                    TR,
                    "Override PCB fabrication output settings by providing a *.lp file containing custom settings. If not set, the settings from the boards will be used instead."
                ),
            ),
            option(
                &["export-pnp-top"],
                file(),
                tr!(
                    TR,
                    "Export pick&place file for automated assembly of the top board side. Existing files will be overwritten. Supported file extensions: {0}",
                    "csv, gbr"
                ),
            ),
            option(
                &["export-pnp-bottom"],
                file(),
                tr!(
                    TR,
                    "Export pick&place file for automated assembly of the bottom board side. Existing files will be overwritten. Supported file extensions: {0}",
                    "csv, gbr"
                ),
            ),
            option(
                &["export-netlist"],
                file(),
                tr!(
                    TR,
                    "Export netlist file for automated PCB testing. Existing files will be overwritten. Supported file extensions: {0}",
                    "d356"
                ),
            ),
            option(
                &["board"],
                name(),
                tr!(
                    TR,
                    "The name of the board(s) to export. Can be given multiple times. If not set, all boards are exported."
                ),
            ),
            option(
                &["board-index"],
                index(),
                tr!(
                    TR,
                    "Same as '{0}', but allows to specify boards by index instead of by name.",
                    "--board"
                ),
            ),
            option(
                &["remove-other-boards"],
                None,
                tr!(
                    TR,
                    "Remove all boards not specified with '{0}' from the project before executing all the other actions. If '{0}' is not passed, all boards will be removed. Pass '{1}' to save the modified project to disk.",
                    "--board[-index]",
                    "--save"
                ),
            ),
            option(
                &["variant"],
                name(),
                tr!(
                    TR,
                    "The name of the assembly variant(s) to export. Can be given multiple times. If not set, all assembly variants are exported."
                ),
            ),
            option(
                &["variant-index"],
                index(),
                tr!(
                    TR,
                    "Same as '{0}', but allows to specify assembly variants by index instead of by name.",
                    "--variant"
                ),
            ),
            option(
                &["set-default-variant"],
                name(),
                tr!(
                    TR,
                    "Move the specified assembly variant to the top before executing all the other actions. Pass '{0}' to save the modified project to disk.",
                    "--save"
                ),
            ),
            option(
                &["save"],
                None,
                tr!(
                    TR,
                    "Save project before closing it (useful to upgrade file format)."
                ),
            ),
            option(
                &["strict"],
                None,
                tr!(
                    TR,
                    "Fail if the project files are not strictly canonical, i.e. there would be changes when saving the project. Note that this option is not available for *.lppz files."
                ),
            ),
        ],
        "open-library" => vec![
            option(
                &["all"],
                None,
                tr!(
                    TR,
                    "Perform the selected action(s) on all elements contained in the opened library."
                ),
            ),
            option(
                &["check"],
                None,
                tr!(
                    TR,
                    "Run the library element check, print all non-approved messages and report failure (exit code = 1) if there are non-approved messages."
                ),
            ),
            option(
                &["minify-step"],
                None,
                tr!(
                    TR,
                    "Minify the STEP models of all packages. Only works in conjunction with '--all'. Pass '--save' to write the minified files to disk."
                ),
            ),
            option(
                &["save"],
                None,
                tr!(
                    TR,
                    "Save library (and contained elements if '--all' is given) before closing them (useful to upgrade file format)."
                ),
            ),
            option(
                &["strict"],
                None,
                tr!(
                    TR,
                    "Fail if the opened files are not strictly canonical, i.e. there would be changes when saving the library elements."
                ),
            ),
        ],
        "open-symbol" => vec![
            option(
                &["check"],
                None,
                tr!(
                    TR,
                    "Run the symbol check, print all non-approved messages and report failure (exit code = 1) if there are non-approved messages."
                ),
            ),
            option(
                &["export"],
                file(),
                tr!(
                    TR,
                    "Export the symbol to a graphical file. Supported file extensions: {0}",
                    graphics_ext
                ),
            ),
        ],
        "open-package" => vec![
            option(
                &["check"],
                None,
                tr!(
                    TR,
                    "Run the package check, print all non-approved messages and report failure (exit code = 1) if there are non-approved messages."
                ),
            ),
            option(
                &["export"],
                file(),
                tr!(
                    TR,
                    "Export the contained footprint(s) to a graphical file. Supported file extensions: {0}",
                    graphics_ext
                ),
            ),
        ],
        "open-step" => vec![
            option(
                &["minify"],
                None,
                tr!(
                    TR,
                    "Minify the STEP model before validating it. Use in conjunction with '{0}' to save the output of the operation.",
                    "--save-to"
                ),
            ),
            option(
                &["tesselate"],
                None,
                tr!(
                    TR,
                    "Tesselate the loaded STEP model to check if LibrePCB is able to render it. Reports failure (exit code = 1) if no content is detected."
                ),
            ),
            option(
                &["save-to"],
                file(),
                tr!(
                    TR,
                    "Write the (modified) STEP file to this output location (may be equal to the opened file path). Only makes sense in conjunction with '{0}'.",
                    "--minify"
                ),
            ),
        ],
        _ => Vec::new(),
    };

    // Mark deprecated options.
    if command == "open-project" {
        let deprecations = deprecated_options();
        for option in &mut options {
            if let Some(replacement) = deprecations.get(option.names[0].as_str()) {
                option.description = format!(
                    "[{} --{replacement}] {}",
                    tr!(TR, "Deprecated, replaced by:").to_uppercase(),
                    option.description
                );
            }
        }
    }
    options
}

/// Returns the positional arguments of the help text of a command (or of
/// the main help text if `command` is empty).
pub fn help_positionals(command: &str) -> Vec<HelpPositional> {
    if command.is_empty() {
        vec![HelpPositional {
            name: "command".into(),
            description: tr!(TR, "The command to execute (see list below)."),
            syntax: "command".into(),
        }]
    } else {
        let (name, description) = command_positional(command);
        vec![
            HelpPositional {
                name: command.into(),
                description: command_description(command),
                // No tr()!
                syntax: format!("{command} [command_options]"),
            },
            HelpPositional {
                name: name.into(),
                description,
                syntax: name.into(),
            },
        ]
    }
}

/// Returns the `clap` command.
pub fn command() -> clap::Command {
    Cli::command()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The help texts must list exactly the options known to `clap`.
    #[test]
    fn help_options_match_clap() {
        let cmd = command();
        for name in COMMANDS {
            let sub = cmd.find_subcommand(name).unwrap();
            let clap_names: Vec<&str> = sub
                .get_arguments()
                .filter_map(|a| a.get_long())
                .filter(|l| !["help", "version", "verbose"].contains(l))
                .collect();
            let help_names: Vec<String> = command_options(name)
                .into_iter()
                .map(|o| o.names[0].clone())
                .collect();
            assert_eq!(clap_names, help_names, "{name}");
            for option in command_options(name) {
                let arg = sub
                    .get_arguments()
                    .find(|a| a.get_long() == Some(option.names[0].as_str()))
                    .unwrap();
                assert_eq!(
                    arg.get_action().takes_values(),
                    option.value_name.is_some(),
                    "{}",
                    option.names[0]
                );
            }
        }
    }

    #[test]
    fn clap_definition_is_valid() {
        command().debug_assert();
    }
}
