//! Port of libs/librepcb/core/export/pickplacecsvwriter.{h,cpp}.
//!
//! Differences to upstream: the application version is passed in and the
//! generation date can be set (upstream: `Application::getVersion()` and
//! `QDateTime::currentDateTime()`).

use std::collections::HashSet;

use super::{BoardSide, PickPlaceData, PickPlaceDataItem, PickPlaceType, Result, Timestamp};
use crate::fileio::CsvFile;

/// The board side(s) exported by a [`PickPlaceCsvWriter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PickPlaceSides {
    /// Only the top side.
    Top,
    /// Only the bottom side.
    Bottom,
    /// Both sides.
    Both,
}

impl PickPlaceSides {
    fn contains(self, side: BoardSide) -> bool {
        match self {
            Self::Top => side == BoardSide::Top,
            Self::Bottom => side == BoardSide::Bottom,
            Self::Both => true,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Both => "Top + Bottom",
        }
    }
}

/// Writes [`PickPlaceData`] as CSV file.
#[derive(Debug, Clone)]
pub struct PickPlaceCsvWriter<'a> {
    data: &'a PickPlaceData,
    app_version: String,
    generation_date: Option<Timestamp>,
    board_sides: PickPlaceSides,
    type_filter: HashSet<PickPlaceType>,
    include_metadata_comment: bool,
    include_non_mounted_parts: bool,
}

impl<'a> PickPlaceCsvWriter<'a> {
    /// Creates a writer for `data`, exporting both sides and all types with
    /// a metadata comment. `app_version` is written into the comment.
    pub fn new(data: &'a PickPlaceData, app_version: impl Into<String>) -> Self {
        Self {
            data,
            app_version: app_version.into(),
            generation_date: None,
            board_sides: PickPlaceSides::Both,
            type_filter: PickPlaceType::ALL.into_iter().collect(),
            include_metadata_comment: true,
            include_non_mounted_parts: false,
        }
    }

    /// Sets the generation date written into the comment (default: the
    /// current time when generating).
    pub fn set_generation_date(&mut self, date: Timestamp) {
        self.generation_date = Some(date);
    }

    /// Sets the exported board side(s).
    pub fn set_board_sides(&mut self, sides: PickPlaceSides) {
        self.board_sides = sides;
    }

    /// Sets the exported assembly types.
    pub fn set_type_filter(&mut self, types: impl IntoIterator<Item = PickPlaceType>) {
        self.type_filter = types.into_iter().collect();
    }

    /// Sets whether a metadata comment is added as help for readers.
    pub fn set_include_metadata_comment(&mut self, include: bool) {
        self.include_metadata_comment = include;
    }

    /// Sets whether "do not mount" parts are included.
    pub fn set_include_non_mounted_parts(&mut self, include: bool) {
        self.include_non_mounted_parts = include;
    }

    /// Generates the CSV file.
    pub fn generate_csv(&self) -> Result<CsvFile> {
        let mut file = CsvFile::new();

        // Optionally add some metadata to to the CSV as a help for readers.
        if self.include_metadata_comment {
            let enabled_types: Vec<&str> = PickPlaceType::ALL
                .into_iter()
                .filter(|t| self.type_filter.contains(t))
                .map(PickPlaceType::name)
                .collect();
            let date = self.generation_date.unwrap_or_else(Timestamp::now);
            file.set_comment(format!(
                "Pick&Place Position Data File\n\
                 \n\
                 Project Name:        {}\n\
                 Project Version:     {}\n\
                 Board Name:          {}\n\
                 Generation Software: LibrePCB {}\n\
                 Generation Date:     {}\n\
                 Unit:                mm\n\
                 Rotation:            Degrees CCW\n\
                 Board Side:          {}\n\
                 Assembly Types:      {}",
                self.data.project_name(),
                self.data.project_version(),
                self.data.board_name(),
                self.app_version,
                date.to_iso_string(),
                self.board_sides.name(),
                enabled_types.join(", "),
            ));
        }

        // Don't translate the CSV header to make pick&place files
        // independent of the user's language.
        file.set_header([
            "Designator",
            "Value",
            "Device",
            "Package",
            "Position X",
            "Position Y",
            "Rotation",
            "Side",
            "Type",
        ]);
        for item in self.data.items().iter().filter(|i| self.is_exported(i)) {
            file.add_value([
                item.designator.clone(),
                item.value.clone(),
                item.device_name.clone(),
                item.package_name.clone(),
                item.position.x.to_mm_string(),
                item.position.y.to_mm_string(),
                item.rotation.mapped_to_0_360deg().to_deg_string(),
                match item.board_side {
                    BoardSide::Top => "Top",
                    BoardSide::Bottom => "Bottom",
                }
                .to_owned(),
                item.item_type.name().to_owned(),
            ])?;
        }
        Ok(file)
    }

    fn is_exported(&self, item: &PickPlaceDataItem) -> bool {
        self.board_sides.contains(item.board_side)
            && self.type_filter.contains(&item.item_type)
            && (item.mount || self.include_non_mounted_parts)
    }
}
