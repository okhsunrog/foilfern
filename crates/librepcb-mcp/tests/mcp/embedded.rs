//! The server embedded in a host application: a host-provided project
//! edited by both the "UI" (direct editor access) and the MCP client,
//! change notices and the revision channel, host decisions (refused close,
//! delegated open), and the HTTP transport on localhost (feature `http`).

use std::path::Path;
use std::sync::Arc;

use librepcb_core::fileio::FilePath;
use librepcb_core::types::ElementName;
use librepcb_editor::NoLibrarySource;
use librepcb_editor::commands::project::AddSchematic;
use librepcb_mcp::{
    ChangeNotice, HostDecision, LibrePcbMcp, McpHost, McpState, OpenProject, SharedProject,
};
use parking_lot::Mutex;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, ()>;

/// A host recording notices; optionally refusing `project_close` and
/// opening projects itself ("in a tab").
#[derive(Default)]
struct TestHost {
    notices: Mutex<Vec<ChangeNotice>>,
    refuse_close: bool,
    /// Projects the host "opened in a tab" (delegated `project_open`).
    tabs: Mutex<Vec<SharedProject>>,
    delegate_open: bool,
}

impl McpHost for TestHost {
    fn open_project(&self, path: &FilePath) -> HostDecision<SharedProject> {
        if !self.delegate_open {
            return HostDecision::Allow;
        }
        let dir = path.parent_dir().unwrap();
        let project = OpenProject::open(&dir, path.file_name(), Arc::new(NoLibrarySource))
            .unwrap()
            .into_shared();
        self.tabs.lock().push(Arc::clone(&project));
        HostDecision::Delegated(project)
    }

    fn close_project(&self, _project: &SharedProject, _discard: bool) -> HostDecision<()> {
        if self.refuse_close {
            HostDecision::Refuse("The user is editing this project in LibrePCB.".to_owned())
        } else {
            HostDecision::Allow
        }
    }

    fn changed(&self, notice: &ChangeNotice) {
        self.notices.lock().push(notice.clone());
    }
}

impl TestHost {
    fn notices(&self) -> Vec<ChangeNotice> {
        self.notices.lock().clone()
    }
}

fn open_shared(lpp: &Path) -> SharedProject {
    let path = FilePath::new(lpp).unwrap();
    OpenProject::open(
        &path.parent_dir().unwrap(),
        path.file_name(),
        Arc::new(NoLibrarySource),
    )
    .unwrap()
    .into_shared()
}

/// Connects an rmcp client to a server on `state` through an in-memory
/// duplex stream.
async fn connect(state: McpState) -> Client {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let server = LibrePcbMcp::with_state(state)
            .serve(server_io)
            .await
            .unwrap();
        let _ = server.waiting().await;
    });
    ().serve(client_io).await.unwrap()
}

async fn call(client: &Client, tool: &str, args: Value) -> CallToolResult {
    let args = match args {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"))
}

async fn call_ok(client: &Client, tool: &str, args: Value) -> Value {
    let res = call(client, tool, args).await;
    let envelope = res.structured_content.clone().expect("structured content");
    assert_ne!(res.is_error, Some(true), "{tool} failed: {envelope:#}");
    envelope["result"].clone()
}

/// Calls a tool which must fail; returns the error kind and message.
async fn call_err(client: &Client, tool: &str, args: Value) -> (String, String) {
    let res = call(client, tool, args).await;
    let envelope = res.structured_content.clone().expect("structured content");
    assert_eq!(res.is_error, Some(true), "{tool} succeeded: {envelope:#}");
    (
        envelope["error"]["kind"].as_str().unwrap().to_owned(),
        envelope["error"]["message"].as_str().unwrap().to_owned(),
    )
}

fn schematic_names(project: &SharedProject) -> Vec<String> {
    project
        .lock()
        .project()
        .schematics()
        .iter()
        .map(|s| s.name().as_str().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn host_project_shared_between_ui_and_mcp() {
    let tmp = tempfile::tempdir().unwrap();
    let lpp = crate::common::copy_test_project("Project With Two Boards", tmp.path());
    let shared = open_shared(&lpp);
    let host = Arc::new(TestHost::default());
    let state = McpState::new(host.clone());
    state.set_project(Some(Arc::clone(&shared)));
    let mut revisions = state.subscribe();
    let client = connect(state.clone()).await;

    // Read tools change nothing and notify nobody.
    let rev0 = shared.lock().project().revision();
    let summary = call_ok(&client, "project_summary", json!({})).await;
    assert_eq!(summary["revision"], rev0);
    assert!(host.notices().is_empty());
    assert!(!revisions.has_changed().unwrap());

    // An MCP write: visible to the UI, one notice with focus.
    let added = call_ok(
        &client,
        "schematic_add",
        json!({ "name": "AI Page", "expected_revision": rev0 }),
    )
    .await;
    let rev1 = shared.lock().project().revision();
    assert!(rev1 > rev0);
    assert!(schematic_names(&shared).contains(&"AI Page".to_owned()));
    assert!(revisions.has_changed().unwrap());
    assert_eq!(*revisions.borrow_and_update(), 1);
    let notices = host.notices();
    assert_eq!(notices.len(), 1);
    let n = &notices[0];
    assert_eq!(n.sequence, 1);
    assert_eq!(n.tool, "schematic_add");
    assert_eq!(n.revision_before, Some(rev0));
    assert_eq!(n.revision, Some(rev1));
    assert!(!n.project_replaced && !n.workspace_replaced);
    assert!(Arc::ptr_eq(n.project.as_ref().unwrap(), &shared));
    assert_eq!(n.focus.schematics.len(), 1);
    assert_eq!(
        serde_json::to_value(n.focus.schematics[0]).unwrap(),
        added["uuid"]
    );
    assert!(n.focus.boards.is_empty());

    // A UI edit through the same editor; the agent's expected_revision is
    // stale now.
    shared
        .lock()
        .editor
        .execute(AddSchematic {
            name: ElementName::new("UI Page").unwrap(),
            index: None,
        })
        .unwrap();
    let rev2 = shared.lock().project().revision();
    assert!(rev2 > rev1);
    let (kind, _) = call_err(
        &client,
        "schematic_add",
        json!({ "name": "Late", "expected_revision": rev1 }),
    )
    .await;
    assert_eq!(kind, "stale_revision");
    assert_eq!(host.notices().len(), 1, "failed writes change nothing");
    let summary = call_ok(&client, "project_summary", json!({})).await;
    assert_eq!(summary["revision"], rev2);

    // One undo stack: "AI: <tool>" groups and the UI's commands.
    let history = call_ok(&client, "history", json!({})).await;
    let texts: Vec<&str> = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0], "AI: schematic_add");
    assert!(!texts[1].starts_with("AI:"));
    // The agent's undo undoes the UI's command ...
    let undone = call_ok(&client, "undo", json!({})).await;
    assert_eq!(undone["done"][0], texts[1]);
    assert!(!schematic_names(&shared).contains(&"UI Page".to_owned()));
    let notices = host.notices();
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[1].tool, "undo");
    assert_eq!(notices[1].revision_before, Some(rev2));
    assert_eq!(*revisions.borrow_and_update(), 2);
    // ... and the UI's undo the agent's.
    assert!(shared.lock().editor.undo().unwrap());
    assert!(!schematic_names(&shared).contains(&"AI Page".to_owned()));
    let history = call_ok(&client, "history", json!({})).await;
    assert_eq!(history["done"], 0);

    // Redo and save through MCP: saving changes only the clean state,
    // which is a change for the host (unsaved indicator) as well.
    call_ok(&client, "redo", json!({})).await;
    assert!(shared.lock().has_unsaved_changes());
    call_ok(&client, "project_save", json!({})).await;
    assert!(!shared.lock().has_unsaved_changes());
    let notices = host.notices();
    assert_eq!(
        notices.iter().map(|n| n.tool.as_str()).collect::<Vec<_>>(),
        ["schematic_add", "undo", "redo", "project_save"]
    );
    assert_eq!(notices[3].revision_before, notices[3].revision);

    // The host switches the MCP session to no project.
    state.set_project(None);
    let (kind, _) = call_err(&client, "project_summary", json!({})).await;
    assert_eq!(kind, "no_project");
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn host_refuses_project_close() {
    let tmp = tempfile::tempdir().unwrap();
    let lpp = crate::common::copy_test_project("Project With Two Boards", tmp.path());
    let shared = open_shared(&lpp);
    let host = Arc::new(TestHost {
        refuse_close: true,
        ..Default::default()
    });
    let state = McpState::new(host.clone());
    state.set_project(Some(Arc::clone(&shared)));
    let client = connect(state.clone()).await;

    let (kind, message) =
        call_err(&client, "project_close", json!({ "discard_changes": true })).await;
    assert_eq!(kind, "refused");
    assert!(message.contains("The user is editing"), "{message}");
    assert!(Arc::ptr_eq(&state.project().unwrap(), &shared));
    call_ok(&client, "project_summary", json!({})).await;
    assert!(host.notices().is_empty());

    // The unsaved-changes check comes first (the host is not asked).
    call_ok(&client, "schematic_add", json!({ "name": "X" })).await;
    let (kind, _) = call_err(&client, "project_close", json!({})).await;
    assert_eq!(kind, "conflict");
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn host_opens_project_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let lpp = crate::common::copy_test_project("Project With Two Boards", tmp.path());
    let host = Arc::new(TestHost {
        delegate_open: true,
        ..Default::default()
    });
    let state = McpState::new(host.clone());
    let client = connect(state.clone()).await;

    let opened = call_ok(
        &client,
        "project_open",
        json!({ "path": lpp.to_string_lossy() }),
    )
    .await;
    assert_eq!(opened["path"], lpp.to_string_lossy().as_ref());
    let tab = host.tabs.lock()[0].clone();
    assert!(Arc::ptr_eq(&state.project().unwrap(), &tab));
    let notices = host.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].tool, "project_open");
    assert!(notices[0].project_replaced);
    assert!(Arc::ptr_eq(notices[0].project.as_ref().unwrap(), &tab));

    // A second open conflicts before the host is asked.
    let (kind, _) = call_err(
        &client,
        "project_open",
        json!({ "path": lpp.to_string_lossy() }),
    )
    .await;
    assert_eq!(kind, "conflict");
    assert_eq!(host.tabs.lock().len(), 1);

    // Closing (allowed) detaches the session; the host's tab keeps the
    // project (and its directory lock).
    call_ok(&client, "project_close", json!({})).await;
    assert!(state.project().is_none());
    let notices = host.notices();
    assert_eq!(notices.len(), 2);
    assert!(notices[1].project_replaced && notices[1].project.is_none());
    assert_eq!(tab.lock().file_path(), lpp.to_string_lossy());
    client.cancel().await.unwrap();
}

#[cfg(feature = "http")]
#[test]
fn http_end_to_end_on_localhost() {
    use rmcp::transport::StreamableHttpClientTransport;

    let tmp = tempfile::tempdir().unwrap();
    let lpp = crate::common::copy_test_project("Project With Two Boards", tmp.path());
    let shared = open_shared(&lpp);
    let host = Arc::new(TestHost::default());
    let state = McpState::new(host.clone());
    state.set_project(Some(Arc::clone(&shared)));

    // The host's runtime (e.g. on a background thread of the app); the
    // server is started from the host's own ("UI") thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = librepcb_mcp::transport::start_http(
        state.clone(),
        "127.0.0.1:0".parse().unwrap(),
        runtime.handle(),
    )
    .unwrap();
    assert!(server.url().starts_with("http://127.0.0.1:"));
    assert!(server.url().ends_with("/mcp"));

    runtime.block_on(async {
        let transport = StreamableHttpClientTransport::from_uri(server.url());
        let client = ().serve(transport).await.unwrap();
        let tools = client.list_all_tools().await.unwrap();
        assert!(tools.iter().any(|t| t.name == "schematic_add"));
        let info = call_ok(&client, "server_info", json!({})).await;
        assert_eq!(info["project"]["path"], lpp.to_string_lossy().as_ref());
        call_ok(&client, "schematic_add", json!({ "name": "Over HTTP" })).await;
        client.cancel().await.unwrap();
    });
    assert!(schematic_names(&shared).contains(&"Over HTTP".to_owned()));
    let notices = host.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].tool, "schematic_add");

    // A second server on the same port fails immediately.
    let busy =
        librepcb_mcp::transport::start_http(state.clone(), server.local_addr(), runtime.handle());
    assert!(busy.is_err());

    runtime.block_on(server.shutdown()).unwrap();
}
