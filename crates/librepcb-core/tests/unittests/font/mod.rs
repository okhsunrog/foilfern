//! Tests of the font module: the FontoBene parser (port of the test program
//! of libs/fontobene-qt/tests, with assertions instead of debug output) and
//! the stroke font (upstream has no unit tests for it).

use std::path::PathBuf;

use librepcb_core::font::fontobene::{Font, Vertex};
use librepcb_core::font::{StrokeFont, StrokeFontPool, StrokeTextPathBuilder};
use librepcb_core::types::{
    Alignment, Angle, HAlign, Length, Point, PositiveLength, Ratio, StrokeTextSpacing,
    UnsignedLength, VAlign,
};

/// Upstream checkout (see `LIBREPCB_UPSTREAM_DIR` in `.cargo/config.toml`).
fn upstream_dir() -> PathBuf {
    PathBuf::from(env!("LIBREPCB_UPSTREAM_DIR"))
}

fn newstroke() -> Vec<u8> {
    std::fs::read(
        upstream_dir().join("tests/data/projects/Empty Project/resources/fontobene/newstroke.bene"),
    )
    .unwrap()
}

fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

#[test]
fn test_fontobene_test_file() {
    let content =
        std::fs::read_to_string(upstream_dir().join("libs/fontobene-qt/tests/fonto.bene")).unwrap();
    let font = Font::parse(&content).unwrap();
    assert_eq!(font.header.id, "test");
    assert_eq!(font.header.name, "Test Font");
    assert_eq!(font.header.version, "1.0");
    assert_eq!(font.header.authors, ["Max Müller <max@example.com>"]);
    assert_eq!(font.header.licenses, ["GPL-3.0+"]);
    assert_eq!(
        font.header.user_data.get("comment").map(String::as_str),
        Some("This is a test font.")
    );
    assert_eq!(font.glyphs.len(), 3);
    let a = &font.glyphs[0];
    assert_eq!(a.codepoint, 0x41);
    assert!(a.references.is_empty());
    assert_eq!(a.polylines.len(), 2);
    assert_eq!(
        a.polylines[1],
        [
            Vertex {
                x: 0.0,
                y: 0.0,
                bulge: 0.0
            },
            Vertex {
                x: 3.0,
                y: 9.0,
                bulge: 0.0
            },
            Vertex {
                x: 6.0,
                y: 0.0,
                bulge: 0.0
            },
        ]
    );
    let f = &font.glyphs[1];
    assert_eq!(f.codepoint, 0x66);
    assert_eq!(f.polylines[0][2].bulge, -4.5);
    let a_circumflex = &font.glyphs[2];
    assert_eq!(a_circumflex.codepoint, 0xC2);
    assert_eq!(a_circumflex.references, [0x41]);
    assert_eq!(a_circumflex.polylines.len(), 1);
}

#[test]
fn test_newstroke() {
    let font = StrokeFont::new(newstroke());
    assert!(font.load_error().is_none());
    assert!(font.font().glyphs.len() > 2000);
    assert_eq!(font.letter_spacing(), Ratio::from_normalized(1.8 / 9.0));
    assert_eq!(font.line_spacing(), Ratio::from_normalized(16.0 / 9.0));

    // Glyph "!": polylines scaled to the height, bulges converted to angles.
    let (paths, spacing) = font.stroke_glyph('!', pos(9_000_000));
    assert_eq!(spacing, Length::ZERO);
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0].vertices()[0].pos, Point::from_nm(430_000, 860_000));

    // Space: only spacing.
    let (paths, spacing) = font.stroke_glyph(' ', pos(9_000_000));
    assert!(paths.is_empty());
    // The glyph spacing 3.6 is stored as `float` (3.5999999), like upstream.
    assert_eq!(spacing, Length::new(3_599_999));

    // References are resolved (Â references the glyph parts of A).
    let (a, _) = font.stroke_glyph('A', pos(1_000_000));
    let (a_circumflex, _) = font.stroke_glyph('Â', pos(1_000_000));
    assert!(a_circumflex.len() > a.len());
    assert!(a.iter().all(|p| a_circumflex.contains(p)));
    // Unknown glyphs are replaced by U+FFFD.
    let (unknown, _) = font.stroke_glyph('\u{E000}', pos(1_000_000));
    assert_eq!(unknown, font.stroke_glyph('\u{FFFD}', pos(1_000_000)).0);
    assert!(!unknown.is_empty());
    // Micro sign falls back to Greek mu (or the other way around).
    assert!(!font.stroke_glyph('µ', pos(1_000_000)).0.is_empty());
}

#[test]
fn test_stroke_alignment() {
    let font = StrokeFont::new(newstroke());
    let height = pos(1_000_000);
    let text = "AB\nC";
    let spacing = Length::new(100_000);
    let line_spacing = Length::new(1_500_000);
    let left_bottom = font.stroke(
        text,
        height,
        spacing,
        line_spacing,
        Alignment::new(HAlign::Left, VAlign::Bottom),
    );
    assert_eq!(left_bottom.bottom_left, Point::ORIGIN);
    assert_eq!(left_bottom.top_right.y, Length::new(2_500_000));
    let (lines, width) = font.stroke_lines(text, height, spacing);
    assert_eq!(lines.len(), 2);
    assert_eq!(left_bottom.top_right.x, width);
    assert!(lines[0].width > lines[1].width);
    let center = font.stroke(
        text,
        height,
        spacing,
        line_spacing,
        Alignment::new(HAlign::Center, VAlign::Center),
    );
    assert_eq!(
        center.bottom_left,
        Point::new(-width / 2, Length::new(-1_250_000))
    );
    assert_eq!(center.paths.len(), left_bottom.paths.len());

    // Auto-rotation rotates upside down texts by 180° around their center.
    let build = |rotation| {
        StrokeTextPathBuilder::build(
            &font,
            StrokeTextSpacing::Auto,
            StrokeTextSpacing::Manual(Ratio::from_percent(150)),
            height,
            UnsignedLength::ZERO,
            Alignment::new(HAlign::Center, VAlign::Center),
            rotation,
            true,
            "A",
        )
    };
    let normal = build(Angle::DEG0);
    let rotated = build(Angle::DEG180);
    assert_eq!(normal.len(), rotated.len());
    assert_ne!(normal, rotated);
}

#[test]
fn test_invalid_font_is_empty() {
    let font = StrokeFont::new(b"[format]\nformat = Foo\n[font]\n".to_vec());
    assert!(font.load_error().is_some());
    assert!(font.font().glyphs.is_empty());
    assert!(font.stroke_glyph('A', pos(1_000_000)).0.is_empty());
}

#[test]
fn test_font_pool() {
    let pool = StrokeFontPool::from_files([
        ("newstroke.bene", newstroke()),
        ("readme.txt", b"foo".to_vec()),
    ]);
    assert!(pool.exists("newstroke.bene"));
    assert!(!pool.exists("readme.txt"));
    assert!(pool.font("newstroke.bene").is_ok());
    assert!(pool.font("foo.bene").is_err());
}
