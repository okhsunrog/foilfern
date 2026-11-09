//! The `librepcb` binary: command line handling and the event loop.
//!
//! Port of apps/librepcb/main.cpp.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use librepcb_app::screenshot;
use librepcb_app::startup;
use librepcb_app::ui;
use librepcb_app::{App, InitialTab};
use slint::ComponentHandle;

/// Tab to show (screenshot mode or after opening a project).
#[derive(Debug, Clone, Copy, ValueEnum)]
enum TabArg {
    Home,
    Schematic,
    Board,
}

/// Side panel page to show.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum PanelArg {
    Home,
    Libraries,
    Documents,
    Layers,
    About,
    None,
}

/// LibrePCB, the electronic design automation software.
#[derive(Debug, Parser)]
#[command(name = "librepcb", version)]
struct Args {
    /// Workspace directory (default: like upstream LibrePCB).
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Project file (*.lpp) to open.
    #[arg(long)]
    project: Vec<PathBuf>,
    /// Files to open (projects).
    files: Vec<PathBuf>,
    /// Language (e.g. "de"; default: workspace setting or system).
    #[arg(long)]
    language: Option<String>,
    /// Tab to show after opening the project.
    #[arg(long, value_enum)]
    tab: Option<TabArg>,
    /// Side panel page to show.
    #[arg(long, value_enum)]
    panel: Option<PanelArg>,
    /// Render the window headless into this PNG file and exit.
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Window size for --screenshot, e.g. 1400x900.
    #[arg(long, default_value = "1400x900")]
    size: String,
    /// Start the embedded LibrePCB MCP server for AI agents (streamable
    /// HTTP at http://ADDR/mcp; default 127.0.0.1:8766).
    #[arg(
        long,
        value_name = "ADDR",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "127.0.0.1:8766"
    )]
    mcp: Option<std::net::SocketAddr>,
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w >= 200 && h >= 200 && w <= 8192 && h <= 8192).then_some((w, h))
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the initialize workspace wizard as often as needed until the
/// workspace is ready (upstream `openWorkspace()` of `main.cpp`); `None` if
/// the user canceled.
fn run_workspace_wizard(
    window: &ui::AppWindow,
    path: Option<PathBuf>,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    use librepcb_app::dialogs::initialize_workspace::InitializeWorkspaceWizard;
    use librepcb_app::modal_dialog::{ModalDialog, ModalResult};
    let mut path = path.and_then(|p| librepcb_app::app::absolute_file_path(&p));
    loop {
        let wizard = InitializeWorkspaceWizard::new(path.clone(), false);
        if !wizard.needs_to_be_shown() {
            return Ok(path.map(|p| p.as_path().to_path_buf()));
        }
        let dialog = ModalDialog::show(window, Box::new(wizard), || {
            let _ = slint::quit_event_loop();
        });
        window.run()?;
        match dialog.result() {
            ModalResult::Accepted(Some(librepcb_app::dialogs::AppRequest::WorkspaceChosen(
                chosen,
            ))) => {
                if let Err(e) = startup::set_most_recently_used_workspace(&chosen) {
                    log::warn!("Failed to store the workspace path: {e}");
                }
                path = Some(chosen);
            }
            _ => return Ok(None),
        }
    }
}

fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let headless = match &args.screenshot {
        Some(_) => {
            let (w, h) = parse_size(&args.size).ok_or("invalid --size, expected WxH")?;
            Some(screenshot::install_platform(w, h)?)
        }
        None => None,
    };

    let window = ui::AppWindow::new()?;
    let ws_path = match (&args.workspace, headless.is_some()) {
        // Explicit workspace (created without asking) or headless mode.
        (Some(_), _) | (None, true) => startup::workspace_path(args.workspace.as_deref())
            .ok_or("could not determine the workspace directory, pass --workspace")?,
        (None, false) => {
            if let Some(lang) = args.language.as_deref() {
                let _ = librepcb_i18n::set_language(lang);
            } else {
                librepcb_i18n::set_language_from_system();
            }
            match run_workspace_wizard(&window, startup::remembered_workspace_path())? {
                Some(path) => path,
                None => return Ok(()), // Canceled.
            }
        }
    };
    log::info!("Workspace: {}", ws_path.display());
    let workspace = startup::open_workspace(&ws_path)?;
    let language = startup::select_language(args.language.as_deref(), &workspace);

    // Slint requires a component to exist before selecting the language.
    if let Err(e) = slint::select_bundled_translation(language) {
        log::warn!("Failed to select the UI language {language}: {e}");
    }
    let app = App::new(window, workspace);

    let mut opened = false;
    for file in args.project.iter().chain(&args.files) {
        opened |= app.open_project(file).is_some();
    }
    if let Some(tab) = args.tab {
        app.show_tab(match tab {
            TabArg::Home => InitialTab::Home,
            TabArg::Schematic => InitialTab::Schematic,
            TabArg::Board => InitialTab::Board,
        });
    }
    if let Some(panel) = args.panel {
        app.show_panel(match panel {
            PanelArg::Home => ui::PanelPage::Home,
            PanelArg::Libraries => ui::PanelPage::Libraries,
            PanelArg::Documents => ui::PanelPage::Documents,
            PanelArg::Layers => ui::PanelPage::Layers,
            PanelArg::About => ui::PanelPage::About,
            PanelArg::None => ui::PanelPage::None,
        });
    }
    if !opened && (!args.project.is_empty() || !args.files.is_empty()) {
        log::warn!("No project could be opened.");
    }
    if let Some(addr) = args.mcp {
        let url = app.start_mcp_server(addr)?;
        log::info!("LibrePCB MCP server: {url}");
    }

    match (headless, &args.screenshot) {
        (Some(headless), Some(path)) => {
            app.window().show()?;
            headless.save_png(path)?;
            log::info!("Screenshot written to {}", path.display());
            Ok(())
        }
        _ => {
            app.window().run()?;
            Ok(())
        }
    }
}
