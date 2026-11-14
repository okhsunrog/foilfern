//! Port of tests/unittests/core/fileio/directorylocktest.cpp.

use chrono::{DateTime, Utc};
use foilfern_core::fileio::{DirectoryLock, Error, FilePath, LockStatus, file_utils};
use foilfern_core::system_info;

use crate::helpers::{TempDir, kill, spawn_dummy_process};

struct Fixture {
    _tmp: TempDir,
    dir: FilePath,
    lock_file: FilePath,
}

fn setup() -> Fixture {
    let tmp = TempDir::new();
    let dir = tmp.path().clone();
    let lock_file = FilePath::new(format!("{dir}/.lock")).unwrap();
    Fixture {
        _tmp: tmp,
        dir,
        lock_file,
    }
}

fn read_lines(fp: &FilePath) -> Vec<String> {
    String::from_utf8(file_utils::read_file(fp).unwrap())
        .unwrap()
        .split('\n')
        .map(str::to_owned)
        .collect()
}

#[test]
fn test_default_constructor() {
    let mut lock = DirectoryLock::default();
    assert!(lock.dir_to_lock().is_none());
    assert!(lock.lock_file_path().is_none());
    assert!(lock.status().is_err());
    assert!(lock.try_lock(None).is_err());
    assert!(lock.lock().is_err());
    assert!(lock.unlock().is_err());
}

#[test]
fn test_constructor_with_existing_dir() {
    let f = setup();
    let mut lock = DirectoryLock::new(&f.dir);
    assert_eq!(lock.dir_to_lock(), Some(&f.dir));
    assert_eq!(lock.lock_file_path(), Some(&f.lock_file));
    lock.status().unwrap();
    lock.try_lock(None).unwrap();
    lock.unlock_if_locked().unwrap();
    lock.lock().unwrap();
    lock.unlock().unwrap();
}

#[test]
fn test_constructor_with_non_existing_dir() {
    // Using DirectoryLock on non-existent directories must not be possible.
    let f = setup();
    let dir = f.dir.path_to("ghost");
    let mut lock = DirectoryLock::new(&dir);
    assert_eq!(lock.dir_to_lock(), Some(&dir));
    assert_eq!(lock.lock_file_path(), Some(&dir.path_to(".lock")));
    assert!(lock.status().is_err());
    assert!(lock.try_lock(None).is_err());
    assert!(lock.lock().is_err());
    assert!(lock.unlock().is_err());
}

#[test]
fn test_constructor_with_existing_file() {
    // Using DirectoryLock on an existing file (instead of a directory) must
    // not be possible.
    let f = setup();
    let file = f.dir.path_to("file");
    file_utils::write_file(&file, b"").unwrap();
    let mut lock = DirectoryLock::new(&file);
    assert_eq!(lock.dir_to_lock(), Some(&file));
    assert_eq!(lock.lock_file_path(), Some(&file.path_to(".lock")));
    assert!(lock.status().is_err());
    assert!(lock.try_lock(None).is_err());
    assert!(lock.lock().is_err());
    assert!(lock.unlock().is_err());
}

#[test]
fn test_destructor_unlock() {
    let f = setup();
    // Destroying without lock.
    {
        let _lock = DirectoryLock::new(&f.dir);
    }
    assert!(!f.lock_file.is_existing_file());

    // Destroying after releasing lock.
    {
        let mut lock = DirectoryLock::new(&f.dir);
        lock.lock().unwrap();
        lock.unlock().unwrap();
    }
    assert!(!f.lock_file.is_existing_file());

    // Destroying with active lock.
    {
        let mut lock = DirectoryLock::new(&f.dir);
        lock.lock().unwrap();
    }
    assert!(!f.lock_file.is_existing_file());
}

#[test]
fn test_destructor_dont_unlock() {
    let f = setup();
    // Destroying without lock.
    {
        let _lock = DirectoryLock::new(&f.dir);
        file_utils::write_file(&f.lock_file, b"").unwrap(); // Imaginary lock file.
    }
    assert!(f.lock_file.is_existing_file());

    // Destroying after releasing lock.
    {
        let mut lock = DirectoryLock::new(&f.dir);
        lock.lock().unwrap();
        lock.unlock().unwrap();
        file_utils::write_file(&f.lock_file, b"").unwrap(); // Imaginary lock file.
    }
    assert!(f.lock_file.is_existing_file());
}

#[test]
fn test_set_get_dir_to_lock() {
    let f = setup();
    // Create invalid lock object.
    let mut lock = DirectoryLock::default();
    assert_eq!(lock.dir_to_lock(), None);
    assert_eq!(lock.lock_file_path(), None);

    // Set path and readback.
    lock.set_dir_to_lock(&f.dir);
    assert_eq!(lock.dir_to_lock(), Some(&f.dir));
    assert_eq!(lock.lock_file_path(), Some(&f.lock_file));
}

#[test]
fn test_single_status_lock_unlock() {
    let f = setup();
    let mut lock = DirectoryLock::new(&f.dir);
    assert_eq!(lock.status().unwrap(), LockStatus::Unlocked);

    // Get lock.
    lock.lock().unwrap();
    assert_eq!(lock.status().unwrap(), LockStatus::LockedByThisApp);
    assert!(f.lock_file.is_existing_file());

    // Release lock.
    lock.unlock().unwrap();
    assert_eq!(lock.status().unwrap(), LockStatus::Unlocked);
    assert!(!f.lock_file.is_existing_file());
}

#[test]
fn test_multiple_status_lock_unlock() {
    let f = setup();
    let mut lock1 = DirectoryLock::new(&f.dir);
    let mut lock2 = DirectoryLock::new(&f.dir);
    assert_eq!(lock1.status().unwrap(), LockStatus::Unlocked);
    assert_eq!(lock2.status().unwrap(), LockStatus::Unlocked);

    // Get lock1.
    lock1.lock().unwrap();
    assert_eq!(lock1.status().unwrap(), LockStatus::LockedByThisApp);
    assert_eq!(lock2.status().unwrap(), LockStatus::LockedByThisApp);
    assert!(f.lock_file.is_existing_file());

    // Get lock2 (steals the lock from lock1).
    lock2.lock().unwrap();
    assert_eq!(lock1.status().unwrap(), LockStatus::LockedByThisApp);
    assert_eq!(lock2.status().unwrap(), LockStatus::LockedByThisApp);
    assert!(f.lock_file.is_existing_file());

    // Release lock2.
    lock2.unlock().unwrap();
    assert_eq!(lock1.status().unwrap(), LockStatus::Unlocked);
    assert_eq!(lock2.status().unwrap(), LockStatus::Unlocked);
    assert!(!f.lock_file.is_existing_file());
}

#[test]
fn test_try_lock_unlocked_dir() {
    let f = setup();
    let mut lock = DirectoryLock::new(&f.dir);
    lock.try_lock(None).unwrap();
    assert_eq!(lock.status().unwrap(), LockStatus::LockedByThisApp);
}

#[test]
fn test_try_lock_locked_dir_without_callback() {
    let f = setup();
    let mut lock1 = DirectoryLock::new(&f.dir);
    let mut lock2 = DirectoryLock::new(&f.dir);
    lock1.try_lock(None).unwrap();
    assert_eq!(lock1.status().unwrap(), LockStatus::LockedByThisApp);
    assert!(matches!(
        lock2.try_lock(None),
        Err(Error::AlreadyLocked { .. })
    ));
}

#[test]
fn test_try_lock_locked_dir_with_callback_returning_false() {
    let f = setup();
    let mut lock1 = DirectoryLock::new(&f.dir);
    let mut lock2 = DirectoryLock::new(&f.dir);
    lock1.try_lock(None).unwrap();
    assert_eq!(lock1.status().unwrap(), LockStatus::LockedByThisApp);
    let mut callback = |_: &FilePath, _: LockStatus, _: &str| Ok(false);
    assert!(lock2.try_lock(Some(&mut callback)).is_err());
}

#[test]
fn test_try_lock_locked_dir_with_callback_returning_true() {
    let f = setup();
    let mut lock1 = DirectoryLock::new(&f.dir);
    let mut lock2 = DirectoryLock::new(&f.dir);
    lock1.try_lock(None).unwrap();
    assert_eq!(lock1.status().unwrap(), LockStatus::LockedByThisApp);
    let mut args = None;
    let mut callback = |dir: &FilePath, status: LockStatus, user: &str| {
        args = Some((dir.clone(), status, user.to_owned()));
        Ok(true)
    };
    lock2.try_lock(Some(&mut callback)).unwrap();
    let (dir, status, user) = args.unwrap();
    assert_eq!(dir, f.dir);
    assert_eq!(status, LockStatus::LockedByThisApp);
    assert_eq!(
        user,
        format!("{}@{}", system_info::username(), system_info::hostname())
    );
}

#[test]
fn test_try_lock_locked_dir_with_callback_throwing_exception() {
    let f = setup();
    let mut lock1 = DirectoryLock::new(&f.dir);
    let mut lock2 = DirectoryLock::new(&f.dir);
    lock1.try_lock(None).unwrap();
    assert_eq!(lock1.status().unwrap(), LockStatus::LockedByThisApp);
    let mut callback = |_: &FilePath, _: LockStatus, _: &str| Err(Error::UserCanceled);
    assert!(
        lock2
            .try_lock(Some(&mut callback))
            .unwrap_err()
            .is_user_canceled()
    );
}

#[test]
fn test_unlock_if_locked_on_unlocked_dir() {
    let f = setup();
    let mut lock = DirectoryLock::new(&f.dir);
    assert_eq!(lock.status().unwrap(), LockStatus::Unlocked);
    assert!(!lock.unlock_if_locked().unwrap());
    assert_eq!(lock.status().unwrap(), LockStatus::Unlocked);
}

#[test]
fn test_unlock_if_locked_on_locked_dir() {
    let f = setup();
    let mut lock = DirectoryLock::new(&f.dir);
    lock.lock().unwrap();
    assert_eq!(lock.status().unwrap(), LockStatus::LockedByThisApp);
    assert!(lock.unlock_if_locked().unwrap());
    assert_eq!(lock.status().unwrap(), LockStatus::Unlocked);
}

#[test]
fn test_stale_lock() {
    let f = setup();
    let child = spawn_dummy_process();
    let pid = child.id();
    kill(child);

    // Get the lock.
    let mut lock = DirectoryLock::new(&f.dir);
    lock.lock().unwrap();

    // Replace the PID in the lock file.
    let mut lines = read_lines(&f.lock_file);
    lines[3] = pid.to_string();
    file_utils::write_file(&f.lock_file, lines.join("\n").as_bytes()).unwrap();

    // Check status.
    assert_eq!(lock.status().unwrap(), LockStatus::StaleLock);

    // Try to get the lock.
    lock.try_lock(None).unwrap();
}

#[test]
fn test_locked_by_other_app() {
    let f = setup();
    // Run a new process.
    let child = spawn_dummy_process();
    let pid = i64::from(child.id());

    // Get the lock, read the lock file content and release the lock.
    let mut lock = DirectoryLock::new(&f.dir);
    lock.lock().unwrap();
    let mut lines = read_lines(&f.lock_file);
    lock.unlock().unwrap();

    // Create a lock file with the PID/name of the other process.
    lines[3] = pid.to_string();
    lines[4] = system_info::process_name_by_pid(pid).unwrap();
    file_utils::write_file(&f.lock_file, lines.join("\n").as_bytes()).unwrap();

    // Check lock status.
    assert_eq!(lock.status().unwrap(), LockStatus::LockedByOtherApp);

    // Try to get the lock. Should fail since the directory is already locked.
    assert!(lock.try_lock(None).is_err());

    // Exit the other process.
    kill(child);
    std::fs::remove_file(&f.lock_file).unwrap();
}

#[test]
fn test_locked_by_other_user() {
    let f = setup();
    // Get the lock, read the lock file content and release the lock.
    let mut lock = DirectoryLock::new(&f.dir);
    lock.lock().unwrap();
    let mut lines = read_lines(&f.lock_file);
    lock.unlock().unwrap();

    // Create a lock file with another user name.
    lines[1] = "DirectoryLockTest_testLockedByOtherUser".into();
    file_utils::write_file(&f.lock_file, lines.join("\n").as_bytes()).unwrap();

    // Check lock status.
    let (status, user) = lock.status_with_user().unwrap();
    assert_eq!(status, LockStatus::LockedByOtherUser);
    assert_eq!(
        user,
        format!(
            "DirectoryLockTest_testLockedByOtherUser@{}",
            system_info::hostname()
        )
    );

    // Try to get the lock. Should fail since the directory is already locked.
    assert!(lock.try_lock(None).is_err());
}

#[test]
fn test_locked_by_unknown_app() {
    let f = setup();
    // Create a lock file, memorize its content and release the lock.
    let mut lock = DirectoryLock::new(&f.dir);
    lock.lock().unwrap();
    let content = file_utils::read_file(&f.lock_file).unwrap();
    lock.unlock().unwrap();

    // Now create the lock file with the same content as before. So it looks
    // like the lock is coming from this application instance, but it isn't.
    file_utils::write_file(&f.lock_file, &content).unwrap();

    // Check lock status.
    assert_eq!(lock.status().unwrap(), LockStatus::LockedByUnknownApp);

    // Try to get the lock. Should fail since the path is considered as locked.
    assert!(lock.try_lock(None).is_err());
}

#[test]
fn test_lock_file_too_few_lines() {
    let f = setup();
    file_utils::write_file(&f.lock_file, b"a\nb\nc\nd\ne").unwrap();
    let lock = DirectoryLock::new(&f.dir);
    assert!(matches!(lock.status(), Err(Error::LockFileTooFewLines(_))));
}

#[test]
fn test_lock_file_content() {
    let f = setup();
    // Get the lock.
    let mut lock = DirectoryLock::new(&f.dir);
    lock.lock().unwrap();

    // Read the lock file.
    let lines = read_lines(&f.lock_file);

    // Verify content.
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[0], system_info::full_username());
    assert_eq!(lines[1], system_info::username());
    assert_eq!(lines[2], system_info::hostname());
    assert_eq!(lines[3], system_info::current_pid().to_string());
    assert_eq!(
        lines[4],
        system_info::process_name_by_pid(system_info::current_pid()).unwrap()
    );
    // Upstream format: Qt::ISODate in UTC, e.g. "2013-04-13T12:43:52Z".
    assert_eq!(lines[5].len(), 20, "{}", lines[5]);
    assert!(lines[5].ends_with('Z'));
    let lock_time: DateTime<Utc> = lines[5].parse().unwrap();
    let diff = (Utc::now() - lock_time).num_milliseconds().abs();
    assert!(diff < 10_000); // Allow up to 10 seconds difference.
}

/// A lock file as written by upstream LibrePCB (C++) must be understood.
#[test]
fn test_upstream_lock_file() {
    let f = setup();
    let content = format!(
        "Homer Simpson\n{}\n{}\n{}\nlibrepcb\n2013-04-13T12:43:52Z",
        system_info::username(),
        system_info::hostname(),
        i32::MAX // Not running.
    );
    file_utils::write_file(&f.lock_file, content.as_bytes()).unwrap();
    let mut lock = DirectoryLock::new(&f.dir);
    assert_eq!(lock.status().unwrap(), LockStatus::StaleLock);
    lock.try_lock(None).unwrap();
    assert_eq!(lock.status().unwrap(), LockStatus::LockedByThisApp);
}
