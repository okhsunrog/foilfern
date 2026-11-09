//! Port of tests/unittests/core/workspace/workspacelibrarydbtest.cpp.

use std::collections::{BTreeSet, HashMap};

use librepcb_core::fileio::FilePath;
use librepcb_core::sqlite_database::SqliteDatabase;
use librepcb_core::types::{Uuid, Version};
use librepcb_core::workspace::{
    CategoryInfo, DeviceInfo, ElementInfo, ElementKind, LibraryDb, LibraryDbWriter, Translations,
};

use crate::helpers::TempDir;

use ElementKind::{
    Component, ComponentCategory, Device, Library, Package, PackageCategory, Symbol,
};

struct Fixture {
    tmp: TempDir,
    ws_db: LibraryDb,
    db: SqliteDatabase,
    uuids: HashMap<i32, Uuid>,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new();
        let ws_db = LibraryDb::open(tmp.path()).unwrap();
        let db = SqliteDatabase::open(ws_db.file_path().as_path()).unwrap();
        Self {
            tmp,
            ws_db,
            db,
            uuids: HashMap::new(),
        }
    }

    fn writer(&self) -> LibraryDbWriter<'_> {
        LibraryDbWriter::new(self.tmp.path(), &self.db)
    }

    fn abs(&self, fp: &str) -> FilePath {
        self.tmp.path().path_to(fp)
    }

    /// Returns a random UUID for `index`, the same for the same index.
    fn uuid(&mut self, index: i32) -> Uuid {
        *self.uuids.entry(index).or_insert_with(Uuid::new_random)
    }
}

fn rnd() -> Uuid {
    Uuid::new_random()
}

fn v(s: &str) -> Version {
    s.parse().unwrap()
}

fn set(uuids: &[Uuid]) -> BTreeSet<Uuid> {
    uuids.iter().copied().collect()
}

/// Formats a version/path list like upstream's `str(QMultiMap)`.
fn str_map(f: &Fixture, list: &[(Version, FilePath)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(version, fp)| (version.to_string(), fp.to_relative(f.tmp.path())))
        .collect()
}

fn expected(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(v, p)| ((*v).to_owned(), (*p).to_owned()))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests for all()
// ---------------------------------------------------------------------------

const ALL_KINDS: [ElementKind; 7] = [
    Library,
    ComponentCategory,
    PackageCategory,
    Symbol,
    Package,
    Component,
    Device,
];

#[test]
fn test_get_all_empty_db() {
    let f = Fixture::new();
    for kind in ALL_KINDS {
        assert!(f.ws_db.all(kind, None, None).unwrap().is_empty());
    }
}

#[test]
fn test_get_all_empty_db_with_uuid() {
    let f = Fixture::new();
    for kind in ALL_KINDS {
        assert!(f.ws_db.all(kind, Some(rnd()), None).unwrap().is_empty());
    }
}

#[test]
fn test_get_all_empty_db_with_library() {
    let f = Fixture::new();
    let lib = f.abs("lib");
    // Library filter with libraries is not possible.
    assert!(f.ws_db.all(Library, None, Some(&lib)).is_err());
    for kind in &ALL_KINDS[1..] {
        assert!(f.ws_db.all(*kind, None, Some(&lib)).unwrap().is_empty());
    }
}

#[test]
fn test_get_all_empty_db_with_uuid_and_library() {
    let f = Fixture::new();
    let lib = f.abs("lib");
    for kind in &ALL_KINDS[1..] {
        assert!(
            f.ws_db
                .all(*kind, Some(rnd()), Some(&lib))
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn test_get_all() {
    let f = Fixture::new();
    let w = f.writer();
    for i in 1..=2 {
        let n = i.to_string();
        w.add_library(
            &f.abs(&format!("lib{n}")),
            rnd(),
            &v(&format!("0.1.{n}")),
            false,
            &[],
            "",
        )
        .unwrap();
        w.add_category(
            ComponentCategory,
            0,
            &f.abs(&format!("cmpcat{n}")),
            rnd(),
            &v(&format!("0.2.{n}")),
            false,
            None,
        )
        .unwrap();
        w.add_category(
            PackageCategory,
            0,
            &f.abs(&format!("pkgcat{n}")),
            rnd(),
            &v(&format!("0.3.{n}")),
            false,
            None,
        )
        .unwrap();
        w.add_element(
            Symbol,
            0,
            &f.abs(&format!("sym{n}")),
            rnd(),
            &v(&format!("0.4.{n}")),
            false,
            "",
        )
        .unwrap();
        w.add_element(
            Package,
            0,
            &f.abs(&format!("pkg{n}")),
            rnd(),
            &v(&format!("0.5.{n}")),
            false,
            "",
        )
        .unwrap();
        w.add_element(
            Component,
            0,
            &f.abs(&format!("cmp{n}")),
            rnd(),
            &v(&format!("0.6.{n}")),
            false,
            "",
        )
        .unwrap();
        w.add_device(
            0,
            &f.abs(&format!("dev{n}")),
            rnd(),
            &v(&format!("0.7.{n}")),
            false,
            "",
            rnd(),
            rnd(),
        )
        .unwrap();
    }

    for (kind, prefix, name) in [
        (Library, "0.1", "lib"),
        (ComponentCategory, "0.2", "cmpcat"),
        (PackageCategory, "0.3", "pkgcat"),
        (Symbol, "0.4", "sym"),
        (Package, "0.5", "pkg"),
        (Component, "0.6", "cmp"),
        (Device, "0.7", "dev"),
    ] {
        assert_eq!(
            str_map(&f, &f.ws_db.all(kind, None, None).unwrap()),
            expected(&[
                (&format!("{prefix}.1"), &format!("{name}1")),
                (&format!("{prefix}.2"), &format!("{name}2")),
            ])
        );
    }
}

// Further tests only check with Symbol, since the implementation is the same
// for all library element types.

fn add_duplicates(f: &mut Fixture) {
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    let w = f.writer();
    let lib1 = w
        .add_library(&f.abs("lib1"), rnd(), &v("1"), false, &[], "")
        .unwrap();
    let lib2 = w
        .add_library(&f.abs("lib2"), rnd(), &v("2"), false, &[], "")
        .unwrap();
    w.add_element(Symbol, lib1, &f.abs("sym1"), u1, &v("0.1"), false, "")
        .unwrap();
    w.add_element(Symbol, lib1, &f.abs("sym2"), u2, &v("0.2"), false, "")
        .unwrap();
    w.add_element(Symbol, lib2, &f.abs("sym3"), u1, &v("0.3"), false, "")
        .unwrap();
    w.add_element(Symbol, lib2, &f.abs("sym4"), u2, &v("0.2"), false, "")
        .unwrap();
}

#[test]
fn test_get_all_with_duplicates() {
    let mut f = Fixture::new();
    add_duplicates(&mut f);
    // Like upstream's QMultiMap: equal versions in reverse insertion order.
    assert_eq!(
        str_map(&f, &f.ws_db.all(Symbol, None, None).unwrap()),
        expected(&[
            ("0.1", "sym1"),
            ("0.2", "sym4"),
            ("0.2", "sym2"),
            ("0.3", "sym3")
        ])
    );
}

#[test]
fn test_get_all_with_uuid() {
    let mut f = Fixture::new();
    add_duplicates(&mut f);
    let u1 = f.uuid(1);
    assert_eq!(
        str_map(&f, &f.ws_db.all(Symbol, Some(u1), None).unwrap()),
        expected(&[("0.1", "sym1"), ("0.3", "sym3")])
    );
}

#[test]
fn test_get_all_with_library() {
    let mut f = Fixture::new();
    add_duplicates(&mut f);
    assert_eq!(
        str_map(
            &f,
            &f.ws_db.all(Symbol, None, Some(&f.abs("lib2"))).unwrap()
        ),
        expected(&[("0.2", "sym4"), ("0.3", "sym3")])
    );
}

#[test]
fn test_get_all_with_uuid_and_library() {
    let mut f = Fixture::new();
    add_duplicates(&mut f);
    let u1 = f.uuid(1);
    assert_eq!(
        str_map(
            &f,
            &f.ws_db.all(Symbol, Some(u1), Some(&f.abs("lib2"))).unwrap()
        ),
        expected(&[("0.3", "sym3")])
    );
}

#[test]
fn test_get_all_in_library() {
    let mut f = Fixture::new();
    add_duplicates(&mut f);
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    assert_eq!(
        f.ws_db.all_in_library(Symbol, &f.abs("lib1")).unwrap(),
        HashMap::from([(f.abs("sym1"), u1), (f.abs("sym2"), u2)])
    );
}

// ---------------------------------------------------------------------------
// Tests for latest()
// ---------------------------------------------------------------------------

#[test]
fn test_get_latest_empty_db() {
    let f = Fixture::new();
    assert_eq!(f.ws_db.latest(Symbol, rnd()).unwrap(), None);
}

#[test]
fn test_get_latest() {
    let mut f = Fixture::new();
    let u0 = f.uuid(0);
    let w = f.writer();
    let lib1 = w
        .add_library(&f.abs("lib1"), rnd(), &v("1"), false, &[], "")
        .unwrap();
    let lib2 = w
        .add_library(&f.abs("lib2"), rnd(), &v("2"), false, &[], "")
        .unwrap();
    w.add_element(Symbol, lib1, &f.abs("sym1"), u0, &v("0.1"), false, "")
        .unwrap();
    w.add_element(Symbol, lib1, &f.abs("sym2"), u0, &v("0.2"), false, "")
        .unwrap();
    w.add_element(Symbol, lib2, &f.abs("sym3"), u0, &v("0.3"), false, "")
        .unwrap();
    w.add_element(Symbol, lib2, &f.abs("sym4"), u0, &v("0.2"), false, "")
        .unwrap();
    assert_eq!(f.ws_db.latest(Symbol, u0).unwrap(), Some(f.abs("sym3")));
    assert_eq!(
        f.ws_db.element_dir(Symbol, u0).unwrap(),
        Some(f.abs("sym3"))
    );
}

// ---------------------------------------------------------------------------
// Tests for find()
// ---------------------------------------------------------------------------

#[test]
fn test_find_empty_db() {
    let f = Fixture::new();
    assert!(f.ws_db.find(Symbol, "foo").unwrap().is_empty());
}

#[test]
fn test_find_empty_keyword() {
    let f = Fixture::new();
    let w = f.writer();
    let lib = w
        .add_library(&f.abs("lib"), rnd(), &v("1"), false, &[], "")
        .unwrap();
    let sym = w
        .add_element(Symbol, lib, &f.abs("sym1"), rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_translation(
        Symbol,
        sym,
        "",
        Some("some name"),
        Some("some desc"),
        Some("some keywords"),
    )
    .unwrap();
    assert!(f.ws_db.find(Symbol, "foo").unwrap().is_empty());
}

fn add_symbol_with_tr(
    f: &Fixture,
    lib: i64,
    path: &str,
    uuid: Uuid,
    version: &str,
    n: &str,
) -> i64 {
    let w = f.writer();
    let sym = w
        .add_element(Symbol, lib, &f.abs(path), uuid, &v(version), false, "")
        .unwrap();
    w.add_translation(
        Symbol,
        sym,
        "",
        Some(&format!("the {n} name")),
        Some(&format!("the {n} desc")),
        Some(&format!("the {n} keywords")),
    )
    .unwrap();
    sym
}

#[test]
fn test_find() {
    let mut f = Fixture::new();
    let (u1, u2, u3) = (f.uuid(1), f.uuid(2), f.uuid(3));
    let lib = f
        .writer()
        .add_library(&f.abs("lib"), rnd(), &v("1"), false, &[], "")
        .unwrap();
    add_symbol_with_tr(&f, lib, "sym1", u1, "0.1", "sym1");
    add_symbol_with_tr(&f, lib, "sym2", u2, "0.2", "sym2");
    add_symbol_with_tr(&f, lib, "sym3", u3, "0.3", "sym3");

    assert_eq!(f.ws_db.find(Symbol, "name").unwrap(), vec![u1, u2, u3]);
    assert_eq!(f.ws_db.find(Symbol, "sym1 name").unwrap(), vec![u1]);
    assert_eq!(f.ws_db.find(Symbol, "sym3 keywords").unwrap(), vec![u3]);
    // Descriptions are not taken into account to avoid way too verbose
    // results!
    assert!(f.ws_db.find(Symbol, "sym2 desc").unwrap().is_empty());
    // Exact UUID matches.
    assert_eq!(f.ws_db.find(Symbol, &u2.to_string()).unwrap(), vec![u2]);
}

#[test]
fn test_find_with_duplicates() {
    let mut f = Fixture::new();
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    let lib = f
        .writer()
        .add_library(&f.abs("lib"), rnd(), &v("1"), false, &[], "")
        .unwrap();
    add_symbol_with_tr(&f, lib, "sym1", u1, "0.1", "sym1");
    add_symbol_with_tr(&f, lib, "sym2", u1, "0.2", "sym2");
    add_symbol_with_tr(&f, lib, "sym3", u2, "0.3", "sym3");

    assert_eq!(f.ws_db.find(Symbol, "name").unwrap(), vec![u1, u2]);
    assert_eq!(f.ws_db.find(Symbol, "sym1 name").unwrap(), vec![u1]);
}

#[test]
fn test_find_with_multiple_translations() {
    let mut f = Fixture::new();
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    let lib = f
        .writer()
        .add_library(&f.abs("lib"), rnd(), &v("1"), false, &[], "")
        .unwrap();
    let sym = add_symbol_with_tr(&f, lib, "sym1", u1, "0.1", "sym1");
    let w = f.writer();
    w.add_translation(
        Symbol,
        sym,
        "en_US",
        Some("the sym1 en_US name"),
        Some("the sym1 en_US desc"),
        Some("the sym1 en_US keywords"),
    )
    .unwrap();
    w.add_translation(
        Symbol,
        sym,
        "de_DE",
        Some("the sym1 de_DE name"),
        Some("the sym1 de_DE desc"),
        Some("the sym1 de_DE keywords"),
    )
    .unwrap();
    add_symbol_with_tr(&f, lib, "sym2", u2, "0.2", "sym2");

    assert_eq!(f.ws_db.find(Symbol, "name").unwrap(), vec![u1, u2]);
    assert_eq!(f.ws_db.find(Symbol, "sym1 name").unwrap(), vec![u1]);
    assert_eq!(f.ws_db.find(Symbol, "sym1 en_US name").unwrap(), vec![u1]);
}

// Not ported from upstream: packages are also found by alternative names.
#[test]
fn test_find_package_by_alternative_name() {
    let mut f = Fixture::new();
    let u1 = f.uuid(1);
    let w = f.writer();
    let pkg = w
        .add_element(Package, 0, &f.abs("pkg"), u1, &v("0.1"), false, "")
        .unwrap();
    w.add_translation(Package, pkg, "", Some("SOT23-3"), None, None)
        .unwrap();
    w.add_alternative_name(pkg, "TO-236AB", "JEDEC").unwrap();
    assert_eq!(f.ws_db.find(Package, "to-236").unwrap(), vec![u1]);
    assert_eq!(f.ws_db.find(Package, "sot23").unwrap(), vec![u1]);
    assert!(f.ws_db.find(Package, "JEDEC").unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Tests for translations()
// ---------------------------------------------------------------------------

fn tr(name: &str, description: &str, keywords: &str) -> Option<Translations> {
    Some(Translations {
        name: name.to_owned(),
        description: description.to_owned(),
        keywords: keywords.to_owned(),
    })
}

const NO_LOCALES: &[&str] = &[];

#[test]
fn test_get_translations_inexistent() {
    let f = Fixture::new();
    for kind in ALL_KINDS {
        assert_eq!(
            f.ws_db
                .translations(kind, &f.abs("fp"), NO_LOCALES)
                .unwrap(),
            None
        );
    }
}

#[test]
fn test_get_translations_empty() {
    let f = Fixture::new();
    let w = f.writer();
    let fp = f.abs("fp");
    let lib = w
        .add_library(&fp, rnd(), &v("0.1"), false, &[], "")
        .unwrap();
    w.add_category(ComponentCategory, lib, &fp, rnd(), &v("0.1"), false, None)
        .unwrap();
    w.add_category(PackageCategory, lib, &fp, rnd(), &v("0.1"), false, None)
        .unwrap();
    w.add_element(Symbol, lib, &fp, rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_element(Package, lib, &fp, rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_element(Component, lib, &fp, rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_device(lib, &fp, rnd(), &v("0.1"), false, "", rnd(), rnd())
        .unwrap();
    for kind in ALL_KINDS {
        assert_eq!(f.ws_db.translations(kind, &fp, NO_LOCALES).unwrap(), None);
    }
}

#[test]
fn test_get_translations_default_locale() {
    let f = Fixture::new();
    let w = f.writer();
    let fp = f.abs("fp");
    let lib = w
        .add_library(&fp, rnd(), &v("0.1"), false, &[], "")
        .unwrap();
    w.add_translation(
        Library,
        lib,
        "",
        Some("lib_n"),
        Some("lib_d"),
        Some("lib_k"),
    )
    .unwrap();
    let id = w
        .add_category(ComponentCategory, lib, &fp, rnd(), &v("0.1"), false, None)
        .unwrap();
    w.add_translation(
        ComponentCategory,
        id,
        "",
        Some("cmpcat_n"),
        Some("cmpcat_d"),
        Some("cmpcat_k"),
    )
    .unwrap();
    let id = w
        .add_category(PackageCategory, lib, &fp, rnd(), &v("0.1"), false, None)
        .unwrap();
    w.add_translation(
        PackageCategory,
        id,
        "",
        Some("pkgcat_n"),
        Some("pkgcat_d"),
        Some("pkgcat_k"),
    )
    .unwrap();
    let id = w
        .add_element(Symbol, lib, &fp, rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_translation(Symbol, id, "", Some("sym_n"), Some("sym_d"), Some("sym_k"))
        .unwrap();
    let id = w
        .add_element(Package, lib, &fp, rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_translation(Package, id, "", Some("pkg_n"), Some("pkg_d"), Some("pkg_k"))
        .unwrap();
    let id = w
        .add_element(Component, lib, &fp, rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_translation(
        Component,
        id,
        "",
        Some("cmp_n"),
        Some("cmp_d"),
        Some("cmp_k"),
    )
    .unwrap();
    let id = w
        .add_device(lib, &fp, rnd(), &v("0.1"), false, "", rnd(), rnd())
        .unwrap();
    w.add_translation(Device, id, "", Some("dev_n"), Some("dev_d"), Some("dev_k"))
        .unwrap();

    for (kind, p) in [
        (Library, "lib"),
        (ComponentCategory, "cmpcat"),
        (PackageCategory, "pkgcat"),
        (Symbol, "sym"),
        (Package, "pkg"),
        (Component, "cmp"),
        (Device, "dev"),
    ] {
        assert_eq!(
            f.ws_db.translations(kind, &fp, NO_LOCALES).unwrap(),
            tr(&format!("{p}_n"), &format!("{p}_d"), &format!("{p}_k"))
        );
    }
}

#[test]
fn test_get_translations_default_with_order() {
    let f = Fixture::new();
    let w = f.writer();
    let id = w
        .add_element(Symbol, 0, &f.abs("fp"), rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_translation(Symbol, id, "", Some("_n"), Some("_d"), Some("_k"))
        .unwrap();
    assert_eq!(
        f.ws_db
            .translations(Symbol, &f.abs("fp"), &["en_US", "zh_CN", "de_DE"])
            .unwrap(),
        tr("_n", "_d", "_k")
    );
}

fn add_multiple_translations(f: &Fixture, default_name: Option<&str>) {
    let w = f.writer();
    let id = w
        .add_element(Symbol, 0, &f.abs("fp"), rnd(), &v("0.1"), false, "")
        .unwrap();
    w.add_translation(Symbol, id, "de_DE", None, Some("de_d"), None)
        .unwrap();
    w.add_translation(Symbol, id, "", default_name, Some("_d"), Some("_k"))
        .unwrap();
    w.add_translation(Symbol, id, "en_US", Some("en_n"), None, None)
        .unwrap();
    w.add_translation(
        Symbol,
        id,
        "it_IT",
        Some("it_n"),
        Some("it_d"),
        Some("it_k"),
    )
    .unwrap();
}

#[test]
fn test_get_translations_multiple_without_order() {
    let f = Fixture::new();
    add_multiple_translations(&f, Some("_n"));
    assert_eq!(
        f.ws_db
            .translations(Symbol, &f.abs("fp"), NO_LOCALES)
            .unwrap(),
        tr("_n", "_d", "_k")
    );
}

#[test]
fn test_get_translations_multiple_with_order() {
    let f = Fixture::new();
    add_multiple_translations(&f, None);
    assert_eq!(
        f.ws_db
            .translations(Symbol, &f.abs("fp"), &["en_US", "zh_CN", "de_DE"])
            .unwrap(),
        tr("en_n", "de_d", "_k")
    );
}

// ---------------------------------------------------------------------------
// Tests for metadata()
// ---------------------------------------------------------------------------

#[test]
fn test_get_metadata_inexistent() {
    let f = Fixture::new();
    for kind in ALL_KINDS {
        assert_eq!(f.ws_db.metadata(kind, &f.abs("fp")).unwrap(), None);
    }
}

#[test]
fn test_get_metadata() {
    let mut f = Fixture::new();
    let u: Vec<Uuid> = (1..=7).map(|i| f.uuid(i)).collect();
    let fp = f.abs("fp");
    let w = f.writer();
    let lib = w.add_library(&fp, u[0], &v("1.1"), false, &[], "").unwrap();
    w.add_category(ComponentCategory, lib, &fp, u[1], &v("2.2"), true, None)
        .unwrap();
    w.add_category(PackageCategory, lib, &fp, u[2], &v("3.3"), false, None)
        .unwrap();
    w.add_element(Symbol, lib, &fp, u[3], &v("4.4"), true, "")
        .unwrap();
    w.add_element(Package, lib, &fp, u[4], &v("5.5"), false, "")
        .unwrap();
    w.add_element(Component, lib, &fp, u[5], &v("6.6"), true, "")
        .unwrap();
    w.add_device(lib, &fp, u[6], &v("7.7"), false, "", rnd(), rnd())
        .unwrap();

    for (i, (kind, deprecated)) in [
        (Library, false),
        (ComponentCategory, true),
        (PackageCategory, false),
        (Symbol, true),
        (Package, false),
        (Component, true),
        (Device, false),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            f.ws_db.metadata(kind, &fp).unwrap(),
            Some(ElementInfo {
                uuid: u[i],
                version: v(&format!("{0}.{0}", i + 1)),
                deprecated,
            })
        );
    }
}

// ---------------------------------------------------------------------------
// Tests for library_metadata()
// ---------------------------------------------------------------------------

#[test]
fn test_get_library_metadata_inexistent() {
    let f = Fixture::new();
    assert_eq!(f.ws_db.library_metadata(&f.abs("fp")).unwrap(), None);
}

#[test]
fn test_get_library_metadata_no_icon() {
    let f = Fixture::new();
    let fp = f.abs("fp");
    f.writer()
        .add_library(&fp, rnd(), &v("1.1"), false, &[], "Hello World!")
        .unwrap();
    let info = f.ws_db.library_metadata(&fp).unwrap().unwrap();
    assert!(info.icon_png.is_empty());
    assert_eq!(info.manufacturer, "Hello World!");
}

#[test]
fn test_get_library_metadata_with_icon() {
    let f = Fixture::new();
    let fp = f.abs("fp");
    f.writer()
        .add_library(&fp, rnd(), &v("1.1"), false, b"\x89PNG", "")
        .unwrap();
    let info = f.ws_db.library_metadata(&fp).unwrap().unwrap();
    assert_eq!(info.icon_png, b"\x89PNG");
    assert_eq!(info.manufacturer, "");
}

// ---------------------------------------------------------------------------
// Tests for category_metadata()
// ---------------------------------------------------------------------------

#[test]
fn test_get_category_metadata_empty_db() {
    let f = Fixture::new();
    assert_eq!(
        f.ws_db
            .category_metadata(ComponentCategory, &f.abs("fp"))
            .unwrap(),
        None
    );
    assert_eq!(
        f.ws_db
            .category_metadata(PackageCategory, &f.abs("fp"))
            .unwrap(),
        None
    );
    assert!(f.ws_db.category_metadata(Symbol, &f.abs("fp")).is_err());
}

#[test]
fn test_get_category_metadata_inexistent() {
    let f = Fixture::new();
    let w = f.writer();
    let lib = w
        .add_library(&f.abs("fp"), rnd(), &v("1.1"), false, &[], "")
        .unwrap();
    w.add_category(
        ComponentCategory,
        lib,
        &f.abs("fp"),
        rnd(),
        &v("2.2"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        PackageCategory,
        lib,
        &f.abs("fp"),
        rnd(),
        &v("3.3"),
        false,
        None,
    )
    .unwrap();
    assert_eq!(
        f.ws_db
            .category_metadata(ComponentCategory, &f.abs("foo"))
            .unwrap(),
        None
    );
    assert_eq!(
        f.ws_db
            .category_metadata(PackageCategory, &f.abs("foo"))
            .unwrap(),
        None
    );
}

#[test]
fn test_get_category_metadata() {
    let mut f = Fixture::new();
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    let w = f.writer();
    let lib = w
        .add_library(&f.abs("fp"), rnd(), &v("1.1"), false, &[], "")
        .unwrap();
    w.add_category(
        ComponentCategory,
        lib,
        &f.abs("fp1"),
        u1,
        &v("2.2"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        ComponentCategory,
        lib,
        &f.abs("fp2"),
        u2,
        &v("3.3"),
        false,
        Some(u1),
    )
    .unwrap();
    w.add_category(
        PackageCategory,
        lib,
        &f.abs("fp3"),
        u2,
        &v("4.4"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        PackageCategory,
        lib,
        &f.abs("fp4"),
        u1,
        &v("5.5"),
        false,
        Some(u2),
    )
    .unwrap();

    let meta = |kind, fp: &str| f.ws_db.category_metadata(kind, &f.abs(fp)).unwrap();
    assert_eq!(
        meta(ComponentCategory, "fp1"),
        Some(CategoryInfo { parent: None })
    );
    assert_eq!(
        meta(ComponentCategory, "fp2"),
        Some(CategoryInfo { parent: Some(u1) })
    );
    assert_eq!(
        meta(PackageCategory, "fp3"),
        Some(CategoryInfo { parent: None })
    );
    assert_eq!(
        meta(PackageCategory, "fp4"),
        Some(CategoryInfo { parent: Some(u2) })
    );
}

// ---------------------------------------------------------------------------
// Tests for device_metadata()
// ---------------------------------------------------------------------------

#[test]
fn test_get_device_metadata_inexistent() {
    let f = Fixture::new();
    assert_eq!(f.ws_db.device_metadata(&f.abs("fp")).unwrap(), None);
}

#[test]
fn test_get_device_metadata() {
    let mut f = Fixture::new();
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    let fp = f.abs("fp");
    f.writer()
        .add_device(0, &fp, rnd(), &v("1.1"), false, "", u1, u2)
        .unwrap();
    assert_eq!(
        f.ws_db.device_metadata(&fp).unwrap(),
        Some(DeviceInfo {
            component_uuid: u1,
            package_uuid: u2,
        })
    );
}

// ---------------------------------------------------------------------------
// Tests for children()
// ---------------------------------------------------------------------------

#[test]
fn test_get_children_empty_db() {
    let f = Fixture::new();
    assert!(
        f.ws_db
            .children(ComponentCategory, None)
            .unwrap()
            .is_empty()
    );
    assert!(f.ws_db.children(PackageCategory, None).unwrap().is_empty());
    assert!(f.ws_db.children(Symbol, None).is_err());
}

#[test]
fn test_get_children_inexistent() {
    let mut f = Fixture::new();
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    let w = f.writer();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat"),
        u1,
        &v("0.1"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        PackageCategory,
        0,
        &f.abs("pkgcat"),
        u2,
        &v("0.1"),
        false,
        None,
    )
    .unwrap();
    assert!(
        f.ws_db
            .children(ComponentCategory, Some(u2))
            .unwrap()
            .is_empty()
    );
    assert!(
        f.ws_db
            .children(PackageCategory, Some(u1))
            .unwrap()
            .is_empty()
    );
}

fn add_invalid_parents(f: &mut Fixture) {
    let (u1, u2, u3, u4) = (f.uuid(1), f.uuid(2), f.uuid(3), f.uuid(4));
    let w = f.writer();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat"),
        u1,
        &v("0.1"),
        false,
        Some(u2),
    )
    .unwrap();
    w.add_category(
        PackageCategory,
        0,
        &f.abs("pkgcat"),
        u3,
        &v("0.1"),
        false,
        Some(u4),
    )
    .unwrap();
}

#[test]
fn test_get_children_invalid_with_uuid() {
    let mut f = Fixture::new();
    add_invalid_parents(&mut f);
    let (u1, u2, u3, u4) = (f.uuid(1), f.uuid(2), f.uuid(3), f.uuid(4));
    assert_eq!(
        f.ws_db.children(ComponentCategory, Some(u2)).unwrap(),
        set(&[u1])
    );
    assert_eq!(
        f.ws_db.children(PackageCategory, Some(u4)).unwrap(),
        set(&[u3])
    );
}

#[test]
fn test_get_children_invalid_without_uuid() {
    let mut f = Fixture::new();
    add_invalid_parents(&mut f);
    let (u1, u3) = (f.uuid(1), f.uuid(3));
    assert_eq!(
        f.ws_db.children(ComponentCategory, None).unwrap(),
        set(&[u1])
    );
    assert_eq!(f.ws_db.children(PackageCategory, None).unwrap(), set(&[u3]));
}

fn add_duplicate_categories(f: &mut Fixture) {
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    let w = f.writer();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat1"),
        u1,
        &v("0.1"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat2"),
        u2,
        &v("0.1"),
        false,
        Some(u1),
    )
    .unwrap();
    w.add_category(
        ComponentCategory,
        1,
        &f.abs("cmpcat3"),
        u1,
        &v("0.1"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        ComponentCategory,
        1,
        &f.abs("cmpcat4"),
        u2,
        &v("0.1"),
        false,
        Some(u1),
    )
    .unwrap();
}

#[test]
fn test_get_children_duplicates_with_uuid() {
    let mut f = Fixture::new();
    add_duplicate_categories(&mut f);
    let (u1, u2) = (f.uuid(1), f.uuid(2));
    assert_eq!(
        f.ws_db.children(ComponentCategory, Some(u1)).unwrap(),
        set(&[u2])
    );
}

#[test]
fn test_get_children_duplicates_without_uuid() {
    let mut f = Fixture::new();
    add_duplicate_categories(&mut f);
    let u1 = f.uuid(1);
    assert_eq!(
        f.ws_db.children(ComponentCategory, None).unwrap(),
        set(&[u1])
    );
}

// ---------------------------------------------------------------------------
// Tests for by_category()
// ---------------------------------------------------------------------------

#[test]
fn test_get_by_category_empty_db() {
    let f = Fixture::new();
    for kind in [Symbol, Package, Component, Device] {
        assert!(f.ws_db.by_category(kind, None, None).unwrap().is_empty());
    }
    assert!(f.ws_db.by_category(Library, None, None).is_err());
}

#[test]
fn test_get_by_category_inexistent() {
    let f = Fixture::new();
    for kind in [Symbol, Package, Component, Device] {
        assert!(
            f.ws_db
                .by_category(kind, Some(rnd()), None)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn test_get_by_category() {
    let mut f = Fixture::new();
    let u: Vec<Uuid> = (1..=6).map(|i| f.uuid(i)).collect();
    let w = f.writer();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat"),
        u[0],
        &v("0.1"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        PackageCategory,
        0,
        &f.abs("pkgcat"),
        u[1],
        &v("0.1"),
        false,
        None,
    )
    .unwrap();
    let sym = w
        .add_element(Symbol, 0, &f.abs("sym"), u[2], &v("0.1"), false, "")
        .unwrap();
    w.add_to_category(Symbol, sym, u[0]).unwrap();
    let pkg = w
        .add_element(Package, 0, &f.abs("pkg"), u[3], &v("0.1"), false, "")
        .unwrap();
    w.add_to_category(Package, pkg, u[1]).unwrap();
    let cmp = w
        .add_element(Component, 0, &f.abs("cmp"), u[4], &v("0.1"), false, "")
        .unwrap();
    w.add_to_category(Component, cmp, u[0]).unwrap();
    let dev = w
        .add_device(0, &f.abs("dev"), u[5], &v("0.1"), false, "", rnd(), rnd())
        .unwrap();
    w.add_to_category(Device, dev, u[0]).unwrap();

    assert_eq!(
        f.ws_db.by_category(Symbol, Some(u[0]), None).unwrap(),
        set(&[u[2]])
    );
    assert_eq!(
        f.ws_db.by_category(Package, Some(u[1]), None).unwrap(),
        set(&[u[3]])
    );
    assert_eq!(
        f.ws_db.by_category(Component, Some(u[0]), None).unwrap(),
        set(&[u[4]])
    );
    assert_eq!(
        f.ws_db.by_category(Device, Some(u[0]), None).unwrap(),
        set(&[u[5]])
    );
    // Not ported from upstream: categories_of() and the limit.
    assert_eq!(
        f.ws_db.categories_of(Device, &f.abs("dev")).unwrap(),
        set(&[u[0]])
    );
    assert_eq!(
        f.ws_db
            .by_category(Device, Some(u[0]), Some(1))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn test_get_by_category_invalid_parent() {
    let mut f = Fixture::new();
    let (u1, u2, u3) = (f.uuid(1), f.uuid(2), f.uuid(3));
    let w = f.writer();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat"),
        u1,
        &v("0.1"),
        false,
        Some(u2),
    )
    .unwrap();
    let cmp = w
        .add_element(Component, 0, &f.abs("fp"), u3, &v("0.1"), false, "")
        .unwrap();
    w.add_to_category(Component, cmp, u1).unwrap();

    // The category "u1" does not have a valid parent, but it will still be
    // listed in category trees as a root category. So its contained elements
    // shall be listed as usual, not in the "without category" node.
    assert_eq!(
        f.ws_db.by_category(Component, Some(u1), None).unwrap(),
        set(&[u3])
    );
    assert!(
        f.ws_db
            .by_category(Component, None, None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn test_get_by_category_endless_recursion() {
    let mut f = Fixture::new();
    let (u1, u2, u3) = (f.uuid(1), f.uuid(2), f.uuid(3));
    let w = f.writer();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat1"),
        u1,
        &v("0.1"),
        false,
        Some(u2),
    )
    .unwrap();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat2"),
        u2,
        &v("0.1"),
        false,
        Some(u1),
    )
    .unwrap();
    let cmp = w
        .add_element(Component, 0, &f.abs("fp"), u3, &v("0.1"), false, "")
        .unwrap();
    w.add_to_category(Component, cmp, u1).unwrap();

    assert_eq!(
        f.ws_db.by_category(Component, Some(u1), None).unwrap(),
        set(&[u3])
    );
    assert!(
        f.ws_db
            .by_category(Component, None, None)
            .unwrap()
            .is_empty()
    );
    // Not ported from upstream: the category tree must not recurse endlessly
    // (the categories are not reachable from a root).
    assert!(
        f.ws_db
            .category_tree(ComponentCategory, NO_LOCALES)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn test_get_by_category_duplicates() {
    let mut f = Fixture::new();
    let (u1, u2, u3) = (f.uuid(1), f.uuid(2), f.uuid(3));
    let w = f.writer();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat1"),
        u2,
        &v("0.1"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        ComponentCategory,
        0,
        &f.abs("cmpcat2"),
        u1,
        &v("0.1"),
        false,
        Some(u2),
    )
    .unwrap();
    w.add_category(
        ComponentCategory,
        1,
        &f.abs("cmpcat3"),
        u2,
        &v("0.1"),
        false,
        None,
    )
    .unwrap();
    w.add_category(
        ComponentCategory,
        1,
        &f.abs("cmpcat4"),
        u1,
        &v("0.1"),
        false,
        Some(u2),
    )
    .unwrap();
    let cmp1 = w
        .add_element(Component, 0, &f.abs("cmp1"), u3, &v("0.1"), false, "")
        .unwrap();
    w.add_to_category(Component, cmp1, u1).unwrap();
    let cmp2 = w
        .add_element(Component, 0, &f.abs("cmp2"), u3, &v("0.1"), false, "")
        .unwrap();
    w.add_to_category(Component, cmp2, u1).unwrap();

    assert_eq!(
        f.ws_db.by_category(Component, Some(u1), None).unwrap(),
        set(&[u3])
    );
    assert!(
        f.ws_db
            .by_category(Component, None, None)
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// Tests for generated()
// ---------------------------------------------------------------------------

#[test]
fn test_get_generated_empty_db() {
    let f = Fixture::new();
    assert!(f.ws_db.generated(Symbol, "").unwrap().is_empty());
    assert!(f.ws_db.generated(Package, "").unwrap().is_empty());
    assert!(f.ws_db.generated(Component, "foo").unwrap().is_empty());
    assert!(f.ws_db.generated(Device, "bar").unwrap().is_empty());
}

#[test]
fn test_get_generated() {
    let mut f = Fixture::new();
    let u: Vec<Uuid> = (1..=8).map(|i| f.uuid(i)).collect();
    let w = f.writer();
    w.add_element(Symbol, 0, &f.abs("sym1"), u[0], &v("0.1"), false, "")
        .unwrap();
    w.add_element(Symbol, 0, &f.abs("sym2"), u[1], &v("0.1"), false, "gen:1")
        .unwrap();
    w.add_element(Package, 0, &f.abs("pkg1"), u[2], &v("0.1"), false, "")
        .unwrap();
    w.add_element(Package, 0, &f.abs("pkg2"), u[3], &v("0.1"), false, "gen:2")
        .unwrap();
    w.add_element(Component, 0, &f.abs("cmp1"), u[4], &v("0.1"), false, "")
        .unwrap();
    w.add_element(
        Component,
        0,
        &f.abs("cmp2"),
        u[5],
        &v("0.1"),
        false,
        "gen:3",
    )
    .unwrap();
    w.add_device(0, &f.abs("dev1"), u[6], &v("0.1"), false, "", rnd(), rnd())
        .unwrap();
    w.add_device(
        0,
        &f.abs("dev2"),
        u[7],
        &v("0.1"),
        false,
        "gen:1",
        rnd(),
        rnd(),
    )
    .unwrap();

    assert!(f.ws_db.generated(Symbol, "").unwrap().is_empty());
    assert_eq!(f.ws_db.generated(Symbol, "gen:1").unwrap(), set(&[u[1]]));
    assert!(f.ws_db.generated(Package, "").unwrap().is_empty());
    assert_eq!(f.ws_db.generated(Package, "gen:2").unwrap(), set(&[u[3]]));
    assert!(f.ws_db.generated(Component, "gen:2").unwrap().is_empty());
    assert_eq!(f.ws_db.generated(Component, "gen:3").unwrap(), set(&[u[5]]));
    assert!(f.ws_db.generated(Device, "gen:2").unwrap().is_empty());
    assert_eq!(f.ws_db.generated(Device, "gen:1").unwrap(), set(&[u[7]]));
}

// ---------------------------------------------------------------------------
// Tests for component_devices()
// ---------------------------------------------------------------------------

#[test]
fn test_get_component_devices_empty_db() {
    let f = Fixture::new();
    assert!(f.ws_db.component_devices(rnd()).unwrap().is_empty());
}

#[test]
fn test_get_component_devices() {
    let mut f = Fixture::new();
    let (u0, u1, u2, u3) = (f.uuid(0), f.uuid(1), f.uuid(2), f.uuid(3));
    let w = f.writer();
    w.add_device(0, &f.abs("dev1"), u1, &v("0.1"), false, "", u0, rnd())
        .unwrap();
    w.add_device(0, &f.abs("dev2"), u2, &v("0.1"), false, "", u0, rnd())
        .unwrap();
    w.add_device(0, &f.abs("dev3"), u3, &v("0.1"), false, "", rnd(), rnd())
        .unwrap();
    assert_eq!(f.ws_db.component_devices(u0).unwrap(), set(&[u1, u2]));
}

#[test]
fn test_get_component_devices_duplicates() {
    let mut f = Fixture::new();
    let (u0, u1, u2) = (f.uuid(0), f.uuid(1), f.uuid(2));
    let w = f.writer();
    w.add_device(0, &f.abs("dev1"), u1, &v("0.1"), false, "", u0, rnd())
        .unwrap();
    w.add_device(1, &f.abs("dev2"), u1, &v("0.1"), false, "", u0, rnd())
        .unwrap();
    w.add_device(1, &f.abs("dev3"), u2, &v("0.1"), false, "", rnd(), rnd())
        .unwrap();
    assert_eq!(f.ws_db.component_devices(u0).unwrap(), set(&[u1]));
}

// ---------------------------------------------------------------------------
// Tests not ported from upstream
// ---------------------------------------------------------------------------

#[test]
fn test_reopen_keeps_content_and_reset_on_version_mismatch() {
    let tmp = TempDir::new();
    let fp = tmp.path().path_to("fp");
    {
        let db = LibraryDb::open(tmp.path()).unwrap();
        let conn = SqliteDatabase::open(db.file_path().as_path()).unwrap();
        LibraryDbWriter::new(tmp.path(), &conn)
            .add_library(&fp, rnd(), &v("1"), false, &[], "")
            .unwrap();
    }
    {
        let db = LibraryDb::open(tmp.path()).unwrap();
        assert_eq!(db.all(Library, None, None).unwrap().len(), 1);
        let conn = SqliteDatabase::open(db.file_path().as_path()).unwrap();
        conn.exec("UPDATE internal SET value_int = 7 WHERE key = 'version'")
            .unwrap();
    }
    let db = LibraryDb::open(tmp.path()).unwrap();
    assert!(db.all(Library, None, None).unwrap().is_empty());
}

#[test]
fn test_parts() {
    use librepcb_core::attribute::{Attribute, AttributeKey, AttributeType};

    let mut f = Fixture::new();
    let d = f.uuid(1);
    let w = f.writer();
    let dev = w
        .add_device(0, &f.abs("dev"), d, &v("0.1"), false, "", rnd(), rnd())
        .unwrap();
    let p1 = w.add_part(dev, "MPN-10", "ACME").unwrap();
    let r = AttributeType::Resistance;
    let attr = |value: &str| {
        Attribute::new(
            AttributeKey::new("RESISTANCE").unwrap(),
            r,
            value,
            r.unit_from_string("kiloohm").unwrap(),
        )
        .unwrap()
    };
    w.add_part_attribute(p1, &attr("10")).unwrap();
    let p2 = w.add_part(dev, "MPN-2", "ACME").unwrap();
    w.add_part_attribute(p2, &attr("2")).unwrap();
    w.add_part(dev, "", "Other").unwrap();
    w.add_part(dev, "MPN-2", "ACME").unwrap(); // Duplicate without attribute.
    let p5 = w.add_part(dev, "MPN-2", "ACME").unwrap(); // Real duplicate.
    w.add_part_attribute(p5, &attr("2")).unwrap();

    let parts = f.ws_db.device_parts(d).unwrap();
    let summary: Vec<(String, String, usize)> = parts
        .iter()
        .map(|p| {
            (
                p.mpn().to_string(),
                p.manufacturer().to_string(),
                p.attributes().len(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (String::new(), "Other".to_owned(), 0),
            ("MPN-10".to_owned(), "ACME".to_owned(), 1),
            ("MPN-2".to_owned(), "ACME".to_owned(), 0),
            ("MPN-2".to_owned(), "ACME".to_owned(), 1),
        ]
    );
    assert_eq!(parts[1].attributes().get(0).unwrap().value(), "10");

    assert_eq!(f.ws_db.find_devices_of_parts("mpn-1").unwrap(), vec![d]);
    assert_eq!(f.ws_db.find_devices_of_parts("acme").unwrap(), vec![d]);
    assert!(f.ws_db.find_devices_of_parts("foo").unwrap().is_empty());
    assert_eq!(f.ws_db.find_parts_of_device(d, "mpn-1").unwrap().len(), 1);
    assert_eq!(f.ws_db.find_parts_of_device(d, "other").unwrap().len(), 1);
    assert!(f.ws_db.find_parts_of_device(rnd(), "").unwrap().is_empty());
}
