//! Port of tests/unittests/editor/library/librarydownloadtest.cpp, plus
//! tests of the library installer (with the local test server data, and an
//! ignored test against the real API server).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use librepcb_core::fileio::{FilePath, TransactionalFileSystem, file_utils};
use librepcb_core::workspace::{ElementKind, ScanOutcome, SearchQuery, Workspace};
use librepcb_network::{
    ApiEndpoint, ClientInfo, Error, InstallMethod, LibraryDownload, NetworkAccessManager, Url,
    fetch_library_list, install_libraries, install_official_libraries, installed_library_dirs,
    library_git, select_libraries,
};
use tokio::sync::Semaphore;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{data_dir, nam, server, url};

struct Fixture {
    _tmp: tempfile::TempDir,
    dst_dir: FilePath,
    dst_lib_dir: FilePath,
}

fn setup() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dst_dir = FilePath::new(tmp.path()).unwrap().path_to("dst");
    file_utils::make_path(&dst_dir).unwrap();
    Fixture {
        _tmp: tmp,
        dst_lib_dir: dst_dir.path_to("my library"),
        dst_dir,
    }
}

fn create_zip(dir: &FilePath, zip: &FilePath) {
    TransactionalFileSystem::open_ro(dir)
        .unwrap()
        .export_to_zip_file(zip, None)
        .unwrap();
}

fn populated_library() -> FilePath {
    FilePath::new(data_dir().join("libraries/Populated Library.lplib")).unwrap()
}

fn file_url(fp: &FilePath) -> Url {
    Url::from_file_path(fp.as_path()).unwrap()
}

async fn download(zip: &FilePath, dest: &FilePath) -> Result<FilePath, Error> {
    LibraryDownload::new(file_url(zip), dest.clone(), Arc::new(Semaphore::new(1)))
        .unwrap()
        .download(&nam())
        .await
}

#[tokio::test]
async fn test_download_invalid_library() {
    let f = setup();
    let src_zip = f.dst_dir.path_to("lib.zip");
    create_zip(&f.dst_dir, &src_zip);
    let err = download(&src_zip, &f.dst_lib_dir).await.unwrap_err();
    assert!(!err.to_string().is_empty());
    assert!(matches!(err, Error::NoLibraryInZip), "{err:?}");
    assert!(!f.dst_lib_dir.is_existing_dir());
}

#[tokio::test]
async fn test_download_valid_library() {
    let f = setup();
    let src_zip = f.dst_dir.path_to("lib.zip");
    create_zip(&populated_library(), &src_zip);
    let dir = download(&src_zip, &f.dst_lib_dir).await.unwrap();
    assert_eq!(dir, f.dst_lib_dir);
    assert!(f.dst_lib_dir.is_existing_dir());
    assert!(f.dst_lib_dir.path_to(".librepcb-lib").is_existing_file());
}

#[tokio::test]
async fn test_download_valid_nested_library() {
    let f = setup();
    let tmp_lib_dir = f.dst_dir.path_to("temp dir");
    file_utils::copy_dir_recursively(&populated_library(), &tmp_lib_dir).unwrap();
    let src_zip = f.dst_dir.path_to("lib.zip");
    create_zip(&f.dst_dir, &src_zip);
    download(&src_zip, &f.dst_lib_dir).await.unwrap();
    assert!(f.dst_lib_dir.is_existing_dir());
    assert!(f.dst_lib_dir.path_to(".librepcb-lib").is_existing_file());
}

#[tokio::test]
async fn test_download_valid_library_destination_already_exists() {
    let f = setup();
    let src_zip = f.dst_dir.path_to("lib.zip");
    create_zip(&populated_library(), &src_zip);

    // Create destination directory, temporary destination directory, and
    // ZIP to check if the library download overwrites them all.
    let dst_tmp_dir = FilePath::new(format!("{}.tmp", f.dst_lib_dir)).unwrap();
    let dst_zip = FilePath::new(format!("{}.zip", f.dst_lib_dir)).unwrap();
    file_utils::make_path(&f.dst_lib_dir).unwrap();
    file_utils::make_path(&dst_tmp_dir).unwrap();
    file_utils::write_file(&dst_zip, b"").unwrap();

    download(&src_zip, &f.dst_lib_dir).await.unwrap();
    assert!(f.dst_lib_dir.is_existing_dir());
    assert!(f.dst_lib_dir.path_to(".librepcb-lib").is_existing_file());
    assert!(!dst_tmp_dir.is_existing_dir());
    assert!(!dst_zip.is_existing_file());
}

// Not ported from upstream: other directories of the same library are
// removed.
#[tokio::test]
async fn test_download_replaces_existing_dirs() {
    let f = setup();
    let src_zip = f.dst_dir.path_to("lib.zip");
    create_zip(&populated_library(), &src_zip);
    let old = f.dst_dir.path_to("old copy.lplib");
    file_utils::make_path(&old).unwrap();
    LibraryDownload::new(
        file_url(&src_zip),
        f.dst_lib_dir.clone(),
        Arc::new(Semaphore::new(1)),
    )
    .unwrap()
    .existing_dirs_to_replace([old.clone(), f.dst_lib_dir.clone()].into())
    .download(&nam())
    .await
    .unwrap();
    assert!(!old.is_existing_dir());
    assert!(f.dst_lib_dir.path_to(".librepcb-lib").is_existing_file());
}

/// Serves the library list with URLs rewritten to the mock server.
async fn mount_library_list(server: &MockServer) {
    let list = std::fs::read_to_string(data_dir().join("server/api/v1/libraries/v2")).unwrap();
    let list = list.replace("http://localhost:50080", &server.uri());
    Mock::given(path("/api/v1/libraries/v2"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(list, "application/json"))
        .mount(server)
        .await;
}

#[tokio::test]
async fn test_select_libraries() {
    let server = server().await;
    mount_library_list(&server).await;
    let endpoint = ApiEndpoint::new(url(&server, "/"));
    let libs = fetch_library_list(&nam(), &endpoint).await.unwrap();
    assert_eq!(libs.len(), 3);

    let names = |selected: Vec<&librepcb_network::Library>| -> Vec<String> {
        selected.iter().map(|l| l.name.clone()).collect()
    };
    assert_eq!(
        names(select_libraries(&libs, &["librepcb connectors"], true).unwrap()),
        ["LibrePCB Base", "LibrePCB Connectors"]
    );
    assert_eq!(
        names(select_libraries(&libs, &["LibrePCB Connectors"], false).unwrap()),
        ["LibrePCB Connectors"]
    );
    assert_eq!(
        names(
            select_libraries(
                &libs,
                &["326f091b-b715-44bf-b385-b613cd60d9f3", "LibrePCB Base"],
                true
            )
            .unwrap()
        ),
        ["LibrePCB Base", "LibrePCB Integrated Circuits"]
    );
    assert!(matches!(
        select_libraries(&libs, &["foo"], true),
        Err(Error::LibraryNotFound(_))
    ));
}

#[tokio::test]
async fn test_install_libraries_into_workspace() {
    let server = server().await;
    mount_library_list(&server).await;
    let endpoint = ApiEndpoint::new(url(&server, "/"));
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = FilePath::new(tmp.path()).unwrap().path_to("ws");
    let ws = Arc::new(Workspace::open_or_create(&ws_dir, None).unwrap());

    let existing = installed_library_dirs(ws.library_db()).unwrap();
    assert!(existing.is_empty());
    let installed = install_official_libraries(
        &nam(),
        &endpoint,
        &ws.remote_libraries_path(),
        &["LibrePCB Connectors"],
        &existing,
    )
    .await
    .unwrap();
    let mut dirs: Vec<String> = installed
        .iter()
        .map(|l| l.directory.to_relative(ws.libraries_path()))
        .collect();
    dirs.sort();
    assert_eq!(
        dirs,
        [
            "remote/6ccc516c-21b7-4cd5-9cf2-7a04cfa361c6.lplib",
            "remote/a9ddf0c6-9b1c-4730-b300-01b4f192ad40.lplib",
        ]
    );

    // Scan (on a blocking thread, like an application would do).
    let ws2 = Arc::clone(&ws);
    let outcome = tokio::task::spawn_blocking(move || {
        ws2.library_db()
            .rescan(&AtomicBool::new(false), &mut |_| {})
    })
    .await
    .unwrap()
    .unwrap();
    let ScanOutcome::Succeeded { element_count } = outcome else {
        panic!("scan aborted");
    };
    assert_eq!(element_count, 122);
    let db = ws.library_db();
    assert_eq!(db.all(ElementKind::Library, None, None).unwrap().len(), 2);
    assert_eq!(db.all(ElementKind::Device, None, None).unwrap().len(), 38);
    let results = db.search(&SearchQuery::new("resistor")).unwrap();
    assert!(!results.is_empty());

    // Reinstalling replaces the library directories.
    let existing = installed_library_dirs(db).unwrap();
    assert_eq!(existing.len(), 2);
    let installed = install_official_libraries(
        &nam(),
        &endpoint,
        &ws.remote_libraries_path(),
        &["LibrePCB Base"],
        &existing,
    )
    .await
    .unwrap();
    assert_eq!(installed.len(), 1);
    assert!(
        installed[0]
            .directory
            .path_to(".librepcb-lib")
            .is_existing_file()
    );
}

#[tokio::test]
async fn test_install_libraries_fails_on_checksum_mismatch() {
    let server = MockServer::start().await;
    let list = std::fs::read_to_string(data_dir().join("server/api/v1/libraries/v2"))
        .unwrap()
        .replace("http://localhost:50080", &server.uri())
        .replace(
            "5f85fa43c057e3cff9206cc086f3023125f8a41d0ed706211d755d5e289314c5",
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
    Mock::given(path("/api/v1/libraries/v2"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(list, "application/json"))
        .mount(&server)
        .await;
    Mock::given(path("/blobs/LibrePCB_Base.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(
            std::fs::read(data_dir().join("server/blobs/LibrePCB_Base.zip")).unwrap(),
        ))
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let remote = FilePath::new(tmp.path()).unwrap().path_to("remote");
    let err = install_official_libraries(
        &nam(),
        &ApiEndpoint::new(url(&server, "/")),
        &remote,
        &["LibrePCB Base"],
        &HashMap::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(&err, Error::LibraryInstall { .. }), "{err:?}");
    assert!(
        !remote
            .path_to("a9ddf0c6-9b1c-4730-b300-01b4f192ad40.lplib")
            .is_existing_dir()
    );
}

fn online_nam() -> NetworkAccessManager {
    NetworkAccessManager::new(ClientInfo {
        app_version: "2.0.0".into(),
        git_revision: String::new(),
        file_format_version: librepcb_core::application::file_format_version(),
        locale: "en_US".into(),
    })
    .unwrap()
}

fn official_endpoint() -> ApiEndpoint {
    ApiEndpoint::new(Url::parse(librepcb_core::workspace::OFFICIAL_API_URL).unwrap())
}

/// Requests the official library list from api.librepcb.org (needs
/// internet access).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires internet access to api.librepcb.org"]
async fn test_fetch_official_library_list_online() {
    let libs = fetch_library_list(&online_nam(), &official_endpoint())
        .await
        .unwrap();
    let base = select_libraries(&libs, &["LibrePCB Base"], true).unwrap();
    assert_eq!(base.len(), 1);
    assert!(base[0].download_url.is_some());
    eprintln!(
        "{} libraries, LibrePCB Base v{} at {:?}",
        libs.len(),
        base[0].version,
        base[0].download_url
    );
}

/// Downloads the official "LibrePCB Base" library from api.librepcb.org
/// into a temporary workspace and scans it (needs internet access to
/// api.librepcb.org and to the download host, currently
/// codeload.github.com).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires internet access to api.librepcb.org and codeload.github.com"]
async fn test_install_official_base_library_online() {
    let nam = online_nam();
    let endpoint = official_endpoint();
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = FilePath::new(tmp.path()).unwrap().path_to("ws");
    let ws = Arc::new(Workspace::open_or_create(&ws_dir, None).unwrap());
    let installed = install_official_libraries(
        &nam,
        &endpoint,
        &ws.remote_libraries_path(),
        &["LibrePCB Base"],
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(installed.len(), 1);
    let ws2 = Arc::clone(&ws);
    let outcome = tokio::task::spawn_blocking(move || {
        ws2.library_db()
            .rescan(&AtomicBool::new(false), &mut |_| {})
    })
    .await
    .unwrap()
    .unwrap();
    let ScanOutcome::Succeeded { element_count } = outcome else {
        panic!("scan aborted");
    };
    eprintln!("Scanned {element_count} elements of {}", installed[0].name);
    assert!(element_count > 500, "{element_count}");
    let results = ws
        .library_db()
        .search(&SearchQuery {
            keyword: "resistor".to_owned(),
            kinds: vec![ElementKind::Device],
            ..SearchQuery::default()
        })
        .unwrap();
    assert!(!results.is_empty());
    eprintln!(
        "First resistor device: {} ({})",
        results[0].name, results[0].uuid
    );
}

fn run_git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The git fallback on a local repository (skipped without `git`): the
/// library replaces the destination atomically, without `.git`, and other
/// directories of the library are removed.
#[tokio::test(flavor = "multi_thread")]
async fn test_clone_library_from_local_git_repository() {
    if !library_git::git_available().await {
        eprintln!("git not found, skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("sym")).unwrap();
    std::fs::write(repo.join(".librepcb-lib"), "1\n").unwrap();
    std::fs::write(repo.join("library.lp"), "(librepcb_library)\n").unwrap();
    std::fs::write(repo.join("sym/readme.txt"), "v1\n").unwrap();
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["add", "."]);
    let commit_args = [
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-q",
        "-m",
        "v1",
    ];
    run_git(&repo, &commit_args);
    let v1 = run_git(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("sym/readme.txt"), "v2\n").unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &commit_args);
    let repo_url = Url::from_directory_path(&repo).unwrap();

    let remote = FilePath::new(tmp.path()).unwrap().path_to("remote");
    let dest = remote.path_to("lib.lplib");
    let old = remote.path_to("old copy.lplib");
    file_utils::make_path(&dest).unwrap();
    file_utils::make_path(&old).unwrap();
    std::fs::write(dest.path_to("stale.txt").as_path(), "x").unwrap();

    // Pinned to the first commit.
    let dir = library_git::clone_library(&repo_url, Some(&v1), &dest, &[old.clone()].into())
        .await
        .unwrap();
    assert_eq!(dir, dest);
    let read = |p: &str| std::fs::read_to_string(dest.path_to(p).as_path()).unwrap();
    assert_eq!(read("sym/readme.txt"), "v1\n");
    assert!(dest.path_to(".librepcb-lib").is_existing_file());
    assert!(!dest.path_to(".git").is_existing_dir());
    assert!(!dest.path_to("stale.txt").is_existing_file());
    assert!(!old.is_existing_dir());
    assert!(!remote.path_to("lib.lplib.git-tmp").is_existing_dir());

    // Without commit (or with an unknown one): the default branch.
    let unknown = "0123456789abcdef0123456789abcdef01234567";
    for commit in [None, Some(unknown)] {
        library_git::clone_library(&repo_url, commit, &dest, &Default::default())
            .await
            .unwrap();
        assert_eq!(read("sym/readme.txt"), "v2\n");
    }

    // Not a library: the destination is kept.
    let not_lib = tmp.path().join("notlib");
    std::fs::create_dir_all(&not_lib).unwrap();
    std::fs::write(not_lib.join("x.txt"), "x").unwrap();
    run_git(&not_lib, &["init", "-q"]);
    run_git(&not_lib, &["add", "."]);
    run_git(&not_lib, &commit_args);
    let err = library_git::clone_library(
        &Url::from_directory_path(&not_lib).unwrap(),
        None,
        &dest,
        &Default::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::NoLibraryInZip), "{err}");
    assert_eq!(read("sym/readme.txt"), "v2\n");
    assert!(!remote.path_to("lib.lplib.git-tmp").is_existing_dir());
}

/// Installs "LibrePCB Base" with a blocked ZIP download URL, so the git
/// fallback clones the repository at the commit of the ZIP URL (needs
/// internet access to api.librepcb.org and github.com, and `git`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires internet access to api.librepcb.org and github.com"]
async fn test_install_official_base_library_with_git_fallback_online() {
    let nam = online_nam();
    let libs = fetch_library_list(&nam, &official_endpoint())
        .await
        .unwrap();
    let mut base = select_libraries(&libs, &["LibrePCB Base"], false).unwrap()[0].clone();
    assert!(base.repository_url.is_some());
    // Same path (with the commit), unreachable host.
    let mut blocked = base.download_url.clone().unwrap();
    blocked.set_host(Some("127.0.0.1")).unwrap();
    blocked.set_port(Some(9)).unwrap();
    base.download_url = Some(blocked);
    let tmp = tempfile::tempdir().unwrap();
    let remote = FilePath::new(tmp.path()).unwrap().path_to("remote");
    let installed = install_libraries(&nam, &[&base], &remote, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(installed[0].method, InstallMethod::Git);
    let dir = &installed[0].directory;
    assert!(dir.path_to(".librepcb-lib").is_existing_file());
    assert!(!dir.path_to(".git").is_existing_dir());
    let pkg_count = std::fs::read_dir(dir.path_to("pkg").as_path())
        .unwrap()
        .count();
    assert!(pkg_count > 500, "{pkg_count}");
}
