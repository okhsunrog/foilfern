//! Port of tests/unittests/core/workspace/workspacetest.cpp.

use std::collections::BTreeMap;

use librepcb_core::application::file_format_version;
use librepcb_core::fileio::{FilePath, VersionFile, file_utils};
use librepcb_core::types::Version;
use librepcb_core::workspace::{DataDirectoryChoice, Workspace, workspace_file_format_version};

use crate::helpers::TempDir;

struct Fixture {
    _tmp: TempDir,
    ws_dir: FilePath,
    version_file: FilePath,
    projects_path: FilePath,
    data_path: FilePath,
    libraries_path: FilePath,
}

fn setup() -> Fixture {
    let tmp = TempDir::new();
    // The whitespaces in the path are there to make the test even stronger ;)
    let ws_dir = tmp.path().path_to("test workspace dir");
    Fixture {
        version_file: ws_dir.path_to(".librepcb-workspace"),
        projects_path: ws_dir.path_to("projects"),
        data_path: ws_dir.path_to("data"),
        libraries_path: ws_dir.path_to("data/libraries"),
        ws_dir,
        _tmp: tmp,
    }
}

fn v(s: &str) -> Version {
    s.parse().unwrap()
}

fn dirs(entries: &[(&str, &str)]) -> BTreeMap<String, Version> {
    entries
        .iter()
        .map(|(name, version)| ((*name).to_owned(), v(version)))
        .collect()
}

fn choice(dir: &str, copy: Option<(&str, &str)>) -> DataDirectoryChoice {
    DataDirectoryChoice {
        dir: dir.to_owned(),
        copy: copy.map(|(a, b)| (a.to_owned(), b.to_owned())),
    }
}

#[test]
fn test_create_open_close() {
    let f = setup();
    // Create new workspace.
    Workspace::create(&f.ws_dir).unwrap();

    // Check existence of workspace directory and version file.
    assert!(f.ws_dir.is_existing_dir());
    assert!(f.version_file.is_existing_file());

    // Open/close workspace.
    {
        let ws = Workspace::open(&f.ws_dir, "data", None).unwrap();
        assert_eq!(ws.path(), &f.ws_dir);
        assert_eq!(ws.projects_path(), &f.projects_path);
        assert_eq!(ws.data_path(), &f.data_path);
        assert_eq!(ws.libraries_path(), &f.libraries_path);
        assert_eq!(
            ws.library_db().file_path(),
            &f.libraries_path.path_to("cache_v8.sqlite")
        );
        assert!(ws.library_db().file_path().is_existing_file());
    }

    // Open/close workspace again.
    Workspace::open(&f.ws_dir, "data", None).unwrap();
}

#[test]
fn test_open_non_existing_workspace() {
    let f = setup();
    assert!(Workspace::open(&f.ws_dir, "data", None).is_err());
}

#[test]
fn test_open_incompatible_workspace_version() {
    let f = setup();
    Workspace::create(&f.ws_dir).unwrap();
    let mut version_file =
        VersionFile::from_bytes(&file_utils::read_file(&f.version_file).unwrap()).unwrap();
    assert_eq!(version_file.version(), &workspace_file_format_version());
    version_file.set_version(v("0.0.1")); // Version 0.0.1 will never exist.
    file_utils::write_file(&f.version_file, &version_file.to_bytes()).unwrap();
    assert!(Workspace::open(&f.ws_dir, "data", None).is_err());
}

#[test]
fn test_if_opening_workspace_multiple_times_fails() {
    let f = setup();
    Workspace::create(&f.ws_dir).unwrap();
    let _ws1 = Workspace::open(&f.ws_dir, "data", None).unwrap();
    assert!(Workspace::open(&f.ws_dir, "data", None).is_err());
}

#[test]
fn test_check_compatibility() {
    let f = setup();
    let err = Workspace::check_compatibility(&f.ws_dir).unwrap_err();
    assert!(!err.to_string().is_empty());
    Workspace::create(&f.ws_dir).unwrap();
    Workspace::check_compatibility(&f.ws_dir).unwrap();
}

#[test]
fn test_find_data_directories() {
    let f = setup();
    assert!(
        Workspace::find_data_directories(&f.ws_dir)
            .unwrap()
            .is_empty()
    );
    Workspace::create(&f.ws_dir).unwrap();
    assert!(
        Workspace::find_data_directories(&f.ws_dir)
            .unwrap()
            .is_empty()
    );
    drop(Workspace::open(&f.ws_dir, "data", None).unwrap());
    let current = file_format_version().to_string();
    assert_eq!(
        Workspace::find_data_directories(&f.ws_dir).unwrap(),
        dirs(&[("data", &current)])
    );
    drop(Workspace::open(&f.ws_dir, "v0.1", None).unwrap());
    assert_eq!(
        Workspace::find_data_directories(&f.ws_dir).unwrap(),
        dirs(&[("data", &current), ("v0.1", "0.1")])
    );
}

#[test]
fn test_determine_data_directory_empty() {
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[])),
        choice("data", None)
    );
}

#[test]
fn test_determine_data_directory_only_older_version() {
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[("v0.0.1", "0.0.1")])),
        choice("data", Some(("v0.0.1", "data")))
    );
}

#[test]
fn test_determine_data_directory_only_current_version() {
    let current = file_format_version().to_string();
    let versioned = format!("v{current}");
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[(&versioned, &current)])),
        choice(&versioned, None)
    );
}

#[test]
fn test_determine_data_directory_only_newer_version() {
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[("v999", "999")])),
        choice("data", None)
    );
}

#[test]
fn test_determine_data_directory_older_and_newer_versions() {
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[
            ("v0.0.1", "0.0.1"),
            ("v0.0.2", "0.0.2"),
            ("v999", "999"),
        ])),
        choice("data", Some(("v0.0.2", "data")))
    );
}

#[test]
fn test_determine_data_directory_only_older_version_in_data() {
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[("data", "0.0.1")])),
        choice("data", Some(("data", "v0.0.1")))
    );
}

#[test]
fn test_determine_data_directory_only_current_version_in_data() {
    let current = file_format_version().to_string();
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[("data", &current)])),
        choice("data", None)
    );
}

#[test]
fn test_determine_data_directory_only_newer_version_in_data() {
    let versioned = format!("v{}", file_format_version());
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[("data", "999")])),
        choice(&versioned, None)
    );
}

#[test]
fn test_determine_data_directory_older_version_in_data_with_backups() {
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[
            ("v0.0.1", "0.0.1"),
            ("v0.0.2", "0.0.2"),
            ("data", "0.0.3"),
        ])),
        choice("data", Some(("data", "v0.0.3")))
    );
}

#[test]
fn test_determine_data_directory_current_version_in_data_with_backups() {
    let current = file_format_version().to_string();
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[
            ("v0.0.1", "0.0.1"),
            ("v0.0.2", "0.0.2"),
            ("data", &current),
        ])),
        choice("data", None)
    );
}

#[test]
fn test_determine_data_directory_older_version_in_data_and_backup() {
    assert_eq!(
        Workspace::determine_data_directory(&dirs(&[("v0.0.1", "0.0.1"), ("data", "0.0.1")])),
        choice("data", None)
    );
}

// Tests below are not ported from upstream.

#[test]
fn test_open_or_create() {
    let f = setup();
    {
        let mut ws = Workspace::open_or_create(&f.ws_dir, None).unwrap();
        assert_eq!(ws.data_path(), &f.data_path);
        assert!(
            ws.local_libraries_path()
                .as_str()
                .ends_with("data/libraries/local")
        );
        assert!(
            ws.remote_libraries_path()
                .as_str()
                .ends_with("data/libraries/remote")
        );
        ws.settings_mut().user_name.set("Foo".to_owned());
        ws.save_settings().unwrap();
    }
    assert_eq!(
        file_utils::read_file(&f.data_path.path_to("settings.lp")).unwrap(),
        b"(librepcb_workspace_settings\n (user \"Foo\")\n)\n"
    );
    assert_eq!(
        file_utils::read_file(&f.data_path.path_to(".librepcb-data")).unwrap(),
        format!("{}\n", file_format_version()).as_bytes()
    );
    // Reopen.
    let ws = Workspace::open_or_create(&f.ws_dir, None).unwrap();
    assert_eq!(ws.settings().user_name.get(), "Foo");

    // A non-empty directory which is not a workspace is not overwritten.
    let other = f._tmp.path().path_to("other");
    file_utils::write_file(&other.path_to("file.txt"), b"x").unwrap();
    assert!(Workspace::open_or_create(&other, None).is_err());
}

#[test]
fn test_open_upgrades_v1_data_directory() {
    let f = setup();
    Workspace::create(&f.ws_dir).unwrap();
    file_utils::write_file(&f.data_path.path_to(".librepcb-data"), b"1\n").unwrap();
    file_utils::write_file(
        &f.data_path.path_to("settings.lp"),
        b"(librepcb_workspace_settings\n (api_endpoints\n  (url \"https://api.librepcb.org\")\n )\n (unknown 1)\n)\n",
    )
    .unwrap();
    let ws = Workspace::open(&f.ws_dir, "data", None).unwrap();
    assert!(ws.settings().api_endpoints.get()[0].use_for_order);
    drop(ws);
    assert_eq!(
        file_utils::read_file(&f.data_path.path_to(".librepcb-data")).unwrap(),
        format!("{}\n", file_format_version()).as_bytes()
    );
    // Unknown entries are removed on upgrade, known ones are written.
    assert_eq!(
        String::from_utf8(file_utils::read_file(&f.data_path.path_to("settings.lp")).unwrap())
            .unwrap(),
        "(librepcb_workspace_settings\n (api_endpoints\n  (endpoint \"https://api.librepcb.org\" (libraries true) (parts true) (order true))\n )\n)\n"
    );
}
