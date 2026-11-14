//! Port of tests/unittests/core/systeminfotest.cpp.

use std::time::Duration;

use foilfern_core::system_info;

use crate::helpers::{kill, own_process_name, spawn_dummy_process};

#[test]
fn test_get_username() {
    // The username must not be empty on any system.
    let username = system_info::username();
    println!("Username: {username}");
    assert!(!username.is_empty());
}

#[test]
fn test_get_full_username() {
    // The full username may be empty because the user didn't set it...
    println!("Full username: {}", system_info::full_username());
}

#[test]
fn test_get_hostname() {
    // The hostname must not be empty on any system.
    let hostname = system_info::hostname();
    println!("Hostname: {hostname}");
    assert!(!hostname.is_empty());
}

#[test]
fn test_is_process_running() {
    // Check this process.
    assert!(system_info::is_process_running(system_info::current_pid()).unwrap());

    // Check another running process.
    let child = spawn_dummy_process();
    let pid = i64::from(child.id());
    assert!(system_info::is_process_running(pid).unwrap());
    kill(child);
    assert!(!system_info::is_process_running(pid).unwrap());

    // Check an invalid process.
    assert!(!system_info::is_process_running(999_999).unwrap());
}

#[test]
fn test_get_process_name_by_pid() {
    // Check this process.
    let name = system_info::process_name_by_pid(system_info::current_pid()).unwrap();
    assert_eq!(name, own_process_name());

    // Check another running process.
    let child = spawn_dummy_process();
    let pid = i64::from(child.id());
    assert_ne!(pid, system_info::current_pid());
    std::thread::sleep(Duration::from_millis(200)); // Workaround from upstream.
    assert_eq!(
        system_info::process_name_by_pid(pid).unwrap(),
        own_process_name()
    );
    kill(child);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(system_info::process_name_by_pid(pid).unwrap(), "");

    // Check an invalid process.
    assert_eq!(system_info::process_name_by_pid(999_999).unwrap(), "");
}

#[test]
fn test_detect_runtime() {
    // Must not panic; the value depends on the environment.
    println!("Runtime: {}", system_info::detect_runtime());
}
