//! The embedded MCP server, rule checks and outputs in the running
//! application (headless): an MCP client edits the open project over HTTP
//! while the UI follows the agent, updates the board scene from the change
//! journal and undoes the agent's edit; the ERC runs automatically, the DRC
//! runs in the background, exports write their files, and the agent closes
//! the project.
//!
//! Runs on the UI thread of the test binary
//! ([`with_headless()`](crate::common::with_headless)). Screenshots are
//! written to `CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use librepcb_app::screenshot::{Headless, write_png};
use librepcb_app::tabs::Tab;
use librepcb_app::{App, InitialTab, startup, ui};
use librepcb_scene::BoardObject;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

const WIDTH: u32 = 1200;
const HEIGHT: u32 = 800;
const TIMEOUT: Duration = Duration::from_secs(120);

fn upstream_project(name: &str) -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .join(name)
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn save(headless: &Headless, name: &str) {
    let pixels = headless.settle(30);
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    write_png(&path, WIDTH, HEIGHT, &pixels).unwrap();
    println!("screenshot: {}", path.display());
}

/// Runs MCP tool calls in a client thread (own tokio runtime) while the UI
/// thread processes events; returns the results.
fn agent(headless: &Headless, url: &str, calls: Vec<(&'static str, Value)>) -> Vec<Value> {
    let url = url.to_owned();
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let results = runtime.block_on(async {
            let client = ().serve(StreamableHttpClientTransport::from_uri(url)).await.unwrap();
            let mut results = Vec::new();
            for (tool, args) in calls {
                let args = match args {
                    Value::Object(map) => map,
                    _ => serde_json::Map::new(),
                };
                let res = client
                    .call_tool(CallToolRequestParams::new(tool).with_arguments(args))
                    .await
                    .unwrap_or_else(|e| panic!("{tool}: {e}"));
                let envelope = res.structured_content.clone().unwrap_or_default();
                assert_ne!(res.is_error, Some(true), "{tool} failed: {envelope:#}");
                results.push(envelope["result"].clone());
            }
            client.cancel().await.unwrap();
            results
        });
        flag.store(true, Ordering::SeqCst);
        results
    });
    assert!(
        headless.run_until(TIMEOUT, || done.load(Ordering::SeqCst)),
        "agent timed out"
    );
    let results = thread.join().unwrap();
    // Let the posted notices and deferred updates run.
    headless.settle(10);
    results
}

/// Number of vias in the scene of the board tab.
fn scene_vias(app: &App) -> usize {
    let state = app.state().borrow();
    state.sections()[0]
        .tabs()
        .iter()
        .find_map(|t| match t {
            Tab::Board2d(b) => b.scene(),
            _ => None,
        })
        .map(|s| {
            s.scene()
                .items()
                .filter_map(|(id, _)| s.object(id))
                .filter(|o| matches!(o, BoardObject::Via(..)))
                .collect::<std::collections::HashSet<_>>()
                .len()
        })
        .unwrap_or(0)
}

fn current_tab_type(app: &App) -> ui::TabType {
    app.window().global::<ui::Data>().get_current_tab().r#type
}

/// The index of the board tab in the first section.
fn board_tab_index(app: &App) -> i32 {
    let state = app.state().borrow();
    state.sections()[0]
        .tabs()
        .iter()
        .position(|t| matches!(t, Tab::Board2d(_)))
        .unwrap() as i32
}

#[test]
fn live_mcp_rule_checks_and_outputs() {
    crate::common::with_headless(WIDTH, HEIGHT, |headless| {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("project");
        copy_dir(&upstream_project("Gerber Test"), &project_dir);
        let workspace = startup::open_workspace(&tmp.path().join("workspace")).unwrap();
        librepcb_i18n::set_language("en").unwrap();
        let window = ui::AppWindow::new().unwrap();
        slint::select_bundled_translation("en").unwrap();
        let app = App::new(window, workspace);
        app.window().show().unwrap();
        assert_eq!(app.open_project(&project_dir.join("project.lpp")), Some(0));
        app.show_tab(InitialTab::Schematic);
        headless.settle(30);

        // ERC: runs automatically after opening.
        let project = app.state().borrow().projects()[0].clone();
        assert!(
            headless.run_until(TIMEOUT, || project
                .checks()
                .borrow()
                .erc()
                .messages()
                .is_some()),
            "ERC did not run"
        );
        headless.settle(10);
        let data = app.window().global::<ui::Data>();
        assert_eq!(
            data.get_current_project().erc.state,
            ui::RuleCheckState::UpToDate
        );
        app.show_panel(ui::PanelPage::RuleCheck);
        save(headless, "app_erc.png");

        // Start the server (any free port) and let an agent add a via.
        let url = app
            .start_mcp_server("127.0.0.1:0".parse().unwrap())
            .unwrap();
        assert!(data.get_mcp_server_running());
        let (net, center) = {
            let state = app.state().borrow();
            let p = state.projects()[0].shared().lock();
            let net = p
                .project()
                .circuit()
                .net_signals()
                .values()
                .next()
                .unwrap()
                .name()
                .to_string();
            drop(p);
            let center = state.sections()[0]
                .tabs()
                .iter()
                .find_map(|t| match t {
                    Tab::Board2d(b) => b.scene().and_then(|s| s.content_bounds()),
                    _ => None,
                })
                .unwrap()
                .center();
            (net, center)
        };
        let vias_before = scene_vias(&app);
        assert_eq!(current_tab_type(&app), ui::TabType::Schematic);
        let results = agent(
            headless,
            &url,
            vec![
                ("project_summary", json!({})),
                (
                    "via_add",
                    json!({
                        "position": { "x": center.x, "y": center.y },
                        "net": net,
                        "size": 6.0,
                        "drill": 3.0,
                    }),
                ),
            ],
        );
        assert!(results[0]["revision"].is_u64());
        // The UI followed the agent to the board and updated its scene.
        assert_eq!(current_tab_type(&app), ui::TabType::Board2d);
        assert_eq!(scene_vias(&app), vias_before + 1);
        assert!(data.get_mcp_agent_active());
        let tab = data.get_current_tab();
        assert_eq!(tab.undo_text, "AI: via_add");
        assert!(tab.unsaved_changes);
        save(headless, "app_mcp_board.png");

        // The UI's undo undoes the agent's edit; redo restores it.
        let board_tab = board_tab_index(&app);
        let backend = app.window().global::<ui::Backend>();
        backend.invoke_trigger_tab(0, board_tab, ui::TabAction::Undo);
        headless.settle(10);
        assert_eq!(scene_vias(&app), vias_before);
        backend.invoke_trigger_tab(0, board_tab, ui::TabAction::Redo);
        headless.settle(10);
        assert_eq!(scene_vias(&app), vias_before + 1);

        // DRC in the background, shown in the rule check panel.
        backend.invoke_trigger_board(0, 0, ui::BoardAction::RunDrc);
        headless.settle(5);
        let board = project.board_at(0).unwrap().0;
        assert!(
            headless.run_until(TIMEOUT, || {
                project
                    .checks()
                    .borrow()
                    .drc(board)
                    .is_some_and(|c| !c.is_running() && c.messages().is_some())
            }),
            "DRC did not finish"
        );
        headless.settle(10);
        let messages = project
            .checks()
            .borrow()
            .drc(board)
            .and_then(|c| c.messages().map(|m| m.entries().len()))
            .unwrap();
        assert!(messages > 0, "the big via should cause DRC messages");
        data.set_current_rule_check_board_index(0);
        app.show_panel(ui::PanelPage::RuleCheck);
        save(headless, "app_drc.png");
        assert_eq!(data.get_current_rule_check().r#type, ui::RuleCheckType::Drc);
        assert_eq!(
            data.get_current_rule_check().state,
            ui::RuleCheckState::UpToDate
        );

        // Double-clicking a message zooms to its location; approving it is an
        // undoable command which keeps the results up to date.
        let rows = data.get_current_rule_check().messages;
        let mut row = rows.row_data(0).unwrap();
        row.action = ui::RuleCheckMessageAction::HighlightAndZoomTo;
        rows.set_row_data(0, row);
        save(headless, "app_drc_zoom.png");
        let unapproved = data.get_current_rule_check().unapproved;
        let mut row = rows.row_data(0).unwrap();
        row.approved = true;
        rows.set_row_data(0, row);
        headless.settle(10);
        let check = data.get_current_rule_check();
        assert_eq!(check.unapproved, unapproved - 1);
        assert_eq!(check.state, ui::RuleCheckState::UpToDate);
        assert!(
            check
                .messages
                .row_data(check.messages.row_count() - 1)
                .unwrap()
                .approved
        );
        assert_eq!(data.get_current_tab().undo_text, "Approve");

        // Exports: Gerber/Excellon and the schematic PDF.
        let output = project_dir.join("output");
        backend.invoke_trigger_tab(0, board_tab, ui::TabAction::ExportFabricationData);
        backend.invoke_trigger_tab(0, board_tab, ui::TabAction::ExportPdf);
        // The graphics export dialog opens; "Run" exports.
        headless.settle(5);
        let forms = app.window().global::<ui::Dialogs>();
        assert!(forms.get_form_shown());
        forms.invoke_form_button(-1);
        headless.settle(5);
        assert!(!forms.get_form_shown());
        let has = |ext: &str| {
            walk(&output)
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == ext))
        };
        assert!(
            headless.run_until(TIMEOUT, || has("gbr") && has("pdf")),
            "exports missing: {:?}",
            walk(&output)
        );
        headless.settle(10);

        // The agent closes the project: the UI closes its tabs.
        agent(
            headless,
            &url,
            vec![("project_close", json!({ "discard_changes": true }))],
        );
        assert!(app.state().borrow().projects().is_empty());
        assert_eq!(current_tab_type(&app), ui::TabType::Home);
    });
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                files.extend(walk(&p));
            } else {
                files.push(p);
            }
        }
    }
    files
}
