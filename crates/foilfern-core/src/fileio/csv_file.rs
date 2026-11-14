//! Port of libs/librepcb/core/fileio/csvfile.{h,cpp}.
//!
//! The CSV records are written with the [`csv`] crate (quoting only when
//! necessary, `"` doubled, `\n` line endings). Like upstream, values are
//! sanitized first (`\r` removed, `\n` replaced by a space), so the quoting
//! rules of both implementations are identical (values containing `,` or
//! `"` are quoted) — except that `csv` writes `""` for a record consisting
//! of a single empty value, where upstream writes an empty line.

use super::error::{Error, Result};
use super::file_path::FilePath;
use super::file_utils;

/// A comma-separated values (CSV) file with an optional comment header.
///
/// [`set_header()`](Self::set_header) must be called before adding values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CsvFile {
    comment: String,
    header: Vec<String>,
    values: Vec<Vec<String>>,
}

impl CsvFile {
    /// Creates an empty CSV file.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the raw comment (without `#`).
    pub fn comment(&self) -> &str {
        &self.comment
    }

    /// Returns the raw header items.
    pub fn header(&self) -> &[String] {
        &self.header
    }

    /// Returns all raw value rows.
    pub fn values(&self) -> &[Vec<String>] {
        &self.values
    }

    /// Sets the comment (may contain line breaks).
    pub fn set_comment(&mut self, comment: impl Into<String>) {
        self.comment = comment.into();
    }

    /// Sets the header items and clears all values (the column count may
    /// have changed).
    pub fn set_header<S: Into<String>>(&mut self, header: impl IntoIterator<Item = S>) {
        self.header = header.into_iter().map(Into::into).collect();
        self.values.clear();
    }

    /// Adds a row of values; fails if the count differs from the header.
    pub fn add_value<S: Into<String>>(&mut self, value: impl IntoIterator<Item = S>) -> Result<()> {
        let value: Vec<String> = value.into_iter().map(Into::into).collect();
        if value.len() != self.header.len() {
            return Err(Error::CsvValueCount);
        }
        self.values.push(value);
        Ok(())
    }

    /// Builds the CSV file content.
    pub fn to_csv_string(&self) -> Result<String> {
        let mut out = self.comment_lines();
        if !self.header.is_empty() {
            let mut writer = csv::WriterBuilder::new()
                .terminator(csv::Terminator::Any(b'\n'))
                .quote_style(csv::QuoteStyle::Necessary)
                .from_writer(Vec::new());
            for line in std::iter::once(&self.header).chain(&self.values) {
                // Always use the header to determine the value count.
                let record = (0..self.header.len())
                    .map(|i| escape_value(line.get(i).map_or("", String::as_str)));
                writer
                    .write_record(record)
                    .map_err(|e| Error::Csv(e.to_string()))?;
            }
            let bytes = writer.into_inner().map_err(|e| Error::Csv(e.to_string()))?;
            out.push_str(&String::from_utf8(bytes).map_err(|e| Error::Csv(e.to_string()))?);
        }
        Ok(out)
    }

    /// Writes the CSV file (UTF-8).
    pub fn save_to_file(&self, csv_fp: &FilePath) -> Result<()> {
        file_utils::write_file(csv_fp, self.to_csv_string()?.as_bytes())
    }

    fn comment_lines(&self) -> String {
        let mut s = String::new();
        if !self.comment.is_empty() {
            for line in self.comment.split('\n') {
                s.push_str(format!("# {line}").trim_end());
                s.push('\n');
            }
            s.push('\n'); // Separate comment and CSV data with an empty line.
        }
        s
    }
}

/// Removes DOS line endings and replaces line breaks by spaces.
fn escape_value(value: &str) -> String {
    value.replace('\r', "").replace('\n', " ")
}
