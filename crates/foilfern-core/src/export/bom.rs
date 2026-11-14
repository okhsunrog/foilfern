//! Port of libs/librepcb/core/export/bom.{h,cpp}.
//!
//! Designators and items are sorted with
//! [`toolbox::compare_numeric()`](crate::utils::toolbox::compare_numeric)
//! (upstream `Toolbox::sortNumeric()`), using a stable sort (upstream:
//! `std::sort`, whose order of equal elements is unspecified).

use std::cmp::Ordering;

use crate::utils::toolbox::compare_numeric;

/// An item (row) of a bill of materials: parts with identical attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BomItem {
    designators: Vec<String>,
    attributes: Vec<String>,
    mount: bool,
}

impl BomItem {
    /// Creates an item with a single designator.
    pub fn new(designator: impl Into<String>, attributes: Vec<String>, mount: bool) -> Self {
        Self {
            designators: vec![designator.into()],
            attributes,
            mount,
        }
    }

    /// Returns the designators, sorted numerically.
    pub fn designators(&self) -> &[String] {
        &self.designators
    }

    /// Returns the attribute values (one per [`Bom::columns()`]).
    pub fn attributes(&self) -> &[String] {
        &self.attributes
    }

    /// Returns whether the parts are mounted (`false` = "do not mount").
    pub fn is_mount(&self) -> bool {
        self.mount
    }

    /// Adds a designator (keeping the designators sorted to improve the
    /// readability of the BOM).
    pub fn add_designator(&mut self, designator: impl Into<String>) {
        self.designators.push(designator.into());
        self.designators.sort_by(|a, b| compare_numeric(a, b));
    }

    fn cmp_for_sorting(&self, other: &Self) -> Ordering {
        // Mounted items first, then by (first) designator.
        other.mount.cmp(&self.mount).then_with(|| {
            compare_numeric(
                self.designators.first().map_or("", String::as_str),
                other.designators.first().map_or("", String::as_str),
            )
        })
    }
}

/// A bill of materials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bom {
    columns: Vec<String>,
    mpn_manufacturer_columns: Vec<(usize, usize)>,
    items: Vec<BomItem>,
}

impl Bom {
    /// Creates an empty BOM with the given attribute columns and the
    /// (MPN, manufacturer) column index pairs.
    pub fn new(columns: Vec<String>, mpn_manufacturer_columns: Vec<(usize, usize)>) -> Self {
        Self {
            columns,
            mpn_manufacturer_columns,
            items: Vec::new(),
        }
    }

    /// Returns the attribute column names.
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Returns the (MPN, manufacturer) column index pairs.
    pub fn mpn_manufacturer_columns(&self) -> &[(usize, usize)] {
        &self.mpn_manufacturer_columns
    }

    /// Returns the items, sorted by mount state and designator.
    pub fn items(&self) -> &[BomItem] {
        &self.items
    }

    /// Returns the number of items with mounted parts.
    pub fn assembled_rows_count(&self) -> usize {
        self.items.iter().filter(|item| item.mount).count()
    }

    /// Returns the total number of mounted parts.
    pub fn total_assembled_parts_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.mount)
            .map(|item| item.designators.len())
            .sum()
    }

    /// Adds a part: merged into the item with identical attributes and
    /// mount state, or added as a new item. `attributes` must contain one
    /// value per column.
    pub fn add_item(&mut self, designator: &str, attributes: Vec<String>, mount: bool) {
        debug_assert_eq!(attributes.len(), self.columns.len());
        match self
            .items
            .iter_mut()
            .find(|item| (item.attributes == attributes) && (item.mount == mount))
        {
            Some(item) => item.add_designator(designator),
            None => self.items.push(BomItem::new(designator, attributes, mount)),
        }
        // Sort items by designator to improve readability of the BOM.
        self.items.sort_by(BomItem::cmp_for_sorting);
    }
}
