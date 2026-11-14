//! Port of tests/unittests/core/serialization/sexpressiontest.cpp.

use std::path::Path;
use std::time::Instant;

use foilfern_core::serialization::{Error, List, Mode, SExpression};

fn parse(input: &str) -> Result<SExpression, Error> {
    SExpression::parse(input.as_bytes(), None, Mode::LibrePcb)
}

fn to_string(s: &SExpression) -> String {
    s.to_string_with_mode(Mode::LibrePcb).unwrap()
}

#[test]
fn test_parse_empty_bytearray() {
    assert!(parse("").is_err());
}

#[test]
fn test_parse_empty_braces() {
    assert!(parse("()").is_err());
}

#[test]
fn test_parse_missing_closing_brace() {
    assert!(parse("(test").is_err());
}

#[test]
fn test_parse_too_few_closing_braces() {
    assert!(parse("(test (foo bar)").is_err());
}

#[test]
fn test_parse_too_many_closing_braces() {
    assert!(parse("(test (foo bar)))").is_err());
}

#[test]
fn test_parse_empty_list() {
    assert!(parse("(test)").unwrap().is_list());
}

#[test]
fn test_parse_string_with_missing_end_quote() {
    assert!(parse("(test \"foo)").is_err());
}

#[test]
fn test_parse_string() {
    let s = parse("(test \"foo bar\")").unwrap();
    assert!(s.is_list());
    assert_eq!(s.child_count(), 1);
    assert_eq!(s.child("@0").unwrap().value().unwrap(), "foo bar");
}

#[test]
fn test_parse_string_with_quotes() {
    let s = parse("(test \"foo \\\"bar\\\"\")").unwrap();
    assert!(s.is_list());
    assert_eq!(s.child_count(), 1);
    assert_eq!(s.child("@0").unwrap().value().unwrap(), "foo \"bar\"");
}

#[test]
fn test_parse_string_with_newlines() {
    let s = parse("(test \"foo\\nbar\")").unwrap();
    assert!(s.is_list());
    assert_eq!(s.child_count(), 1);
    assert_eq!(s.child("@0").unwrap().value().unwrap(), "foo\nbar");
}

#[test]
fn test_parse_string_with_backslash() {
    let s = parse("(test \"foo\\\\bar\")").unwrap();
    assert!(s.is_list());
    assert_eq!(s.child_count(), 1);
    assert_eq!(s.child("@0").unwrap().value().unwrap(), "foo\\bar");
}

#[test]
fn test_parse_expression_with_children_and_comments() {
    let input = "; (This whole line is a comment with CRLF line ending)\r\n\
                 (librepcb_board 71762d7e-e7f1-403c-8020-db9670c01e9b\n \
                 (default_font \"newstroke.bene\")\n \
                 (grid (type lines) (interval 0.15875) (unit millimeters))\n \
                 (fabrication_output_settings ; \"Just a comment\"\n  \
                 (base_path \"./output/{{VERSION}}/gerber/{{PROJECT}}\")\n  \
                 (outlines (suffix \"\"))\n  \
                 (silkscreen_top (suffix \".gto\")\n   \
                 (layers top_legend top_names)\n  \
                 )\n \
                 )\n\
                 )\n";
    let s = parse(input).unwrap();
    let value = |path: &str| s.child(path).unwrap().value().unwrap().to_owned();
    assert_eq!(value("default_font/@0"), "newstroke.bene");
    assert_eq!(value("grid/interval/@0"), "0.15875");
    assert_eq!(
        value("fabrication_output_settings/base_path/@0"),
        "./output/{{VERSION}}/gerber/{{PROJECT}}"
    );
    assert_eq!(value("fabrication_output_settings/outlines/suffix/@0"), "");
    assert_eq!(
        value("fabrication_output_settings/silkscreen_top/suffix/@0"),
        ".gto"
    );
}

#[test]
fn test_parse_partial_expression() {
    let input = "(librepcb_board 71762d7e-e7f1-403c-8020-db9670c01e9b\n \
                 (default_font \"newstroke.bene\")\n \
                 (grid (type lines) (interval 0.15875) (unit millimeters))\n \
                 (fabrication_output_settings ; \"Just a comment\"\n  \
                 (base_path \"./output/{{VERSION}}/gerber/{{PROJECT}}\")\n  \
                 (outlines (suffix \"\"))\n  \
                 (silkscreen_top (suffix \".gto\")\n   \
                 (layers top_legend top_names)\n  \
                 )\n \
                 )\n\
                 )"; // final newline omitted
    // Check if parsing fails at *any* character boundary of the input string.
    let bytes = input.as_bytes();
    for i in 0..bytes.len() {
        let left = &bytes[..i];
        let right = &bytes[bytes.len() - i..];
        assert!(
            SExpression::parse(left, None, Mode::LibrePcb).is_err(),
            "left({i})"
        );
        assert!(
            SExpression::parse(right, None, Mode::LibrePcb).is_err(),
            "right({i})"
        );
    }
}

#[test]
fn test_serialize_string_with_escaping() {
    let s = SExpression::string("Foo\n \r\n \" \\ Bar");
    assert_eq!(to_string(&s), "\"Foo\\n \\r\\n \\\" \\\\ Bar\"\n");
}

#[test]
fn test_roundtrip() {
    // Create input with wrong indentation, this shall be fixed by
    // to_string_with_mode().
    let input = "(librepcb_board 71762d7e-e7f1-403c-8020-db9670c01e9b\n\
                 (default_font \"newstroke.bene\")\n\
                 (grid (type lines) (interval 0.15875) (unit millimeters))\n\
                 (fabrication_output_settings\n\
                 (base_path \"./output/{{VERSION}}/gerber/{{PROJECT}}\")\n\
                 (outlines (suffix \"\"))\n  \
                 (silkscreen_top (suffix \".gto\")\n    \
                 (layers top_legend top_names)\n  \
                 )\n\
                 )\n \
                 (emptylines foo\n \n      (child 1)\n \n \n  )\n\
                 (multiline foo\n\
                 )\n\
                 (emptyline\n\
                 )\n\
                 (empty)\n\
                 )\n";
    let expected = "(librepcb_board 71762d7e-e7f1-403c-8020-db9670c01e9b\n \
                    (default_font \"newstroke.bene\")\n \
                    (grid (type lines) (interval 0.15875) (unit millimeters))\n \
                    (fabrication_output_settings\n  \
                    (base_path \"./output/{{VERSION}}/gerber/{{PROJECT}}\")\n  \
                    (outlines (suffix \"\"))\n  \
                    (silkscreen_top (suffix \".gto\")\n   \
                    (layers top_legend top_names)\n  \
                    )\n \
                    )\n \
                    (emptylines foo\n\n  (child 1)\n\n\n )\n \
                    (multiline foo\n \
                    )\n \
                    (emptyline\n \
                    )\n \
                    (empty)\n\
                    )\n";
    assert_eq!(to_string(&parse(input).unwrap()), expected);
}

#[test]
fn test_child_skips_line_breaks() {
    let s = parse("(root \n (child \n 0 \n 1 \n 2 \n ))").unwrap();
    assert_eq!(s.child("child/@0").unwrap().value().unwrap(), "0");
    assert_eq!(s.child("child/@1").unwrap().value().unwrap(), "1");
    assert_eq!(s.child("child/@2").unwrap().value().unwrap(), "2");
    assert!(s.child("child/@3").is_none());
    assert!(s.child("child/@-1").is_none());
    assert!(s.required_child("child/@3").is_err());
}

#[test]
fn test_remove_child() {
    let input = "(test value\n (child1 a b c)\n (child2 a b c)\n)\n";
    let mut s = parse(input).unwrap();
    let list = s.as_list_mut().unwrap();
    let index = list
        .children()
        .iter()
        .position(|c| c.as_list().is_some_and(|l| l.name() == "child1"))
        .unwrap();
    list.children_mut().remove(index);
    assert_eq!(to_string(&s), "(test value\n\n (child2 a b c)\n)\n");
}

#[test]
fn test_to_byte_array_empty_list() {
    assert_eq!(to_string(&SExpression::list("test")), "(test)\n");
}

#[test]
fn test_to_byte_array_empty_list_with_trailing_line_break() {
    let mut s = List::new("test");
    s.ensure_line_break();
    assert_eq!(to_string(&s.into()), "(test\n)\n");
}

#[test]
fn test_to_byte_array_list_with_line_breaks() {
    let mut s = List::new("test");
    s.append_child("child", &SExpression::token("1"));
    s.ensure_line_break();
    s.append_child("child", &SExpression::token("2"));
    s.ensure_line_break();
    assert_eq!(to_string(&s.into()), "(test (child 1)\n (child 2)\n)\n");
}

#[test]
fn test_to_byte_array_list_with_too_many_line_breaks() {
    let mut s = List::new("test");
    s.append_child("child", &SExpression::token("1"));
    s.ensure_line_break();
    s.ensure_line_break();
    s.ensure_line_break();
    s.append_child("child", &SExpression::token("2"));
    s.ensure_line_break();
    s.ensure_line_break();
    s.ensure_line_break();
    assert_eq!(to_string(&s.into()), "(test (child 1)\n (child 2)\n)\n");
}

#[test]
fn test_invalid_tokens_are_rejected() {
    assert!(matches!(
        SExpression::list("").to_byte_array(Mode::LibrePcb),
        Err(Error::InvalidListName(_))
    ));
    assert!(matches!(
        SExpression::token("a b").to_byte_array(Mode::LibrePcb),
        Err(Error::InvalidToken(_))
    ));
    assert!(matches!(
        SExpression::token("ä").to_byte_array(Mode::LibrePcb),
        Err(Error::InvalidToken(_))
    ));
    assert_eq!(
        SExpression::token("ä")
            .to_byte_array(Mode::Permissive)
            .unwrap(),
        "ä\n".as_bytes()
    );
}

#[test]
fn test_parse_error_message() {
    let err =
        SExpression::parse(b"(test", Some(Path::new("/tmp/foo.lp")), Mode::LibrePcb).unwrap_err();
    assert_eq!(
        err.to_string(),
        "File parse error: S-Expression node ended without closing ')'.\n\
         File: /tmp/foo.lp\nInvalid Content: ''"
    );
}

#[test]
fn test_parse_performance() {
    let path = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects/Nested Planes/boards/default/board.lp");
    let content = std::fs::read(&path).unwrap();
    let start = Instant::now();
    let n = 50; // Upstream uses 5000 loops (in release builds).
    for _ in 0..n {
        SExpression::parse(&content, Some(&path), Mode::LibrePcb).unwrap();
    }
    println!("Needed {:?} for {n} loops", start.elapsed());
}
