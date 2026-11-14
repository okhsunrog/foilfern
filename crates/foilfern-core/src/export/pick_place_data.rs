//! Port of libs/librepcb/core/export/pickplacedata.{h,cpp}.
//!
//! Differences to upstream:
//! - `PickPlaceDataItem` is a plain struct with public fields (upstream: a
//!   9-argument constructor and getters), and its board side is the shared
//!   [`BoardSide`] enum.
//! - Items are sorted by designator with
//!   [`toolbox::compare_numeric()`](crate::utils::toolbox::compare_numeric),
//!   stable (upstream: `std::sort`, unspecified order of equal designators).

use super::BoardSide;
use crate::types::{Angle, Point};
use crate::utils::toolbox::compare_numeric;

/// Assembly type of a pick&place item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PickPlaceType {
    /// Pure THT package.
    Tht,
    /// Pure SMT package.
    Smt,
    /// Mixed THT/SMT package.
    Mixed,
    /// No package to mount, just a fiducial.
    Fiducial,
    /// Anything special, e.g. mechanical parts.
    Other,
}

impl PickPlaceType {
    /// All types, in the order used in files.
    pub const ALL: [Self; 5] = [
        Self::Tht,
        Self::Smt,
        Self::Mixed,
        Self::Fiducial,
        Self::Other,
    ];

    /// Returns the name written into pick&place files.
    pub fn name(self) -> &'static str {
        match self {
            Self::Tht => "THT",
            Self::Smt => "SMT",
            Self::Mixed => "THT+SMT",
            Self::Fiducial => "Fiducial",
            Self::Other => "Other",
        }
    }
}

/// One item (component) of a pick&place file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickPlaceDataItem {
    /// Component designator.
    pub designator: String,
    /// Component value.
    pub value: String,
    /// Device name.
    pub device_name: String,
    /// Package name.
    pub package_name: String,
    /// Position.
    pub position: Point,
    /// Rotation (counter-clockwise).
    pub rotation: Angle,
    /// Board side.
    pub board_side: BoardSide,
    /// Assembly type.
    pub item_type: PickPlaceType,
    /// Whether the part is mounted (`false` = "do not mount").
    pub mount: bool,
}

/// The content of a pick&place file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickPlaceData {
    project_name: String,
    project_version: String,
    board_name: String,
    items: Vec<PickPlaceDataItem>,
}

impl PickPlaceData {
    /// Creates empty data for the given project and board.
    pub fn new(
        project_name: impl Into<String>,
        project_version: impl Into<String>,
        board_name: impl Into<String>,
    ) -> Self {
        Self {
            project_name: project_name.into(),
            project_version: project_version.into(),
            board_name: board_name.into(),
            items: Vec::new(),
        }
    }

    /// Returns the project name.
    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    /// Returns the project version.
    pub fn project_version(&self) -> &str {
        &self.project_version
    }

    /// Returns the board name.
    pub fn board_name(&self) -> &str {
        &self.board_name
    }

    /// Returns the items, sorted by designator.
    pub fn items(&self) -> &[PickPlaceDataItem] {
        &self.items
    }

    /// Adds an item (keeping the items sorted by designator to improve the
    /// readability of the exported file).
    pub fn add_item(&mut self, item: PickPlaceDataItem) {
        let index = self
            .items
            .partition_point(|i| compare_numeric(&i.designator, &item.designator).is_le());
        self.items.insert(index, item);
    }
}
