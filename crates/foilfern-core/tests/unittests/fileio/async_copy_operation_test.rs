//! Port of tests/unittests/core/fileio/asynccopyoperationtest.cpp.

use std::sync::mpsc;
use std::time::Duration;

use foilfern_core::fileio::{AsyncCopyOperation, CopyProgress, FilePath, file_utils};

use crate::helpers::TempDir;

struct Fixture {
    _tmp: TempDir,
    non_existing_dir: FilePath,
    empty_dir: FilePath,
    populated_dir: FilePath,
    destination_dir: FilePath,
}

fn setup() -> Fixture {
    let tmp = TempDir::new();
    let dir = tmp.path().clone();
    let f = Fixture {
        non_existing_dir: dir.path_to("non existing"),
        empty_dir: dir.path_to("empty directory"),
        populated_dir: dir.path_to("populated directory"),
        destination_dir: dir.path_to("destination directory"),
        _tmp: tmp,
    };
    file_utils::make_path(&f.empty_dir).unwrap();
    file_utils::write_file(&f.populated_dir.path_to("foo/a dir/f"), b"A").unwrap();
    file_utils::write_file(&f.populated_dir.path_to(".dotfile"), b"B").unwrap();
    f
}

#[derive(Default)]
struct Signals {
    status: Vec<String>,
    percent: Vec<i32>,
    result: Option<Result<(), String>>,
}

/// Runs the operation on its own thread and collects the reported progress
/// through a channel (upstream: queued signal connections).
fn run(copy: AsyncCopyOperation) -> Signals {
    let (tx, rx) = mpsc::channel();
    let running = copy.start(move |p| {
        let _ = tx.send(p);
    });
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(running.wait().map_err(|e| e.to_string()));
    });
    let result = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("copy operation timed out");
    let mut signals = Signals {
        result: Some(result),
        ..Default::default()
    };
    for progress in rx.try_iter() {
        match progress {
            CopyProgress::Status(s) => {
                println!("STATUS: {s}");
                signals.status.push(s);
            }
            CopyProgress::Percent(p) => {
                println!("PROGRESS: {p}");
                signals.percent.push(p);
            }
        }
    }
    signals
}

#[test]
fn test_empty_source_dir() {
    let f = setup();
    let s = run(AsyncCopyOperation::new(&f.empty_dir, &f.destination_dir));
    assert!(!s.status.is_empty());
    assert!(!s.percent.is_empty());
    assert_eq!(s.result, Some(Ok(())));
    assert!(f.destination_dir.is_empty_dir());
}

#[test]
fn test_populated_source_dir() {
    let f = setup();
    let s = run(AsyncCopyOperation::new(
        &f.populated_dir,
        &f.destination_dir,
    ));
    assert!(!s.status.is_empty());
    assert!(!s.percent.is_empty());
    assert_eq!(s.result, Some(Ok(())));
    assert_eq!(s.percent.last(), Some(&100));
    let dst = &f.destination_dir;
    assert_eq!(
        file_utils::read_file(&dst.path_to("foo/a dir/f")).unwrap(),
        b"A"
    );
    assert_eq!(
        file_utils::read_file(&dst.path_to(".dotfile")).unwrap(),
        b"B"
    );
}

#[test]
fn test_non_existent_source_dir() {
    let f = setup();
    let s = run(AsyncCopyOperation::new(
        &f.non_existing_dir,
        &f.destination_dir,
    ));
    assert!(!s.status.is_empty());
    assert!(matches!(s.result, Some(Err(_))));
    assert!(!f.destination_dir.is_existing_dir());
}

#[test]
fn test_existing_destination_dir() {
    let f = setup();
    let s = run(AsyncCopyOperation::new(&f.empty_dir, &f.populated_dir));
    assert!(!s.status.is_empty());
    assert!(matches!(s.result, Some(Err(_))));

    // Verify that the already existing destination is not removed.
    assert!(f.populated_dir.path_to("foo/a dir/f").is_existing_file());
    assert!(f.populated_dir.path_to(".dotfile").is_existing_file());
}

#[test]
fn test_abort() {
    let f = setup();
    let copy = AsyncCopyOperation::new(&f.populated_dir, &f.destination_dir);
    let abort = copy.abort_handle();
    abort.abort(); // Abort before the first file is copied.
    let mut statuses = Vec::new();
    let result = copy.run(&mut |p| {
        if let CopyProgress::Status(s) = p {
            statuses.push(s);
        }
    });
    assert!(result.unwrap_err().is_user_canceled());
    assert!(!f.destination_dir.is_existing_dir());
    assert!(
        !FilePath::new(format!("{}~", f.destination_dir))
            .unwrap()
            .is_existing_dir()
    );
}
