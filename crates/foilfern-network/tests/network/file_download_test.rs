//! Port of tests/unittests/core/network/filedownloadtest.cpp.

use std::sync::Arc;

use foilfern_core::fileio::{FilePath, file_utils};
use foilfern_network::{ChecksumAlgorithm, FileDownload, Url};
use tokio::sync::Semaphore;

use crate::{data_url, nam, record_progress, server, url};

struct Data {
    url: Url,
    dest_filename: &'static str,
    extract_dirname: Option<&'static str>,
    sha256: Option<&'static str>,
    error_msg: Option<&'static str>,
    test_existing_destination: bool,
}

fn test_data() -> Vec<Data> {
    let zip = || data_url("unittests/librepcbcommon/FileDownloadTest/first_pcb.zip");
    vec![
        Data {
            url: zip(),
            dest_filename: "first_pcb_downloaded.zip",
            extract_dirname: Some("first_pcb_extracted"),
            sha256: Some("f6f18782790d2a185698f7028a83397d56ef6145679f646c8de5ddfc298d8f89"),
            error_msg: None,
            test_existing_destination: false,
        },
        Data {
            url: zip(),
            dest_filename: "first_pcb_downloaded.zip",
            extract_dirname: Some("first_pcb_extracted"),
            sha256: Some("f6f18782790d2a185698f7028a83397d56ef6145679f646c8de5ddfc298d8f89"),
            error_msg: None,
            test_existing_destination: true,
        },
        Data {
            url: zip(),
            dest_filename: "first_pcb_downloaded.zip",
            extract_dirname: None,
            // Wrong checksum.
            sha256: Some("f6f18782790d2a185698f7028a83397d56ef6145679f646c8de5ddfc298d8f88"),
            error_msg: Some("Checksum verification of downloaded file failed!"),
            test_existing_destination: false,
        },
        Data {
            url: data_url("unittests/librepcbcommon/FileDownloadTest/libraries"),
            dest_filename: "libraries.json",
            extract_dirname: None,
            sha256: None,
            error_msg: None,
            test_existing_destination: false,
        },
        Data {
            url: Url::parse("file:///some-invalid-url").unwrap(),
            dest_filename: "some-invalid-url",
            extract_dirname: Some("some-invalid-url_extracted"),
            sha256: None,
            // The rest is platform dependent.
            error_msg: Some("Error opening "),
            test_existing_destination: false,
        },
    ]
}

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: FilePath,
}

fn setup() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dir = FilePath::new(tmp.path()).unwrap();
    Fixture { _tmp: tmp, dir }
}

fn semaphore() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(1))
}

#[test]
fn test_constructor_and_setters_and_destructor() {
    let f = setup();
    for data in test_data() {
        let mut dl = FileDownload::new(data.url, f.dir.path_to(data.dest_filename), semaphore())
            .expected_reply_content_size(100)
            .expected_checksum(ChecksumAlgorithm::Sha1, b"42".to_vec());
        if let Some(dir) = data.extract_dirname {
            dl = dl.zip_extraction_directory(f.dir.path_to(dir), None);
        }
        drop(dl);
    }
}

async fn run_download(f: &Fixture, data: &Data) {
    let destination = f.dir.path_to(data.dest_filename);
    let extract_dir = data.extract_dirname.map(|d| f.dir.path_to(d));

    // Create destination files.
    if data.test_existing_destination {
        file_utils::write_file(&destination, b"Foo").unwrap();
        let dir = extract_dir.as_ref().unwrap();
        file_utils::write_file(&dir.path_to("foo"), b"Foo").unwrap();
    }

    // Start the file download.
    let mut dl = FileDownload::new(data.url.clone(), destination.clone(), semaphore());
    if let Some(dir) = &extract_dir {
        dl = dl.zip_extraction_directory(dir.clone(), None);
    }
    if let Some(sha256) = data.sha256 {
        dl = dl.expected_checksum(ChecksumAlgorithm::Sha256, hex::decode(sha256).unwrap());
    }
    let progress = record_progress(dl.subscribe_progress());
    let result = dl.download(&nam()).await;
    let progress = progress.await.unwrap();

    // Check the result.
    assert!(!progress.is_empty());
    match data.error_msg {
        None => {
            let downloaded = result.unwrap();
            assert!(progress.iter().any(|p| p.bytes > 0));
            assert_eq!(downloaded.file, destination);
            assert_eq!(downloaded.extracted_to, extract_dir);
            assert_eq!(
                extract_dir.is_none(),
                destination.is_existing_file(),
                "{}",
                data.dest_filename
            );
        }
        Some(msg) => {
            let err = result.unwrap_err().to_string();
            assert!(err.starts_with(msg), "{err}");
            assert!(!destination.is_existing_file());
        }
    }
    match (data.error_msg, &extract_dir) {
        (None, Some(dir)) => {
            assert!(dir.is_existing_dir());
            assert!(!dir.is_empty_dir());
            // The previous content was replaced, and all temporary
            // directories were removed.
            assert!(!dir.path_to("foo").is_existing_file());
            assert!(
                dir.path_to("LibrePCB-first_pcb/.gitignore")
                    .is_existing_file()
            );
            assert!(
                !FilePath::new(format!("{dir}.tmp"))
                    .unwrap()
                    .is_existing_dir()
            );
            assert!(
                !FilePath::new(format!("{dir}.backup"))
                    .unwrap()
                    .is_existing_dir()
            );
        }
        (_, Some(dir)) => assert!(!dir.is_existing_dir()),
        (_, None) => {}
    }
}

#[tokio::test]
async fn test_download() {
    for data in test_data() {
        let f = setup();
        run_download(&f, &data).await;
    }
}

#[tokio::test]
async fn test_download_http() {
    let server = server().await;
    let f = setup();
    let data = Data {
        url: url(&server, "/blobs/LibrePCB_Base.zip"),
        dest_filename: "base.zip",
        extract_dirname: None,
        sha256: Some("5f85fa43c057e3cff9206cc086f3023125f8a41d0ed706211d755d5e289314c5"),
        error_msg: None,
        test_existing_destination: false,
    };
    run_download(&f, &data).await;
    let content = file_utils::read_file(&f.dir.path_to("base.zip")).unwrap();
    let expected = std::fs::read(crate::data_dir().join("server/blobs/LibrePCB_Base.zip")).unwrap();
    assert!(content == expected);
}

#[tokio::test]
async fn test_download_http_not_found() {
    let server = server().await;
    let f = setup();
    let destination = f.dir.path_to("x.zip");
    let err = FileDownload::new(
        url(&server, "/blobs/nonexistent.zip"),
        destination.clone(),
        semaphore(),
    )
    .zip_extraction_directory(f.dir.path_to("x"), None)
    .download(&nam())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Not Found"), "{err}");
    assert!(!destination.is_existing_file());
    assert!(!f.dir.path_to("x").is_existing_dir());
    // No temporary files left behind.
    assert!(f.dir.is_empty_dir());
}

#[tokio::test]
async fn test_zip_discovery_and_cleanup_callbacks() {
    let f = setup();
    let extract_dir = f.dir.path_to("extracted");
    let (tx, rx) = std::sync::mpsc::channel();
    let downloaded = FileDownload::new(
        data_url("unittests/librepcbcommon/FileDownloadTest/first_pcb.zip"),
        f.dir.path_to("first_pcb.zip"),
        semaphore(),
    )
    .zip_extraction_directory(
        extract_dir.clone(),
        Some(Box::new(|tmp: &FilePath| {
            Ok(tmp.path_to("LibrePCB-first_pcb"))
        })),
    )
    .zip_cleanup_callback(Box::new(move |dir: &FilePath| {
        tx.send(dir.clone()).unwrap();
    }))
    .download(&nam())
    .await
    .unwrap();
    assert_eq!(downloaded.extracted_to.as_ref(), Some(&extract_dir));
    assert_eq!(rx.recv().unwrap(), extract_dir);
    // The root folder of the ZIP was stripped.
    assert!(extract_dir.path_to(".gitignore").is_existing_file());
}

#[tokio::test]
async fn test_zip_discovery_outside_extraction_dir_fails() {
    let f = setup();
    let other = f.dir.clone();
    let err = FileDownload::new(
        data_url("unittests/librepcbcommon/FileDownloadTest/first_pcb.zip"),
        f.dir.path_to("first_pcb.zip"),
        semaphore(),
    )
    .zip_extraction_directory(
        f.dir.path_to("extracted"),
        Some(Box::new(move |_: &FilePath| Ok(other))),
    )
    .download(&nam())
    .await;
    assert!(err.is_err());
    assert!(!f.dir.path_to("extracted").is_existing_dir());
}
