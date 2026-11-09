//! Port of apps/librepcb-cli/{main,commandlineinterface}.cpp.
//! Uses clap for native argument parsing. Domain work stays in the synchronous
//! core; no Qt application or display server is needed.

mod error;
mod library;
mod project;
mod step;
mod support;

use clap::{Args, Parser, Subcommand};
use error::Result;
use librepcb_i18n::tr;

#[derive(Parser)]
#[command(
    name = "librepcb-cli",
    version,
    about = tr!("CommandLineInterface","LibrePCB Command Line Interface")
)]
struct Cli {
    /// Verbose diagnostic output.
    #[arg(short = 'v', long, global = true, help=tr!("CommandLineInterface","Verbose output."))]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Open, check, export or save a project (*.lpp or *.lppz).
    #[command(name = "open-project", about=tr!("CommandLineInterface","Open, check, export or save a project (*.lpp or *.lppz)."))]
    Project(Box<ProjectOptions>),
    /// Open, check or save a library (*.lplib).
    #[command(name = "open-library", about=tr!("CommandLineInterface","Open, check or save a library (*.lplib)."))]
    Library(LibraryOptions),
    /// Check a library symbol.
    #[command(name = "open-symbol", about=tr!("CommandLineInterface","Check a library symbol."))]
    Symbol(ElementOptions),
    /// Check a library package.
    #[command(name = "open-package", about=tr!("CommandLineInterface","Check a library package."))]
    Package(ElementOptions),
    /// Load and validate a STEP model without a display server.
    #[command(name = "open-step")]
    Step(StepOptions),
}
#[derive(Args)]
struct StepOptions {
    file: String,
    /// Remove duplicate STEP entities while retaining representations and styles.
    #[arg(long)]
    minify: bool,
    /// Tessellate the model on the CPU and report vertex/color counts.
    #[arg(long)]
    tesselate: bool,
    /// Save the input (after optional minification) before validation.
    #[arg(long)]
    save_to: Option<String>,
}
#[derive(Args)]
struct ProjectOptions {
    project: String,
    #[arg(long, help=tr!("CommandLineInterface","Run the electrical rule check and fail on non-approved messages."))]
    erc: bool,
    #[arg(long, help=tr!("CommandLineInterface","Run the design rule check and fail on non-approved messages."))]
    drc: bool,
    #[arg(long, help=tr!("CommandLineInterface","Override DRC settings with a *.lp file."))]
    drc_settings: Option<String>,
    #[arg(long, help=tr!("CommandLineInterface","Run a named output job. Can be repeated."))]
    run_job: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Run all existing output jobs."))]
    run_jobs: bool,
    #[arg(long, help=tr!("CommandLineInterface","Override output jobs with a *.lp file."))]
    jobs: Option<String>,
    #[arg(long, help=tr!("CommandLineInterface","Override the output base directory of jobs."))]
    outdir: Option<String>,
    #[arg(long, help=tr!("CommandLineInterface","Select boards by name. Can be repeated."))]
    board: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Select boards by index. Can be repeated."))]
    board_index: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Select assembly variants by name. Can be repeated."))]
    variant: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Select assembly variants by index. Can be repeated."))]
    variant_index: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Remove unselected boards before checks and exports. Use --save to persist."))]
    remove_other_boards: bool,
    #[arg(long, help=tr!("CommandLineInterface","Move the named assembly variant to the first position. Use --save to persist."))]
    set_default_variant: Option<String>,
    #[arg(long, help=tr!("CommandLineInterface","Save changes and file format upgrades to disk."))]
    save: bool,
    #[arg(long, help=tr!("CommandLineInterface","Fail if files would change on save."))]
    strict: bool,
    #[arg(long, help=tr!("CommandLineInterface","Export a generic CSV bill of materials (deprecated; use output jobs)."))]
    export_bom: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Export schematics as PDF, SVG or PNG (deprecated; use output jobs)."))]
    export_schematics: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Export board-specific CSV bills of materials (deprecated; use output jobs)."))]
    export_board_bom: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Comma-separated custom BOM columns (deprecated; use output jobs)."))]
    bom_attributes: Option<String>,
    #[arg(long, help=tr!("CommandLineInterface","Export board Gerber/Excellon files (deprecated; use output jobs)."))]
    export_pcb_fabrication_data: bool,
    #[arg(long, help=tr!("CommandLineInterface","Override fabrication settings with a *.lp file (deprecated; use output jobs)."))]
    pcb_fabrication_settings: Option<String>,
    #[arg(long, help=tr!("CommandLineInterface","Export top assembly data to CSV or Gerber X3 (deprecated; use output jobs)."))]
    export_pnp_top: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Export bottom assembly data to CSV or Gerber X3 (deprecated; use output jobs)."))]
    export_pnp_bottom: Vec<String>,
    #[arg(long, help=tr!("CommandLineInterface","Export IPC-D-356A netlists (deprecated; use output jobs)."))]
    export_netlist: Vec<String>,
}
#[derive(Args)]
struct LibraryOptions {
    library: String,
    #[arg(long, help=tr!("CommandLineInterface","Process all library elements."))]
    all: bool,
    #[arg(long, help=tr!("CommandLineInterface","Run checks and fail on non-approved messages."))]
    check: bool,
    #[arg(long, help=tr!("CommandLineInterface","Minify STEP models contained in packages."))]
    minify_step: bool,
    #[arg(long, help=tr!("CommandLineInterface","Save changes and file format upgrades to disk."))]
    save: bool,
    #[arg(long, help=tr!("CommandLineInterface","Fail if files would change on save."))]
    strict: bool,
}
#[derive(Args)]
struct ElementOptions {
    element: String,
    #[arg(long, help=tr!("CommandLineInterface","Export as PDF, SVG or PNG."))]
    export: Option<String>,
    #[arg(long, help=tr!("CommandLineInterface","Run checks and fail on non-approved messages."))]
    check: bool,
}
fn run(cli: Cli) -> Result<(bool, bool)> {
    if cli.verbose {
        eprintln!(
            "{}",
            tr!(
                "CommandLineInterface",
                "Verbose output enabled. Reading native LibrePCB model and file format {0}; project and library operations use a transactional file system.",
                librepcb_core::application::file_format_version()
            )
        );
    }
    match cli.command {
        Command::Project(options) => {
            let deprecated = support::deprecations(&options);
            Ok((project::run(*options)?, deprecated))
        }
        Command::Library(options) => library::run(options).map(|success| (success, false)),
        Command::Symbol(options) => library::symbol(options).map(|success| (success, false)),
        Command::Package(options) => library::package(options).map(|success| (success, false)),
        Command::Step(options) => step::run(options).map(|success| (success, false)),
    }
}
fn main() -> std::process::ExitCode {
    let (success, warnings) = match run(Cli::parse()) {
        Ok(success) => success,
        Err(error) => {
            eprintln!("{}", tr!("CommandLineInterface", "ERROR: {0}", error));
            (false, false)
        }
    };
    println!(
        "{}",
        if success && warnings {
            tr!("CommandLineInterface", "Finished with warnings!")
        } else if success {
            tr!("CommandLineInterface", "SUCCESS")
        } else {
            tr!("CommandLineInterface", "Finished with errors!")
        }
    );
    if success && warnings {
        std::process::ExitCode::from(2)
    } else if success {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    #[test]
    fn valid_command_tree() {
        Cli::command().debug_assert();
    }
}
