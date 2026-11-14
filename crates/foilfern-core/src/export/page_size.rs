//! Page sizes of graphics exports: the part of Qt's `QPageSize` that
//! LibrePCB uses (identification by `QPageSize::key()` and the size in
//! PostScript points, which determines the page layout).
//!
//! No LibrePCB source file; the table is Qt 6's `qt_pageSizes` (keys and
//! `QPageSize::sizePoints()`) in the order of `QPageSize::PageSizeId`. The
//! graphics output jobs store the key (`paper` node). Ported because the
//! accepted keys decide which job files can be run.

use crate::types::Length;

/// A page size in PostScript points (1/72 inch). Predefined sizes are
/// portrait, except for the few landscape ones like `Ledger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageSize {
    key: &'static str,
    width_pt: u32,
    height_pt: u32,
}

impl PageSize {
    /// The predefined page size of a `QPageSize::key()` (e.g. `"A4"`,
    /// `"Letter"`), `None` for unknown keys. Qt's `Custom` ID has no valid
    /// size and is not included.
    pub fn from_key(key: &str) -> Option<Self> {
        PAGE_SIZES
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(key, width_pt, height_pt)| Self {
                key,
                width_pt: *width_pt,
                height_pt: *height_pt,
            })
    }

    /// Returns whether `key` is a key of Qt's page size table, including
    /// `Custom` (upstream `OutputJobRunner::buildPages()` accepts it, and
    /// the export then treats the invalid size as automatic).
    pub fn is_known_key(key: &str) -> bool {
        key == "Custom" || Self::from_key(key).is_some()
    }

    /// A custom page size (upstream `QPageSize(QSizeF(w, h),
    /// QPageSize::Millimeter, "Custom", QPageSize::ExactMatch)`), rounded to
    /// whole points like Qt.
    pub fn custom(width: Length, height: Length) -> Self {
        let to_pt = |l: Length| (l.to_mm() * 72.0 / 25.4).round().max(0.0) as u32;
        Self {
            key: "Custom",
            width_pt: to_pt(width),
            height_pt: to_pt(height),
        }
    }

    /// The key (`"Custom"` for custom sizes).
    pub fn key(&self) -> &'static str {
        self.key
    }

    /// Width in points.
    pub fn width_pt(&self) -> u32 {
        self.width_pt
    }

    /// Height in points.
    pub fn height_pt(&self) -> u32 {
        self.height_pt
    }

    /// Size in pixels at a resolution (upstream `QPageSize::rectPixels()`:
    /// the point size scaled and rounded).
    pub fn size_pixels(&self, dpi: u32) -> (u32, u32) {
        let px = |pt: u32| (f64::from(pt) * f64::from(dpi) / 72.0).round() as u32;
        (px(self.width_pt), px(self.height_pt))
    }

    /// All predefined keys in Qt's order.
    pub fn keys() -> impl Iterator<Item = &'static str> {
        PAGE_SIZES.iter().map(|(k, _, _)| *k)
    }
}

/// `(key, width, height)` in points.
#[rustfmt::skip]
const PAGE_SIZES: &[(&str, u32, u32)] = &[
    ("Letter", 612, 792), ("Legal", 612, 1008), ("Executive.7.5x10in", 540, 720),
    ("A0", 2384, 3370), ("A1", 1684, 2384), ("A2", 1191, 1684), ("A3", 842, 1191),
    ("A4", 595, 842), ("A5", 420, 595), ("A6", 297, 420), ("A7", 210, 297),
    ("A8", 148, 210), ("A9", 105, 148), ("A10", 73, 105),
    ("ISOB0", 2835, 4008), ("ISOB1", 2004, 2835), ("ISOB2", 1417, 2004),
    ("ISOB3", 1001, 1417), ("ISOB4", 709, 1001), ("ISOB5", 499, 709),
    ("ISOB6", 354, 499), ("ISOB7", 249, 354), ("ISOB8", 176, 249),
    ("ISOB9", 125, 176), ("ISOB10", 88, 125),
    ("EnvC5", 459, 649), ("Env10", 297, 684), ("EnvDL", 312, 624),
    ("Folio", 595, 935), ("Ledger", 1224, 792), ("Tabloid", 792, 1224),
    ("A3Extra", 913, 1262), ("A4Extra", 667, 914), ("A4Plus", 595, 936),
    ("A4Small", 595, 842), ("A5Extra", 492, 668), ("ISOB5Extra", 570, 782),
    ("B0", 2920, 4127), ("B1", 2064, 2920), ("B2", 1460, 2064), ("B3", 1032, 1460),
    ("B4", 729, 1032), ("B5", 516, 729), ("B6", 363, 516), ("B7", 258, 363),
    ("B8", 181, 258), ("B9", 127, 181), ("B10", 91, 127),
    ("AnsiC", 1224, 1584), ("AnsiD", 1584, 2448), ("AnsiE", 2448, 3168),
    ("LegalExtra", 684, 1080), ("LetterExtra", 684, 864), ("LetterPlus", 612, 914),
    ("LetterSmall", 612, 792), ("TabloidExtra", 864, 1296),
    ("ARCHA", 648, 864), ("ARCHB", 864, 1296), ("ARCHC", 1296, 1728),
    ("ARCHD", 1728, 2592), ("ARCHE", 2592, 3456),
    ("7x9", 504, 648), ("8x10", 576, 720), ("9x11", 648, 792), ("9x12", 648, 864),
    ("10x11", 720, 792), ("10x13", 720, 936), ("10x14", 720, 1008),
    ("12x11", 864, 792), ("15x11", 1080, 792),
    ("Executive", 522, 756), ("Note", 612, 792), ("Quarto", 610, 780),
    ("Statement", 396, 612), ("SuperA", 643, 1009), ("SuperB", 864, 1380),
    ("Postcard", 284, 419), ("DoublePostcard", 567, 419),
    ("PRC16K", 414, 610), ("PRC32K", 275, 428), ("PRC32KBig", 275, 428),
    ("FanFoldUS", 1071, 792), ("FanFoldGerman", 612, 864),
    ("FanFoldGermanLegal", 612, 936),
    ("EnvISOB4", 708, 1001), ("EnvISOB5", 499, 709), ("EnvISOB6", 499, 354),
    ("EnvC0", 2599, 3676), ("EnvC1", 1837, 2599), ("EnvC2", 1298, 1837),
    ("EnvC3", 918, 1296), ("EnvC4", 649, 918), ("EnvC6", 323, 459),
    ("EnvC65", 324, 648), ("EnvC7", 230, 323),
    ("Env9", 279, 639), ("Env11", 324, 747), ("Env12", 342, 792), ("Env14", 360, 828),
    ("EnvMonarch", 279, 540), ("EnvPersonal", 261, 468),
    ("EnvChou3", 340, 666), ("EnvChou4", 255, 581), ("EnvInvite", 624, 624),
    ("EnvItalian", 312, 652), ("EnvKaku2", 680, 941), ("EnvKaku3", 612, 785),
    ("EnvPRC1", 289, 468), ("EnvPRC2", 289, 499), ("EnvPRC3", 354, 499),
    ("EnvPRC4", 312, 590), ("EnvPRC5", 312, 624), ("EnvPRC6", 340, 652),
    ("EnvPRC7", 454, 652), ("EnvPRC8", 340, 876), ("EnvPRC9", 649, 918),
    ("EnvPRC10", 918, 1298), ("EnvYou4", 298, 666),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup() {
        let a4 = PageSize::from_key("A4").expect("A4");
        assert_eq!((a4.width_pt(), a4.height_pt()), (595, 842));
        assert_eq!(a4.size_pixels(72), (595, 842));
        assert_eq!(a4.size_pixels(600), (4958, 7017));
        assert_eq!(PageSize::from_key("Custom"), None);
        assert!(PageSize::is_known_key("Custom"));
        assert!(!PageSize::is_known_key("foo"));
        assert_eq!(PageSize::keys().count(), 118);
        let custom = PageSize::custom(Length::new(210_000_000), Length::new(297_000_000));
        assert_eq!((custom.width_pt(), custom.height_pt()), (595, 842));
    }
}
