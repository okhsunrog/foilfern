//! Port of tests/unittests/core/fileio/filepathtest.cpp.

use foilfern_core::fileio::{CleanFileNameOptions, FileNameCase, FilePath};

struct Data {
    valid: bool,
    input_file_path: &'static str,
    input_base_path: &'static str,
    to_str: &'static str,
    #[cfg_attr(not(windows), allow(dead_code))]
    to_windows_style: &'static str,
    to_relative: &'static str,
    is_root: bool,
}

const fn d(
    valid: bool,
    input_file_path: &'static str,
    input_base_path: &'static str,
    to_str: &'static str,
    to_windows_style: &'static str,
    to_relative: &'static str,
    is_root: bool,
) -> Data {
    Data {
        valid,
        input_file_path,
        input_base_path,
        to_str,
        to_windows_style,
        to_relative,
        is_root,
    }
}

#[rustfmt::skip]
fn test_data() -> Vec<Data> {
    let mut v = Vec::new();
    #[cfg(windows)]
    v.extend([
        d(true , "C:\\foo\\bar"        , "C:/foo"      , "C:/foo/bar"    , "C:\\foo\\bar"    , "bar"           , false), // Win path to a dir
        d(true , "C:\\foo\\bar\\"      , "C:/bar"      , "C:/foo/bar"    , "C:\\foo\\bar"    , "../foo/bar"    , false), // Win path to a dir + backslash
        d(true , "C:\\foo\\bar.txt"    , "C:/bar"      , "C:/foo/bar.txt", "C:\\foo\\bar.txt", "../foo/bar.txt", false), // Win path to a file
        d(true , "C:\\foo\\bar"        , "C:/foo\\bar" , "C:/foo/bar"    , "C:\\foo\\bar"    , ""              , false), // Win path with path==base
        d(true , "C:\\\\foo\\..\\bar\\", "C:\\"        , "C:/bar"        , "C:\\bar"         , "bar"           , false), // Win path with .. and double backslashes
        d(true , "C:\\"                , "C:\\foo"     , "C:/"           , "C:\\"            , ".."            , true ), // Win drive root path
    ]);
    #[cfg(not(windows))]
    v.extend([
        d(true , "/foo/bar"            , "/foo"        , "/foo/bar"      , "\\foo\\bar"      , "bar"           , false), // UNIX path to a dir
        d(true , "/foo/bar/"           , "/bar"        , "/foo/bar"      , "\\foo\\bar"      , "../foo/bar"    , false), // UNIX path to a dir + slash
        d(true , "/foo/bar.txt"        , "/bar"        , "/foo/bar.txt"  , "\\foo\\bar.txt"  , "../foo/bar.txt", false), // UNIX path to a file
        d(true , "/foo/bar"            , "/foo/bar"    , "/foo/bar"      , "\\foo\\bar"      , ""              , false), // UNIX path with path==base
        d(true , "//foo/..//bar//"     , "/"           , "/bar"          , "\\bar"           , "bar"           , false), // UNIX path with .. and double slashes
        d(true , "/"                   , "/foo"        , "/"             , "\\"              , ".."            , true ), // UNIX root path
    ]);
    #[cfg(windows)]
    v.extend([
        d(false, "foo\\bar"            , ""            , ""              , ""                , ""              , false), // rel. Win path to a dir
        d(false, "foo\\bar.txt"        , ""            , ""              , ""                , ""              , false), // rel. Win path to a file
    ]);
    v.extend([
        d(false, "foo/bar"             , ""            , ""              , ""                , ""              , false), // rel. UNIX path to a dir
        d(false, "foo/bar.txt"         , ""            , ""              , ""                , ""              , false), // rel. UNIX path to a file
        d(false, ""                    , ""            , ""              , ""                , ""              , false), // empty path
    ]);
    v
}

fn to_str(p: &Option<FilePath>) -> &str {
    p.as_ref().map_or("", FilePath::as_str)
}

#[test]
fn test_default_constructor() {
    // Upstream: invalid default-constructed FilePath -> `Option::None`.
    let p: Option<FilePath> = None;
    assert_eq!(to_str(&p), "");
}

#[test]
fn test_constructor() {
    for data in test_data() {
        let p = FilePath::new(data.input_file_path);
        assert_eq!(p.is_some(), data.valid, "{}", data.input_file_path);
        assert_eq!(to_str(&p), data.to_str, "{}", data.input_file_path);
    }
}

#[test]
fn test_copy_constructor() {
    for data in test_data() {
        let p1 = FilePath::new(data.input_file_path);
        let p2 = p1.clone();
        assert_eq!(p1, p2);
        assert_eq!(to_str(&p1), to_str(&p2));
    }
}

#[test]
fn test_from_str() {
    for data in test_data() {
        let p = data.input_file_path.parse::<FilePath>();
        assert_eq!(p.is_ok(), data.valid, "{}", data.input_file_path);
        assert_eq!(to_str(&p.ok()), data.to_str);
    }
}

#[test]
fn test_to_native() {
    for data in test_data() {
        let native = FilePath::new(data.input_file_path)
            .map(|p| p.to_native())
            .unwrap_or_default();
        #[cfg(windows)]
        assert_eq!(native, data.to_windows_style);
        #[cfg(not(windows))]
        assert_eq!(native, data.to_str);
    }
}

#[test]
fn test_to_relative() {
    for data in test_data().into_iter().filter(|d| d.valid) {
        let base = FilePath::new(data.input_base_path).unwrap();
        let p = FilePath::new(data.input_file_path).unwrap();
        assert_eq!(
            p.to_relative(&base),
            data.to_relative,
            "{}",
            data.input_file_path
        );
    }
}

#[test]
fn test_to_relative_native() {
    let sep = std::path::MAIN_SEPARATOR_STR;
    for data in test_data().into_iter().filter(|d| d.valid) {
        let base = FilePath::new(data.input_base_path).unwrap();
        let p = FilePath::new(data.input_file_path).unwrap();
        assert_eq!(
            p.to_relative_native(&base),
            data.to_relative.replace('/', sep)
        );
    }
}

#[test]
fn test_from_relative() {
    for data in test_data().into_iter().filter(|d| d.valid) {
        let base = FilePath::new(data.input_base_path).unwrap();
        let p = FilePath::from_relative(&base, data.to_relative);
        assert_eq!(p.as_str(), data.to_str);
    }
}

#[test]
fn test_is_root() {
    for data in test_data() {
        let p = FilePath::new(data.input_file_path);
        assert_eq!(p.is_some_and(|p| p.is_root()), data.is_root);
    }
}

#[test]
fn test_operator_assign() {
    for data in test_data() {
        let p1 = FilePath::new(data.input_file_path);
        let mut p2 = FilePath::new("/valid/path");
        assert!(p2.is_some());
        p2 = p1.clone();
        assert_eq!(p1, p2);
    }
}

#[test]
fn test_clean_file_name() {
    let input = " ∑ ;.'[a]*(/∮E⋅→∞∏g¼∀x∈ ℝ:T@st⌈x⌉α∧¬β=∨)⊆\nℕ ₀H₂Ω⌀,\
                 -=[];\\^με½τρ1ÖÄ23ά ειวชΚμ\tεチハ\r\n\r_+{}|\"?>< ~  ";
    let clean = |replace_spaces, case| {
        FilePath::clean_file_name(input, CleanFileNameOptions::new(replace_spaces, case), 120)
    };
    assert_eq!(
        clean(false, FileNameCase::Keep),
        ".aEg14x RTstxN 0H2-121OA23 _"
    );
    assert_eq!(
        clean(false, FileNameCase::Lower),
        ".aeg14x rtstxn 0h2-121oa23 _"
    );
    assert_eq!(
        clean(false, FileNameCase::Upper),
        ".AEG14X RTSTXN 0H2-121OA23 _"
    );
    assert_eq!(
        clean(true, FileNameCase::Keep),
        ".aEg14x_RTstxN_0H2-121OA23__"
    );
    assert_eq!(
        clean(true, FileNameCase::Lower),
        ".aeg14x_rtstxn_0h2-121oa23__"
    );
    assert_eq!(
        clean(true, FileNameCase::Upper),
        ".AEG14X_RTSTXN_0H2-121OA23__"
    );
}
