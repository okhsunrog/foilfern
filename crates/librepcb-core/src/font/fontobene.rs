//! Port of the FontoBene parser libs/fontobene-qt (version 1.0.0, Apache-2.0,
//! header-only C++/Qt library).
//!
//! FontoBene (<https://github.com/fontobene/fontobene>) is a stroke font
//! format. Hand-written because no maintained Rust parser exists: the
//! reference `fontobene-rs` is unpublished and only validates the grammar
//! (with the pre-1.0 `pest`). This port reproduces the (lenient) behavior of
//! fontobene-qt, which LibrePCB uses: e.g. unknown glyph header content is
//! ignored, glyphs are parsed until an empty line, and glyph references
//! may only point to glyphs defined earlier in the file.
//!
//! Differences to upstream (see also COMPAT.md):
//! - The `GlyphListCache` (lookup with replacements) is built eagerly and is
//!   immutable, so fonts can be shared between threads.
//! - Numbers and codepoints are parsed with Rust's `str::parse()` /
//!   `u16::from_str_radix()` and lines trimmed with `str::trim()` instead of
//!   emulating `QString`.
//! - Codepoints are limited to the Basic Multilingual Plane (`u16`), like
//!   upstream (a glyph outside of it makes the whole font invalid).

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::Regex;

/// Error of the FontoBene parser (upstream `fontobene::Exception`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Malformed file content.
    #[error("{0}")]
    Parse(String),
    /// A glyph (and its replacements) does not exist.
    #[error("Glyph {} not found.", codepoint_to_string(*.0))]
    GlyphNotFound(u32),
    /// A glyph references a glyph defined later in the file.
    #[error("Forward reference detected in {}.", codepoint_to_string(u32::from(*.0)))]
    ForwardReference(u16),
}

/// Formats a codepoint like `U+00b5`.
fn codepoint_to_string(codepoint: u32) -> String {
    format!("U+{codepoint:04x}")
}

/// The file header.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// Font identifier.
    pub id: String,
    /// Font name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Font version.
    pub version: String,
    /// Authors.
    pub authors: Vec<String>,
    /// Licenses.
    pub licenses: Vec<String>,
    /// Recommended letter spacing (in font units, 9 = cap height).
    pub letter_spacing: f64,
    /// Recommended line spacing (in font units, 9 = cap height).
    pub line_spacing: f64,
    /// Custom key/value pairs of the `[user]` section.
    pub user_data: BTreeMap<String, String>,
}

impl Default for Header {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            version: String::new(),
            authors: Vec::new(),
            licenses: Vec::new(),
            letter_spacing: 0.0,
            line_spacing: 9.0,
            user_data: BTreeMap::new(),
        }
    }
}

impl Header {
    /// Parses the header, consuming the lines up to the `---` separator.
    fn load<'a>(lines: &mut impl Iterator<Item = &'a str>) -> Result<Self, Error> {
        #[derive(PartialEq, Eq)]
        enum Section {
            None,
            Format,
            Font,
            User,
        }
        let mut header = Self::default();
        let mut section = Section::None;
        let (mut format, mut format_version) = (String::new(), String::new());
        for line in lines.by_ref() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = match line.split_once('=') {
                Some((key, value)) => (key.trim(), value.trim()),
                None => ("", ""),
            };
            let unexpected = || Error::Parse(format!("Unexpected content: \"{line}\""));
            match section {
                Section::None => {
                    if line == "[format]" {
                        section = Section::Format;
                    } else {
                        return Err(unexpected());
                    }
                }
                Section::Format => {
                    if line == "[font]" {
                        if format != "FontoBene" {
                            return Err(Error::Parse(format!("Unknown format: \"{format}\"")));
                        }
                        if !format_version.starts_with("1.") {
                            return Err(Error::Parse(format!(
                                "Unsupported format version: \"{format_version}\""
                            )));
                        }
                        section = Section::Font;
                    } else if key == "format" {
                        format = value.to_owned();
                    } else if key == "format_version" {
                        format_version = value.to_owned();
                    } else {
                        return Err(unexpected());
                    }
                }
                Section::Font => match (line, key) {
                    ("---", _) => break,
                    ("[user]", _) => section = Section::User,
                    (_, "id") => header.id = value.to_owned(),
                    (_, "name") => header.name = value.to_owned(),
                    (_, "description") => header.description = value.to_owned(),
                    (_, "version") => header.version = value.to_owned(),
                    (_, "author") => header.authors.push(value.to_owned()),
                    (_, "license") => header.licenses.push(value.to_owned()),
                    (_, "letter_spacing") => header.letter_spacing = parse_number(value)?,
                    (_, "line_spacing") => header.line_spacing = parse_number(value)?,
                    _ => return Err(unexpected()),
                },
                Section::User => {
                    if line == "---" {
                        break;
                    } else if !key.is_empty() {
                        header.user_data.insert(key.to_owned(), value.to_owned());
                    } else {
                        return Err(unexpected());
                    }
                }
            }
        }
        Ok(header)
    }
}

/// A vertex of a glyph polyline, in font units (9 = cap height).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vertex {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Arc angle to the next vertex (in units of 20°, i.e. 9 = 180°).
    pub bulge: f64,
}

impl Vertex {
    /// Returns X scaled to the given cap height.
    pub fn scaled_x(&self, full_scale: f64) -> f64 {
        self.x * full_scale / 9.0
    }

    /// Returns Y scaled to the given cap height.
    pub fn scaled_y(&self, full_scale: f64) -> f64 {
        self.y * full_scale / 9.0
    }

    /// Returns the bulge scaled so that `9` corresponds to
    /// `full_scale_180deg`.
    pub fn scaled_bulge(&self, full_scale_180deg: f64) -> f64 {
        self.bulge * full_scale_180deg / 9.0
    }

    fn parse(s: &str) -> Result<Self, Error> {
        let numbers: Vec<&str> = s.split(',').collect();
        if !(2..=3).contains(&numbers.len()) {
            return Err(Error::Parse(format!("Invalid vertex: \"{s}\"")));
        }
        Ok(Self {
            x: parse_number(numbers[0])?,
            y: parse_number(numbers[1])?,
            bulge: numbers.get(2).map_or(Ok(0.0), |b| parse_number(b))?,
        })
    }
}

/// A polyline of a glyph.
pub type Polyline = Vec<Vertex>;

fn parse_polyline(s: &str) -> Result<Polyline, Error> {
    s.split(';').map(Vertex::parse).collect()
}

/// A glyph definition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Glyph {
    /// Codepoint (0 = invalid).
    pub codepoint: u16,
    /// Referenced glyphs whose polylines are part of this glyph.
    pub references: Vec<u16>,
    /// Polylines.
    pub polylines: Vec<Polyline>,
    /// Glyph specific spacing (e.g. for whitespace glyphs).
    pub spacing: Option<f64>,
}

impl Glyph {
    /// Returns whether the glyph is valid (codepoint > 0).
    pub fn is_valid(&self) -> bool {
        self.codepoint > 0
    }

    /// Parses the next glyph (up to the next empty line). Returns an invalid
    /// glyph if there is none.
    fn load<'a>(lines: &mut impl Iterator<Item = &'a str>) -> Result<Self, Error> {
        let mut glyph = Self::default();
        let mut in_body = false;
        for line in lines.by_ref() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if !in_body {
                if !line.is_empty() {
                    glyph.codepoint = parse_codepoint(find_glyph_codepoint(line))?;
                    in_body = true;
                }
            } else if let Some(reference) = line.strip_prefix('@') {
                glyph.references.push(parse_codepoint(reference)?);
            } else if let Some(spacing) = line.strip_prefix('~') {
                // Upstream stores the glyph spacing as `float`, which changes
                // the stroked text geometry (e.g. 3.6 becomes 3.5999999), so
                // round it to single precision like upstream.
                glyph.spacing = Some(f64::from(parse_number(spacing)? as f32));
            } else if !line.is_empty() {
                glyph.polylines.push(parse_polyline(line)?);
            } else {
                break;
            }
        }
        Ok(glyph)
    }
}

/// Returns the codepoint of a glyph header line like `[0041] A` (first match
/// of the upstream regex, or an empty string).
fn find_glyph_codepoint(line: &str) -> &str {
    static CODEPOINT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([0-9a-fA-F]{4,6})\]").expect("valid regex literal"));
    CODEPOINT
        .captures(line)
        .and_then(|c| c.get(1))
        .map_or("", |m| m.as_str())
}

/// Parses a hexadecimal codepoint of the Basic Multilingual Plane.
fn parse_codepoint(s: &str) -> Result<u16, Error> {
    u16::from_str_radix(s.trim(), 16)
        .map_err(|_| Error::Parse(format!("Invalid codepoint: \"{s}\"")))
}

/// Parses a (finite) number.
fn parse_number(s: &str) -> Result<f64, Error> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::Parse(format!("Invalid number: \"{s}\"")))
}

/// A parsed FontoBene font.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Font {
    /// The header.
    pub header: Header,
    /// All valid glyphs in file order.
    pub glyphs: Vec<Glyph>,
}

impl Font {
    /// Parses a font from the file content.
    pub fn parse(content: &str) -> Result<Self, Error> {
        let content = content.strip_prefix('\u{feff}').unwrap_or(content);
        let mut lines = content.lines();
        let header = Header::load(&mut lines)?;
        let mut glyphs = Vec::with_capacity(200);
        let mut lines = lines.peekable();
        while lines.peek().is_some() {
            let glyph = Glyph::load(&mut lines)?;
            if glyph.is_valid() {
                glyphs.push(glyph);
            }
        }
        Ok(Self { header, glyphs })
    }
}

/// Glyph lookup with replacement glyphs (upstream `GlyphListCache` and
/// `GlyphListAccessor`).
#[derive(Debug, Clone, Default)]
pub struct GlyphListAccessor {
    /// Index of the first glyph of each codepoint.
    indices: HashMap<u16, usize>,
    /// Replacement for non-existent glyphs (0 = none).
    replacement_glyph: u16,
    /// Alternative glyphs to try per codepoint.
    replacements: HashMap<u16, Vec<u16>>,
}

impl GlyphListAccessor {
    /// Creates the accessor for the glyphs of `font`.
    pub fn new(font: &Font) -> Self {
        let mut indices = HashMap::new();
        for (i, glyph) in font.glyphs.iter().enumerate() {
            indices.entry(glyph.codepoint).or_insert(i);
        }
        Self {
            indices,
            ..Self::default()
        }
    }

    /// Sets the replacement for non-existent glyphs.
    pub fn set_replacement_glyph(&mut self, codepoint: u16) {
        self.replacement_glyph = codepoint;
    }

    /// Makes all given codepoints replacements of each other.
    pub fn add_replacements(&mut self, codepoints: &[u16]) {
        for codepoint in codepoints {
            self.replacements
                .entry(*codepoint)
                .or_default()
                .extend_from_slice(codepoints);
        }
    }

    /// Returns the index of the glyph to use for `codepoint` (the glyph
    /// itself, one of its replacements, or the replacement glyph).
    fn glyph_index(&self, codepoint: u32) -> Result<usize, Error> {
        // Font glyphs are limited to the Basic Multilingual Plane.
        let bmp = u16::try_from(codepoint).ok();
        bmp.into_iter()
            .chain(
                bmp.and_then(|cp| self.replacements.get(&cp))
                    .into_iter()
                    .flatten()
                    .copied(),
            )
            .chain(std::iter::once(self.replacement_glyph))
            .find_map(|cp| self.indices.get(&cp).copied())
            .ok_or(Error::GlyphNotFound(codepoint))
    }

    /// Returns all polylines of the glyph of a character (including
    /// referenced glyphs) and its spacing (0 if not specified).
    pub fn all_polylines_of_glyph(
        &self,
        font: &Font,
        character: char,
    ) -> Result<(Vec<Polyline>, f64), Error> {
        let mut spacing = 0.0;
        let index = self.glyph_index(u32::from(character))?;
        let polylines = self.polylines_impl(font, index, &mut spacing)?;
        Ok((polylines, spacing))
    }

    fn polylines_impl(
        &self,
        font: &Font,
        index: usize,
        spacing: &mut f64,
    ) -> Result<Vec<Polyline>, Error> {
        let glyph = &font.glyphs[index];
        let mut polylines = glyph.polylines.clone();
        for reference in &glyph.references {
            let ref_index = self.glyph_index(u32::from(*reference))?;
            if ref_index < index {
                polylines.extend(self.polylines_impl(font, ref_index, spacing)?);
            } else {
                // Forward references are forbidden (to avoid endless loops)!
                return Err(Error::ForwardReference(glyph.codepoint));
            }
        }
        if let Some(value) = glyph.spacing {
            *spacing = value;
        }
        Ok(polylines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codepoint_regex() {
        assert_eq!(find_glyph_codepoint("[0041] A"), "0041");
        assert_eq!(find_glyph_codepoint("x [00c2]"), "00c2");
        assert_eq!(find_glyph_codepoint("[041]"), "");
        assert_eq!(find_glyph_codepoint("[0000041]"), "");
        assert_eq!(find_glyph_codepoint("[zz][10FFFF]"), "10FFFF");
        assert!(parse_codepoint("10FFFF").is_err());
        assert_eq!(parse_codepoint("00B5"), Ok(0xB5));
    }

    #[test]
    fn header_errors() {
        assert!(Font::parse("foo").is_err());
        assert!(Font::parse("[format]\nformat = Foo\n[font]\n").is_err());
        assert!(
            Font::parse("[format]\nformat = FontoBene\nformat_version = 2.0\n[font]\n").is_err()
        );
        let font = Font::parse("").unwrap();
        assert_eq!(font.header.line_spacing, 9.0);
    }
}
