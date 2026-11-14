//! Port of tests/unittests/core/sqlitedatabasetest.cpp.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use foilfern_core::sqlite_database::SqliteDatabase;
use rusqlite::named_params;
use tempfile::TempDir;

/// Temporary directory (removed on drop) and the database file path in it.
fn setup() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    (dir, path)
}

const CREATE_TABLE: &str = "CREATE TABLE test (`id` INTEGER PRIMARY KEY NOT NULL, `name` TEXT)";

#[test]
fn test_if_constructor_creates_file() {
    let (_dir, path) = setup();
    assert!(!path.is_file());
    drop(SqliteDatabase::open(&path).unwrap());
    assert!(path.is_file());
}

#[test]
fn test_exec_query() {
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    db.exec("CREATE TABLE test (`id` INTEGER PRIMARY KEY NOT NULL)")
        .unwrap();
}

#[test]
fn test_prepared_query() {
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    db.exec(CREATE_TABLE).unwrap();
    let mut query = db
        .prepare("INSERT INTO test (name) VALUES (:name)", &[])
        .unwrap();
    query.execute(named_params! {":name": "hello"}).unwrap();
}

#[test]
fn test_prepared_query_with_replacements() {
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    db.exec(CREATE_TABLE).unwrap();
    let mut query = db
        .prepare("SELECT COUNT(*) FROM %table", &[("%table", "test")])
        .unwrap();
    assert_eq!(db.count(&mut query, []).unwrap(), 0);
    assert!(db.prepare("SELECT FROM", &[]).is_err());
}

#[test]
fn test_insert() {
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    db.exec(CREATE_TABLE).unwrap();
    for i in 0..100 {
        let mut query = db
            .prepare("INSERT INTO test (name) VALUES (:name)", &[])
            .unwrap();
        let name = format!("row {i}");
        let id = db
            .insert(&mut query, named_params! {":name": name})
            .unwrap();
        assert_eq!(id, i + 1);
    }
}

#[test]
fn test_clear_existing_table() {
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    db.exec(CREATE_TABLE).unwrap();
    db.exec("INSERT INTO test (name) VALUES ('hello')").unwrap();
    db.clear_table("test").unwrap();
    db.clear_table("test").unwrap(); // clearing an empty table should also work
}

#[test]
fn test_clear_non_existing_table() {
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    assert!(db.clear_table("test").is_err());
}

#[test]
fn test_transaction_scope_guard_commit() {
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    {
        let transaction = db.transaction().unwrap();
        db.exec(CREATE_TABLE).unwrap();
        db.exec("INSERT INTO test (name) VALUES ('hello')").unwrap();
        transaction.commit().unwrap();
    }
    db.clear_table("test").unwrap();
}

#[test]
fn test_transaction_scope_guard_rollback() {
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    {
        let _transaction = db.transaction().unwrap();
        db.exec(CREATE_TABLE).unwrap();
        db.exec("INSERT INTO test (name) VALUES ('hello')").unwrap();
    }
    assert!(db.clear_table("test").is_err());
}

#[test]
fn test_multiple_instances_in_same_thread() {
    let (_dir, path) = setup();
    let db1 = SqliteDatabase::open(&path).unwrap();
    let db2 = SqliteDatabase::open(&path).unwrap();
    db1.exec("CREATE TABLE test1 (`id` INTEGER PRIMARY KEY NOT NULL)")
        .unwrap();
    db2.exec("CREATE TABLE test2 (`id` INTEGER PRIMARY KEY NOT NULL)")
        .unwrap();
    db1.clear_table("test2").unwrap();
    db1.clear_table("test1").unwrap();
}

#[test]
fn test_concurrent_read_access_while_write_transaction() {
    // Prepare database.
    let (_dir, path) = setup();
    let db = SqliteDatabase::open(&path).unwrap();
    db.exec(CREATE_TABLE).unwrap();

    let count = AtomicUsize::new(0);
    let cancel = AtomicBool::new(false);
    std::thread::scope(|scope| {
        // Start worker thread which starts a transaction and writes to the
        // database.
        let worker = scope.spawn(|| {
            let db = SqliteDatabase::open(&path).unwrap();
            let transaction = db.transaction().unwrap();
            let timeout = Instant::now() + Duration::from_secs(120);
            while !cancel.load(Ordering::SeqCst) && Instant::now() < timeout {
                db.exec("INSERT INTO test (name) VALUES ('hello')").unwrap();
                count.fetch_add(1, Ordering::SeqCst);
            }
            transaction.commit().unwrap();
        });

        // Wait until the thread has inserted the first values.
        let timeout = Instant::now() + Duration::from_secs(120);
        while count.load(Ordering::SeqCst) < 10 && Instant::now() < timeout {
            std::thread::yield_now();
        }
        assert!(count.load(Ordering::SeqCst) >= 10);

        // Now the worker thread is continuously inserting new values, so
        // let's try to read from the database now.
        for _ in 0..10 {
            let mut query = db.prepare("SELECT COUNT(*) FROM test", &[]).unwrap();
            let row_count = db.count(&mut query, []).unwrap();
            assert_eq!(row_count, 0); // Transaction not committed yet!
        }

        // Terminate the worker thread.
        cancel.store(true, Ordering::SeqCst);
        worker.join().unwrap();
    });

    // Transaction finished -> row count should now be updated.
    let mut query = db.prepare("SELECT COUNT(*) FROM test", &[]).unwrap();
    let row_count = db.count(&mut query, []).unwrap();
    assert_eq!(
        row_count,
        i64::try_from(count.load(Ordering::SeqCst)).unwrap()
    );
}
