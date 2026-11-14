//! Port of libs/librepcb/core/export/bomcsvwriter.{h,cpp}.

use super::{Bom, Result};
use crate::fileio::CsvFile;

/// Writes a [`Bom`] as CSV file.
#[derive(Debug, Clone)]
pub struct BomCsvWriter<'a> {
    bom: &'a Bom,
    include_non_mounted_parts: bool,
}

impl<'a> BomCsvWriter<'a> {
    /// Creates a writer for `bom`.
    pub fn new(bom: &'a Bom) -> Self {
        Self {
            bom,
            include_non_mounted_parts: false,
        }
    }

    /// Sets whether "do not mount" parts are included (with quantity 0;
    /// default: `false`).
    pub fn set_include_non_mounted_parts(&mut self, include: bool) {
        self.include_non_mounted_parts = include;
    }

    /// Generates the CSV file.
    pub fn generate_csv(&self) -> Result<CsvFile> {
        let mut file = CsvFile::new();
        // Don't translate the CSV header to make BOM files independent of
        // the user's language.
        file.set_header(
            ["Quantity", "Designators"]
                .into_iter()
                .map(str::to_owned)
                .chain(self.bom.columns().iter().cloned()),
        );
        for item in self.bom.items() {
            let count = if item.is_mount() {
                item.designators().len()
            } else {
                0
            };
            if (count == 0) && !self.include_non_mounted_parts {
                continue;
            }
            file.add_value(
                [count.to_string(), item.designators().join(", ")]
                    .into_iter()
                    .chain(item.attributes().iter().cloned()),
            )?;
        }
        Ok(file)
    }
}
