//! Port of tests/unittests/core/fileio/transactionalfilesystemtest.cpp.

use std::collections::BTreeSet;

use foilfern_core::fileio::{
    FilePath, RestoreMode, TransactionalFileSystem, ZipArchive, file_utils,
};

use crate::helpers::TempDir;

struct Fixture {
    _tmp: TempDir,
    tmp_dir: FilePath,
    non_existing_dir: FilePath,
    empty_dir: FilePath,
    populated_dir: FilePath,
}

fn setup() -> Fixture {
    let tmp = TempDir::new();
    // Temporary dir (with spaces in path to make tests harder).
    let tmp_dir = tmp.path().path_to("spaces in path");
    file_utils::write_file(&tmp_dir.path_to("1.txt"), b"1").unwrap();

    let empty_dir = tmp_dir.path_to("empty");
    file_utils::make_path(&empty_dir).unwrap();

    let p = tmp_dir.path_to("populated");
    for dir in [".dot/dir", "1/2/3", "a/b", "foo dir/bar dir"] {
        file_utils::make_path(&p.path_to(dir)).unwrap();
    }
    for (file, content) in [
        ("1.txt", "1"),
        ("2.txt", "2"),
        (".dot/file.txt", "file"),
        (".dot/dir/foo.txt", "foo"),
        ("1/1a.txt", "1a"),
        ("1/1b.txt", "1b"),
        ("1/2/3/4.txt", "4"),
        ("a/b/c", "c"),
        ("foo dir/bar dir.txt", "bar"),
        ("foo dir/bar dir/X", "X"),
    ] {
        file_utils::write_file(&p.path_to(file), content.as_bytes()).unwrap();
    }

    Fixture {
        _tmp: tmp,
        non_existing_dir: tmp_dir.path_to("nonexisting"),
        empty_dir,
        populated_dir: p,
        tmp_dir,
    }
}

fn open(dir: &FilePath, writable: bool) -> TransactionalFileSystem {
    TransactionalFileSystem::open(dir, writable, RestoreMode::No, None).unwrap()
}

fn read(fs: &TransactionalFileSystem, path: &str) -> String {
    String::from_utf8(fs.read(path).unwrap()).unwrap()
}

fn read_file(fp: &Option<FilePath>) -> String {
    String::from_utf8(file_utils::read_file(fp.as_ref().unwrap()).unwrap()).unwrap()
}

fn is_file(fp: Option<FilePath>) -> bool {
    fp.unwrap().is_existing_file()
}

fn is_dir(fp: Option<FilePath>) -> bool {
    fp.unwrap().is_existing_dir()
}

fn contains(list: Vec<String>, item: &str) -> bool {
    list.iter().any(|s| s == item)
}

#[test]
fn test_constructor_non_existing_dir() {
    let f = setup();
    let _fs = open(&f.non_existing_dir, true);
}

#[test]
fn test_constructor_empty_dir() {
    let f = setup();
    let _fs = open(&f.empty_dir, true);
}

#[test]
fn test_constructor_populated_dir() {
    let f = setup();
    let _fs = open(&f.populated_dir, true);
}

#[test]
fn test_get_path() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    assert_eq!(fs.path(), &f.populated_dir);
}

#[test]
fn test_is_writable_false() {
    let f = setup();
    assert!(!open(&f.populated_dir, false).is_writable());
}

#[test]
fn test_is_writable_true() {
    let f = setup();
    assert!(open(&f.populated_dir, true).is_writable());
}

#[test]
fn test_get_abs_path_without_argument() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    assert_eq!(fs.abs_path(""), Some(f.populated_dir.clone()));
}

#[test]
fn test_get_abs_path_with_argument() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    assert_eq!(
        fs.abs_path("foo/bar"),
        Some(f.populated_dir.path_to("foo/bar"))
    );
}

#[test]
fn test_write_creates_new_file() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    assert!(!fs.file_exists("new file"));
    fs.write("new file", b"content").unwrap();
    assert!(fs.file_exists("new file"));
    assert_eq!(read(&fs, "new file"), "content");
}

#[test]
fn test_write_existing_file() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    assert!(fs.file_exists("1.txt"));
    assert_eq!(read(&fs, "1.txt"), "1");
    fs.write("1.txt", b"new content").unwrap();
    assert!(fs.file_exists("1.txt"));
    assert_eq!(read(&fs, "1.txt"), "new content");
}

#[test]
fn test_write_creates_new_directory_and_file() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    assert!(!fs.file_exists("x/y/z"));
    fs.write("x/y/z", b"foo").unwrap();
    assert!(fs.file_exists("x/y/z"));
    assert!(contains(fs.dirs(""), "x"));
    assert!(contains(fs.dirs("x"), "y"));
    assert!(contains(fs.files("x/y"), "z"));
}

#[test]
fn test_write_is_delayed_until_save() {
    let f = setup();
    let fp = f.populated_dir.path_to("new dir/new file");
    let rel = fp.to_relative(&f.populated_dir);
    let fs = open(&f.populated_dir, true);
    assert!(!fs.file_exists(&rel));
    assert!(!fp.is_existing_file());

    // Write file.
    fs.write(&rel, b"content").unwrap();
    assert!(!fp.is_existing_file());

    // Save.
    fs.save().unwrap();
    assert!(fp.is_existing_file());
    assert_eq!(file_utils::read_file(&fp).unwrap(), b"content");
}

#[test]
fn test_remove_existing_file() {
    let f = setup();
    let fp = f.populated_dir.path_to("1/1a.txt");
    let rel = fp.to_relative(&f.populated_dir);
    let fs = open(&f.populated_dir, true);
    assert!(fs.file_exists(&rel));
    assert!(contains(fs.files("1"), "1a.txt"));
    assert!(fp.is_existing_file());

    // Remove file.
    fs.remove_file(&rel).unwrap();
    assert!(!fs.file_exists(&rel));
    assert!(!contains(fs.files("1"), "1a.txt"));
    assert!(fp.is_existing_file());

    // Save.
    fs.save().unwrap();
    assert!(!fs.file_exists(&rel));
    assert!(!contains(fs.files("1"), "1a.txt"));
    assert!(!fp.is_existing_file());
}

#[test]
fn test_remove_new_file() {
    let f = setup();
    let fp = f.populated_dir.path_to("1/nonexisting.txt");
    let rel = fp.to_relative(&f.populated_dir);
    let fs = open(&f.populated_dir, true);
    assert!(!fs.file_exists(&rel));
    assert!(!contains(fs.files("1"), "nonexisting.txt"));
    assert!(!fp.is_existing_file());

    // Create new file.
    fs.write(&rel, b"foo").unwrap();
    assert!(fs.file_exists(&rel));
    assert!(contains(fs.files("1"), "nonexisting.txt"));
    assert!(!fp.is_existing_file());

    // Remove the new file.
    fs.remove_file(&rel).unwrap();
    assert!(!fs.file_exists(&rel));
    assert!(!contains(fs.files("1"), "nonexisting.txt"));
    assert!(!fp.is_existing_file());

    // Save.
    fs.save().unwrap();
    assert!(!fs.file_exists(&rel));
    assert!(!contains(fs.files("1"), "nonexisting.txt"));
    assert!(!fp.is_existing_file());
}

#[test]
fn test_remove_dir_recursively() {
    let f = setup();
    let dp = f.populated_dir.path_to(".dot");
    let fp = f.populated_dir.path_to(".dot/dir/foo.txt");
    let rel = fp.to_relative(&f.populated_dir);
    let fs = open(&f.populated_dir, true);
    assert!(fs.file_exists(&rel));
    assert!(contains(fs.dirs(""), ".dot"));
    assert!(contains(fs.dirs(".dot"), "dir"));
    assert!(contains(fs.files(".dot/dir"), "foo.txt"));
    assert!(dp.is_existing_dir());
    assert!(fp.is_existing_file());

    // Remove dir.
    fs.remove_dir_recursively(".dot").unwrap();
    assert!(!fs.file_exists(&rel));
    assert!(!contains(fs.dirs(""), ".dot"));
    assert!(!contains(fs.dirs(".dot"), "dir"));
    assert!(!contains(fs.files(".dot/dir"), "foo.txt"));
    assert!(dp.is_existing_dir());
    assert!(fp.is_existing_file());

    // Save.
    fs.save().unwrap();
    assert!(!fs.file_exists(&rel));
    assert!(!contains(fs.dirs(""), ".dot"));
    assert!(!contains(fs.dirs(".dot"), "dir"));
    assert!(!contains(fs.files(".dot/dir"), "foo.txt"));
    assert!(!dp.is_existing_dir());
    assert!(!fp.is_existing_file());
}

#[test]
fn test_remove_sub_dir_recursively() {
    let f = setup();
    let dp = f.populated_dir.path_to(".dot");
    let sp = f.populated_dir.path_to(".dot/dir");
    let fp = f.populated_dir.path_to(".dot/dir/foo.txt");
    let rel = fp.to_relative(&f.populated_dir);
    let fs = open(&f.populated_dir, true);
    assert!(fs.file_exists(&rel));
    assert!(contains(fs.dirs(""), ".dot"));
    assert!(contains(fs.dirs(".dot"), "dir"));
    assert!(contains(fs.files(".dot/dir"), "foo.txt"));
    assert!(dp.is_existing_dir());
    assert!(sp.is_existing_dir());
    assert!(fp.is_existing_file());

    // Remove dir.
    fs.remove_dir_recursively(".dot/dir").unwrap();
    assert!(!fs.file_exists(&rel));
    assert!(contains(fs.dirs(""), ".dot"));
    assert!(!contains(fs.dirs(".dot"), "dir"));
    assert!(!contains(fs.files(".dot/dir"), "foo.txt"));
    assert!(dp.is_existing_dir());
    assert!(sp.is_existing_dir());
    assert!(fp.is_existing_file());

    // Save.
    fs.save().unwrap();
    assert!(!fs.file_exists(&rel));
    assert!(contains(fs.dirs(""), ".dot"));
    assert!(!contains(fs.dirs(".dot"), "dir"));
    assert!(!contains(fs.files(".dot/dir"), "foo.txt"));
    assert!(dp.is_existing_dir());
    assert!(!sp.is_existing_dir());
    assert!(!fp.is_existing_file());
}

#[test]
fn test_save_throws_exception_if_non_writable() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    assert!(fs.save().is_err());
}

/// The file operations shared by several tests.
fn do_some_file_operations(fs: &TransactionalFileSystem) {
    fs.write("x/y/z", b"z").unwrap(); // create new file
    fs.write("z/y/x.txt", b"x").unwrap(); // create new file
    fs.write("z/y.txt", b"y").unwrap(); // create new file
    fs.write("1.txt", b"new 1").unwrap(); // overwrite existing file
    fs.write(".dot/file.txt", b"new file").unwrap(); // overwrite existing file
    fs.remove_file("z/y/x.txt").unwrap(); // remove new file
    fs.remove_file("1.txt").unwrap(); // remove existing file
    fs.remove_dir_recursively("z").unwrap(); // remove new directory
    fs.remove_dir_recursively("a").unwrap(); // remove existing directory
    fs.write("z/1.txt", b"1").unwrap(); // create new file
    fs.write("z/2.txt", b"2").unwrap(); // create new file
    fs.remove_file("z/1.txt").unwrap(); // remove new file
}

fn check_initial_state(fs: &TransactionalFileSystem) {
    assert!(!fs.file_exists("x/y/z"));
    assert!(!fs.file_exists("z/y/x.txt"));
    assert!(!fs.file_exists("z/y.txt"));
    assert!(fs.file_exists("1.txt"));
    assert!(fs.file_exists("a/b/c"));
    assert!(!fs.file_exists("z/1.txt"));
    assert!(!fs.file_exists("z/2.txt"));
}

fn check_state_in_memory(fs: &TransactionalFileSystem) {
    assert!(fs.file_exists("x/y/z"));
    assert!(!fs.file_exists("z/y/x.txt"));
    assert!(!fs.file_exists("z/y.txt"));
    assert!(!fs.file_exists("1.txt"));
    assert!(!fs.file_exists("a/b/c"));
    assert!(!fs.file_exists("z/1.txt"));
    assert!(fs.file_exists("z/2.txt"));
    assert_eq!(read(fs, "x/y/z"), "z");
    assert_eq!(read(fs, "z/2.txt"), "2");
    assert_eq!(read(fs, ".dot/file.txt"), "new file");
    for path in ["z/y/x.txt", "z/y.txt", "1.txt", "a/b/c", "z/1.txt"] {
        assert!(fs.read(path).is_err(), "{path}");
    }
}

fn check_state_on_disk_unsaved(fs: &TransactionalFileSystem) {
    assert!(!is_file(fs.abs_path("x/y/z")));
    assert!(!is_file(fs.abs_path("z/y/x.txt")));
    assert!(!is_file(fs.abs_path("z/y.txt")));
    assert!(is_file(fs.abs_path("1.txt")));
    assert!(is_file(fs.abs_path("a/b/c")));
    assert!(!is_file(fs.abs_path("z/1.txt")));
    assert!(!is_file(fs.abs_path("z/2.txt")));
    assert_eq!(read_file(&fs.abs_path("1.txt")), "1");
    assert_eq!(read_file(&fs.abs_path("a/b/c")), "c");
    assert_eq!(read_file(&fs.abs_path(".dot/file.txt")), "file");
}

fn check_state_on_disk_saved(fs: &TransactionalFileSystem) {
    assert!(is_file(fs.abs_path("x/y/z")));
    assert!(!is_file(fs.abs_path("z/y/x.txt")));
    assert!(!is_file(fs.abs_path("z/y.txt")));
    assert!(!is_file(fs.abs_path("1.txt")));
    assert!(!is_dir(fs.abs_path("a")));
    assert!(!is_file(fs.abs_path("z/1.txt")));
    assert!(is_file(fs.abs_path("z/2.txt")));
    assert_eq!(read_file(&fs.abs_path("x/y/z")), "z");
    assert_eq!(read_file(&fs.abs_path("z/2.txt")), "2");
    assert_eq!(read_file(&fs.abs_path(".dot/file.txt")), "new file");
}

#[test]
fn test_combination_of_all_methods() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    check_initial_state(&fs);
    do_some_file_operations(&fs);
    check_state_in_memory(&fs);
    check_state_on_disk_unsaved(&fs);

    // Save to file system.
    fs.save().unwrap();

    // Check state in memory (equal to the state before saving).
    check_state_in_memory(&fs);
    check_state_on_disk_saved(&fs);

    // Do some more file operations.
    fs.write("foo", b"foo").unwrap(); // create new file
    fs.write("z/2.txt", b"new 2").unwrap(); // overwrite existing file
    fs.remove_file("x/y/z").unwrap(); // remove existing file

    let check_memory = |fs: &TransactionalFileSystem| {
        assert!(!fs.file_exists("x/y/z"));
        assert!(!fs.file_exists("z/y/x.txt"));
        assert!(!fs.file_exists("z/y.txt"));
        assert!(!fs.file_exists("1.txt"));
        assert!(!fs.file_exists("a/b/c"));
        assert!(!fs.file_exists("z/1.txt"));
        assert!(fs.file_exists("z/2.txt"));
        assert!(fs.file_exists("foo"));
        assert_eq!(read(fs, "z/2.txt"), "new 2");
        assert_eq!(read(fs, "foo"), "foo");
        for path in ["x/y/z", "z/y/x.txt", "z/y.txt", "1.txt", "a/b/c", "z/1.txt"] {
            assert!(fs.read(path).is_err(), "{path}");
        }
    };
    check_memory(&fs);

    // Save to file system.
    fs.save().unwrap();

    // Check state in memory (equal to the state before saving).
    check_memory(&fs);

    // Check state on file system.
    assert!(!is_file(fs.abs_path("x/y/z")));
    assert!(!is_file(fs.abs_path("z/y/x.txt")));
    assert!(!is_file(fs.abs_path("z/y.txt")));
    assert!(!is_file(fs.abs_path("1.txt")));
    assert!(!is_dir(fs.abs_path("a")));
    assert!(!is_file(fs.abs_path("z/1.txt")));
    assert!(is_file(fs.abs_path("z/2.txt")));
    assert!(is_file(fs.abs_path("foo")));
    assert_eq!(read_file(&fs.abs_path("z/2.txt")), "new 2");
    assert_eq!(read_file(&fs.abs_path("foo")), "foo");
    assert_eq!(read_file(&fs.abs_path(".dot/file.txt")), "new file");
}

#[test]
fn test_autosave_is_removed_when_saving() {
    let f = setup();
    let fp = f.populated_dir.path_to(".autosave");
    let fs = open(&f.populated_dir, true);
    fs.autosave().unwrap();
    assert!(fp.is_existing_dir());
    fs.save().unwrap();
    assert!(!fp.is_existing_dir());
}

#[test]
fn test_autosave_is_removed_in_destructor() {
    let f = setup();
    let fp = f.populated_dir.path_to(".autosave");
    {
        let fs = open(&f.populated_dir, true);
        fs.autosave().unwrap();
        assert!(fp.is_existing_dir());
    }
    assert!(!fp.is_existing_dir());
}

#[test]
fn test_restore_autosave() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    check_initial_state(&fs);
    do_some_file_operations(&fs);
    check_state_in_memory(&fs);
    check_state_on_disk_unsaved(&fs);

    // Perform autosave.
    fs.autosave().unwrap();

    // Remove lock because we can't get a stale lock without crashing the app.
    file_utils::remove_file(&f.populated_dir.path_to(".lock")).unwrap();

    // Open another file system on the same directory to restore the autosave.
    let fs2 =
        TransactionalFileSystem::open(&f.populated_dir, true, RestoreMode::Yes, None).unwrap();
    assert!(fs2.is_restored_from_autosave());
    check_state_in_memory(&fs2);
    check_state_on_disk_unsaved(&fs2);

    // Save to file system.
    fs2.save().unwrap();
    check_state_on_disk_saved(&fs2);
}

#[test]
fn test_restore_mode_ask_and_abort() {
    let f = setup();
    {
        let fs = open(&f.populated_dir, true);
        fs.write("new", b"new").unwrap();
        fs.autosave().unwrap();
        fs.release_lock().unwrap(); // Keeps the autosave on drop.
    }

    // Abort.
    let result = TransactionalFileSystem::open(&f.populated_dir, false, RestoreMode::Abort, None);
    assert!(result.is_err());

    // Ask: no.
    let mut asked = None;
    let mut ask_no = |dir: &FilePath| {
        asked = Some(dir.clone());
        Ok(false)
    };
    let fs =
        TransactionalFileSystem::open(&f.populated_dir, false, RestoreMode::Ask(&mut ask_no), None)
            .unwrap();
    assert_eq!(asked.as_ref(), Some(&f.populated_dir));
    assert!(!fs.is_restored_from_autosave());
    assert!(!fs.file_exists("new"));

    // Ask: yes.
    let mut ask_yes = |_: &FilePath| Ok(true);
    let fs = TransactionalFileSystem::open(
        &f.populated_dir,
        false,
        RestoreMode::Ask(&mut ask_yes),
        None,
    )
    .unwrap();
    assert!(fs.is_restored_from_autosave());
    assert_eq!(read(&fs, "new"), "new");
}

#[test]
fn test_failed_open_keeps_autosave() {
    let f = setup();
    let autosave = f.populated_dir.path_to(".autosave");
    let fs = open(&f.populated_dir, true);
    fs.autosave().unwrap();

    // Opening the locked directory fails, which must not remove the autosave.
    assert!(TransactionalFileSystem::open_rw(&f.populated_dir).is_err());
    assert!(autosave.is_existing_dir());
}

/// Autosave index file format as written by upstream LibrePCB.
#[test]
fn test_autosave_file_format() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    fs.write("b.txt", b"b").unwrap();
    fs.write("a/x.txt", b"x").unwrap();
    fs.remove_file("1.txt").unwrap();
    fs.remove_dir_recursively("1").unwrap();
    fs.autosave().unwrap();
    let content = String::from_utf8(
        file_utils::read_file(&f.populated_dir.path_to(".autosave/autosave.lp")).unwrap(),
    )
    .unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines[0], "(librepcb_autosave");
    assert!(lines[1].starts_with(" (created 20") && lines[1].ends_with("Z)"));
    assert!(lines[2].starts_with(" (modified_files_directory \"20"));
    assert_eq!(
        &lines[3..],
        [
            " (modified_file \"a/x.txt\")",
            " (modified_file \"b.txt\")",
            " (removed_file \"1.txt\")",
            " (removed_directory \"1/\")",
            ")",
        ]
    );
    let dir_name = lines[2]
        .trim_start_matches(" (modified_files_directory \"")
        .trim_end_matches("\")");
    assert_eq!(dir_name.len(), "yyyy-MM-dd_hh-mm-ss-zzz".len());
    let files_dir = f.populated_dir.path_to(".autosave").path_to(dir_name);
    assert_eq!(
        file_utils::read_file(&files_dir.path_to("a/x.txt")).unwrap(),
        b"x"
    );
    assert_eq!(
        file_utils::read_file(&files_dir.path_to("b.txt")).unwrap(),
        b"b"
    );
}

#[test]
fn test_restored_backup_after_failed_save() {
    let f = setup();
    let backup_dir = f.populated_dir.path_to(".backup");

    {
        let fs = open(&f.populated_dir, true);
        fs.write("x/y/z", b"z").unwrap(); // create new file
        fs.write("1.txt", b"new 1").unwrap(); // overwrite existing file
        fs.remove_file("2.txt").unwrap(); // remove existing file
        fs.remove_dir_recursively("a").unwrap(); // remove existing directory

        // Create a directory where x/y/z would be saved to -> leads to an
        // error when saving the file system.
        file_utils::make_path(&f.populated_dir.path_to("x/y/z")).unwrap();

        // Save must now fail and the ".backup" directory must persist.
        assert!(fs.save().is_err());
        assert!(backup_dir.is_existing_dir());
    }

    for _ in 0..2 {
        // Opening the file system must automatically restore the backup.
        let fs = open(&f.populated_dir, true);
        assert_eq!(read(&fs, "x/y/z"), "z");
        assert_eq!(read(&fs, "1.txt"), "new 1");
        assert!(!fs.file_exists("2.txt"));
        assert!(!contains(fs.dirs(""), "a"));
        assert!(backup_dir.is_existing_dir());
    }

    {
        // Remove the directory now, save file system and the backup must be
        // removed.
        file_utils::remove_dir_recursively(&f.populated_dir.path_to("x/y/z")).unwrap();
        let fs = open(&f.populated_dir, true);
        fs.save().unwrap();
        assert!(!backup_dir.is_existing_dir());
    }

    // Check if files are written to disk.
    let p = &f.populated_dir;
    assert_eq!(file_utils::read_file(&p.path_to("x/y/z")).unwrap(), b"z");
    assert_eq!(
        file_utils::read_file(&p.path_to("1.txt")).unwrap(),
        b"new 1"
    );
    assert!(!p.path_to("2.txt").is_existing_file());
    assert!(!p.path_to("a").is_existing_dir());
    assert!(!backup_dir.is_existing_dir());
}

#[test]
fn test_export_import_zip_by_file_path() {
    let f = setup();
    let zip_fp = f.populated_dir.path_to("export to.zip");
    assert!(!zip_fp.is_existing_file());
    {
        let fs = open(&f.populated_dir, true);
        fs.export_to_zip_file(&zip_fp, None).unwrap();
        assert!(zip_fp.is_existing_file());
    }
    {
        let fs = open(&f.empty_dir, true);
        fs.load_from_zip(&zip_fp).unwrap();
        assert_eq!(read(&fs, "foo dir/bar dir.txt"), "bar");
    }
}

#[test]
fn test_export_zip_by_file_path_with_filter() {
    let f = setup();
    let zip_fp = f.populated_dir.path_to("export to filter.zip");
    let fs = open(&f.populated_dir, true);
    let filter = |fp: &str| fp == "1.txt" || fp == "1/1a.txt";
    fs.export_to_zip_file(&zip_fp, Some(&filter)).unwrap();

    let zip = ZipArchive::open(&zip_fp).unwrap();
    assert_eq!(zip.len(), 2);
}

#[test]
fn test_export_import_zip_by_byte_array() {
    let f = setup();
    let content = {
        let fs = open(&f.populated_dir, true);
        fs.export_to_zip(None).unwrap()
    };
    {
        let fs = open(&f.empty_dir, true);
        fs.load_from_zip_bytes(content).unwrap();
        assert_eq!(read(&fs, "foo dir/bar dir.txt"), "bar");
    }
}

#[test]
fn test_export_zip_by_byte_array_with_filter() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    let filter = |fp: &str| fp == "1.txt" || fp == "1/1a.txt";
    let content = fs.export_to_zip(Some(&filter)).unwrap();

    let zip = ZipArchive::from_bytes(content).unwrap();
    assert_eq!(zip.len(), 2);
}

#[test]
fn test_export_zip_skips_dot_dirs_and_lock_file() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    fs.write(".hidden file", b"h").unwrap();
    let mut zip = ZipArchive::from_bytes(fs.export_to_zip(None).unwrap()).unwrap();
    let names: BTreeSet<String> = (0..zip.len()).map(|i| zip.file_name(i).unwrap()).collect();
    let expected: BTreeSet<String> = [
        ".hidden file",
        "1.txt",
        "2.txt",
        "1/1a.txt",
        "1/1b.txt",
        "1/2/3/4.txt",
        "a/b/c",
        "foo dir/bar dir.txt",
        "foo dir/bar dir/X",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(names, expected);
}

#[test]
fn test_discard_changes() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    check_initial_state(&fs);
    do_some_file_operations(&fs);

    // Discard all changes.
    fs.discard_changes();

    // Check state in memory.
    check_initial_state(&fs);

    // Save to file system.
    fs.save().unwrap();

    // Check state on file system.
    check_state_on_disk_unsaved(&fs);
}

#[test]
fn test_save_and_restore_state() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    fs.write("new", b"new").unwrap();
    let state = fs.save_state();
    fs.remove_file("new").unwrap();
    assert!(!fs.file_exists("new"));
    fs.restore_state(state);
    assert_eq!(read(&fs, "new"), "new");
}

#[test]
fn test_check_for_modifications() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    check_initial_state(&fs);
    do_some_file_operations(&fs);

    // Check modifications.
    let mut modified = fs.check_for_modifications().unwrap();
    modified.sort();
    assert_eq!(
        modified,
        [".dot/file.txt", "1.txt", "a/", "x/y/z", "z/2.txt"]
    );

    // Save to file system.
    fs.save().unwrap();

    // Check modifications, should be empty now.
    assert!(fs.check_for_modifications().unwrap().is_empty());
}

#[test]
fn test_release_lock() {
    let f = setup();
    let lock_fp = f.populated_dir.path_to(".lock");

    let fs = open(&f.populated_dir, true);
    assert!(lock_fp.is_existing_file());
    fs.write("foo", b"x").unwrap(); // Create new file.
    fs.save().unwrap();
    fs.write("bar", b"x").unwrap(); // Create new file.
    fs.release_lock().unwrap();
    assert!(!lock_fp.is_existing_file());
    fs.release_lock().unwrap(); // Second call should do nothing.
    assert!(!lock_fp.is_existing_file());
    fs.write("foobar", b"x").unwrap(); // Create new file.
    assert!(fs.save().is_err()); // Failed because it's read-only.
}

#[test]
fn test_rename_file() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    fs.rename_file("1.txt", "renamed.txt").unwrap();
    assert!(!fs.file_exists("1.txt"));
    assert_eq!(read(&fs, "renamed.txt"), "1");
}

// ---------------------------------------------------------------------------
//  Security Tests: Sandbox Breakout
// ---------------------------------------------------------------------------

// These tests make sure that any file operation outside the file system (i.e.
// with too many "../" in the path) will fail. This is important for security
// reasons (sandbox breakout).

#[test]
fn test_get_abs_path_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    assert!(fs.abs_path("../1.txt").is_none());
}

#[test]
fn test_get_dirs_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    assert!(!file_utils::find_directories(&f.tmp_dir).is_empty());
    assert!(fs.dirs("../").is_empty());
}

#[test]
fn test_get_files_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    assert!(
        !file_utils::files_in_directory(&f.tmp_dir, &[], false, false)
            .unwrap()
            .is_empty()
    );
    assert!(fs.files("../").is_empty());
}

#[test]
fn test_file_exists_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    assert!(
        f.populated_dir
            .parent_dir()
            .unwrap()
            .path_to("1.txt")
            .is_existing_file()
    );
    assert!(!fs.file_exists("../1.txt"));
}

#[test]
fn test_read_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, false);
    let outside = f.populated_dir.parent_dir().unwrap().path_to("1.txt");
    assert_eq!(file_utils::read_file(&outside).unwrap(), b"1");
    assert_eq!(read(&fs, "1.txt"), "1");
    assert!(fs.read("../1.txt").is_err());
    assert!(fs.read("../populated/1.txt").is_err());
    assert!(fs.read_if_exists("../1.txt").is_err());
    assert!(fs.read_if_exists("../populated/1.txt").is_err());
    assert!(fs.read("..\\1.txt").is_err());
    assert!(fs.read("a\\..\\..\\1.txt").is_err());
}

#[test]
fn test_write_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    assert!(fs.write("../new", b"new").is_err());
    assert!(fs.write("../populated/new", b"new").is_err());
    assert!(
        !f.populated_dir
            .parent_dir()
            .unwrap()
            .path_to("new")
            .is_existing_file()
    );
    assert!(!fs.file_exists("new"));
}

#[test]
fn test_rename_file_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    assert!(fs.rename_file("../1.txt", "new").is_err());
    assert!(fs.rename_file("1.txt", "../new").is_err());
    assert!(!fs.file_exists("new"));
    assert!(
        !f.populated_dir
            .parent_dir()
            .unwrap()
            .path_to("new")
            .is_existing_file()
    );
}

#[test]
fn test_remove_file_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    assert!(fs.remove_file("../1.txt").is_err());
    assert!(
        f.populated_dir
            .parent_dir()
            .unwrap()
            .path_to("1.txt")
            .is_existing_file()
    );
}

#[test]
fn test_remove_dir_recursively_breakout() {
    let f = setup();
    let fs = open(&f.populated_dir, true);
    assert!(fs.remove_dir_recursively("../").is_err());
    assert!(
        f.populated_dir
            .parent_dir()
            .unwrap()
            .path_to("1.txt")
            .is_existing_file()
    );
}

// ---------------------------------------------------------------------------
//  Parametrized dirs() / files() / file_exists() / read() tests
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const SUB_DIRS_DATA: &[(&str, &str, &[&str])] = &[
    // root,        relPath,           entries
    ("nonexisting", "",                &[]),
    ("nonexisting", "foo",             &[]),
    ("nonexisting", "foo/bar",         &[]),
    ("empty",       "",                &[]),
    ("empty",       "foo",             &[]),
    ("empty",       "foo/bar",         &[]),
    ("populated",   "",                &[".dot", "1", "a", "foo dir"]),
    ("populated",   ".dot",            &["dir"]),
    ("populated",   ".dot/dir",        &[]),
    ("populated",   "1",               &["2"]),
    ("populated",   "1/2",             &["3"]),
    ("populated",   "1/2/3",           &[]),
    ("populated",   "1/2/3/4",         &[]),
    ("populated",   "a",               &["b"]),
    ("populated",   "a/b",             &[]),
    ("populated",   "foo dir",         &["bar dir"]),
    ("populated",   "foo dir/bar dir", &[]),
    ("populated",   "2",               &[]),
    ("populated",   "3",               &[]),
    ("populated",   "b",               &[]),
    ("populated",   "c",               &[]),
    ("populated",   "bar dir",         &[]),
    ("populated",   "hello",           &[]),
];

#[rustfmt::skip]
const FILES_IN_DIR_DATA: &[(&str, &str, &[&str])] = &[
    // root,        relPath,           entries
    ("nonexisting", "",                &[]),
    ("nonexisting", "foo",             &[]),
    ("nonexisting", "foo/bar",         &[]),
    ("empty",       "",                &[]),
    ("empty",       "foo",             &[]),
    ("empty",       "foo/bar",         &[]),
    ("populated",   "",                &["1.txt", "2.txt"]),
    ("populated",   ".dot",            &["file.txt"]),
    ("populated",   ".dot/dir",        &["foo.txt"]),
    ("populated",   "1",               &["1a.txt", "1b.txt"]),
    ("populated",   "1/2",             &[]),
    ("populated",   "1/2/3",           &["4.txt"]),
    ("populated",   "1/2/3/4",         &[]),
    ("populated",   "a",               &[]),
    ("populated",   "a/b",             &["c"]),
    ("populated",   "foo dir",         &["bar dir.txt"]),
    ("populated",   "foo dir/bar dir", &["X"]),
    ("populated",   "2",               &[]),
    ("populated",   "3",               &[]),
    ("populated",   "b",               &[]),
    ("populated",   "c",               &[]),
    ("populated",   "bar dir",         &[]),
    ("populated",   "hello",           &[]),
];

#[rustfmt::skip]
const FILE_EXISTS_DATA: &[(&str, &str, Option<&str>)] = &[
    // root,        relPath,               content (None = non-existing file)
    ("nonexisting", "",                    None),
    ("nonexisting", "foo",                 None),
    ("empty",       "",                    None),
    ("empty",       "foo/bar",             None),
    ("populated",   "",                    None),
    ("populated",   "1.txt",               Some("1")),
    ("populated",   "2.txt",               Some("2")),
    ("populated",   ".dot/file.txt",       Some("file")),
    ("populated",   ".dot/dir/foo.txt",    Some("foo")),
    ("populated",   "1",                   None),
    ("populated",   "1/1a.txt",            Some("1a")),
    ("populated",   "1/1b.txt",            Some("1b")),
    ("populated",   "1/2",                 None),
    ("populated",   "1/2/3/4.txt",         Some("4")),
    ("populated",   "1/2/3/4",             None),
    ("populated",   "a",                   None),
    ("populated",   "a/b/c",               Some("c")),
    ("populated",   "foo dir/bar dir.txt", Some("bar")),
    ("populated",   "foo dir/bar dir/X",   Some("X")),
    ("populated",   "2",                   None),
    ("populated",   "hello",               None),
];

fn set(items: impl IntoIterator<Item = impl Into<String>>) -> BTreeSet<String> {
    items.into_iter().map(Into::into).collect()
}

#[test]
fn test_get_sub_dirs() {
    let f = setup();
    for &(root, rel_path, entries) in SUB_DIRS_DATA {
        let fs = open(&f.tmp_dir.path_to(root), false);
        let dirs = fs.dirs(rel_path);
        assert_eq!(dirs.len(), entries.len(), "{root}/{rel_path}");
        assert_eq!(set(dirs), set(entries.iter().copied()), "{root}/{rel_path}");
    }
}

#[test]
fn test_get_files_in_dir() {
    let f = setup();
    for &(root, rel_path, entries) in FILES_IN_DIR_DATA {
        let fs = open(&f.tmp_dir.path_to(root), false);
        let files = fs.files(rel_path);
        assert_eq!(files.len(), entries.len(), "{root}/{rel_path}");
        assert_eq!(
            set(files),
            set(entries.iter().copied()),
            "{root}/{rel_path}"
        );
    }
}

#[test]
fn test_file_exists() {
    let f = setup();
    for &(root, rel_path, content) in FILE_EXISTS_DATA {
        let fs = open(&f.tmp_dir.path_to(root), false);
        assert_eq!(
            fs.file_exists(rel_path),
            content.is_some(),
            "{root}/{rel_path}"
        );
    }
}

#[test]
fn test_read() {
    let f = setup();
    for &(root, rel_path, content) in FILE_EXISTS_DATA {
        let fs = open(&f.tmp_dir.path_to(root), false);
        match content {
            None => assert!(fs.read(rel_path).is_err(), "{root}/{rel_path}"),
            Some(content) => assert_eq!(read(&fs, rel_path), content),
        }
    }
}

#[test]
fn test_read_if_exists() {
    let f = setup();
    for &(root, rel_path, content) in FILE_EXISTS_DATA {
        let fs = open(&f.tmp_dir.path_to(root), false);
        assert_eq!(
            fs.read_if_exists(rel_path).unwrap(),
            content.map(|c| c.as_bytes().to_vec()),
            "{root}/{rel_path}"
        );
    }
}

// ---------------------------------------------------------------------------
//  Parametrized clean_path() tests
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const CLEAN_PATH_DATA: &[(&str, &str)] = &[
    // input,                          output
    ("",                               ""),
    ("   ",                            ""),
    (".",                              ""),
    ("..",                             ".."),
    ("../",                            ".."),
    ("foo bar",                        "foo bar"),
    ("/foo\\\\bar/",                   "foo/bar"),
    (" /hello world/foo bar/.txt ",    "hello world/foo bar/.txt"),
    ("///HELLO/\\\\/FOO///",           "HELLO/FOO"),
    ("  /\\  Hello World  \\/  ",      "Hello World"),
    ("foo/../bar",                     "bar"),
    ("foo/bar/../././.",               "foo"),
    ("./foo/bar/hello/../..",          "foo"),
    ("./foo/bar/hello/../../",         "foo"),
];

#[test]
fn test_clean_path() {
    for &(input, output) in CLEAN_PATH_DATA {
        assert_eq!(
            TransactionalFileSystem::clean_path(input),
            output,
            "{input:?}"
        );
    }
}
