//! Presentation details which have no native LibrePCB field.

use std::collections::{BTreeMap, BTreeSet};

use super::Schematic;
use crate::fileio::FileSystem;
use crate::geometry::{NetLabel, Path};
use crate::project::Project;
use crate::types::{Alignment, Angle, HAlign, Length, Point, UnsignedLength, Uuid, VAlign};
use crate::utils::overline_markup_parser::extract_overlines;

/// Optional appearance, keyed by native object IDs so electrical data stays in
/// the native project. Stored beside the sheet without extending its format.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportAppearance {
    pub global_labels: BTreeMap<Uuid, GlobalLabel>,
    pub pin_numbers: BTreeMap<Uuid, BTreeMap<Uuid, String>>,
    #[serde(default)]
    pub pin_names: BTreeMap<Uuid, BTreeMap<Uuid, String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pin_text_heights: BTreeMap<Uuid, BTreeMap<Uuid, PinTextHeights>>,
    pub no_connects: BTreeMap<Uuid, BTreeSet<Uuid>>,
    pub unpopulated: BTreeSet<Uuid>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

impl ImportAppearance {
    pub fn asset_path(schematic: &Schematic) -> String {
        format!(
            "schematics/{}/import-appearance.json",
            schematic.directory_name()
        )
    }

    pub fn load(project: &Project, schematic: &Schematic) -> Self {
        Self::read(project, schematic).unwrap_or_default()
    }

    /// Reads the existing asset without discarding malformed data on an edit.
    pub fn read(project: &Project, schematic: &Schematic) -> crate::project::Result<Self> {
        let Some(data) = project
            .directory()
            .read_if_exists(&Self::asset_path(schematic))?
        else {
            return Ok(Self::default());
        };
        Ok(serde_json::from_slice(&data)?)
    }

    /// Line heights for this placed pin's legends, when the import supplies them.
    pub fn pin_text_heights(&self, symbol: Uuid, pin: Uuid) -> PinTextHeights {
        self.pin_text_heights
            .get(&symbol)
            .and_then(|pins| pins.get(&pin))
            .copied()
            .unwrap_or_default()
    }

    /// Returns whether this placed pin is marked as intentionally unconnected.
    pub fn is_no_connect(&self, symbol: Uuid, pin: Uuid) -> bool {
        self.no_connects
            .get(&symbol)
            .is_some_and(|pins| pins.contains(&pin))
    }

    /// Sets or clears a placed pin's marker. Returns whether it changed.
    pub fn set_no_connect(&mut self, symbol: Uuid, pin: Uuid, marked: bool) -> bool {
        if marked {
            return self.no_connects.entry(symbol).or_default().insert(pin);
        }
        let Some(pins) = self.no_connects.get_mut(&symbol) else {
            return false;
        };
        let changed = pins.remove(&pin);
        if pins.is_empty() {
            self.no_connects.remove(&symbol);
        }
        changed
    }
}

/// Native line-height equivalents of the two KiCad pin font sizes.
/// A zero height suppresses that legend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PinTextHeights {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<UnsignedLength>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<UnsignedLength>,
}

/// Shape and size of a KiCad global label.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GlobalLabel {
    pub input: bool,
    pub output: bool,
    pub height: Length,
}

impl GlobalLabel {
    /// Outline and text placement. The connection point is the native
    /// label anchor. Dimensions use the native font's approximate advance.
    pub fn geometry(&self, label: &NetLabel, name: &str) -> (Path, Point, Angle, Alignment) {
        let height = self.height.to_nm().clamp(1, 100_000_000);
        let half = height * 3 / 5;
        let advance = height * 11 / 20;
        let count = extract_overlines(name).0.chars().count().min(1000) as i64;
        let inset = if self.input { half } else { 0 };
        let end = inset + height * 3 / 5 + advance * count;
        let tip = if self.output { half } else { 0 };
        let rotation = label.rotation().mapped_to_0_360deg();
        let map = |x, y| label.position() + Point::from_nm(x, y).rotated(rotation, Point::ORIGIN);
        let outline = Path::new(
            [
                (0, 0),
                (inset, half),
                (end, half),
                (end + tip, 0),
                (end, -half),
                (inset, -half),
                (0, 0),
            ]
            .into_iter()
            .map(|(x, y)| map(x, y).into())
            .collect(),
        );
        let angle = match rotation {
            Angle::DEG180 => Angle::DEG0,
            Angle::DEG270 => Angle::DEG90,
            _ => rotation,
        };
        (
            outline,
            map(inset.midpoint(end), 0),
            angle,
            Alignment::new(HAlign::Center, VAlign::Center),
        )
    }
}

/// Two diagonals used by imported no-connect and DNP markers.
pub fn cross(a: Point, b: Point) -> [(Point, Point); 2] {
    [(a, b), (Point::new(a.x, b.y), Point::new(b.x, a.y))]
}

/// Body extent, with a centered marker for symbols without graphics.
pub fn symbol_cross(
    symbol: &crate::library::sym::Symbol,
    transform: &crate::utils::transform::Transform,
) -> [(Point, Point); 2] {
    let mut points = symbol
        .polygons()
        .iter()
        .flat_map(|p| p.path().vertices().iter().map(|v| v.pos))
        .chain(symbol.circles().iter().flat_map(|c| {
            let r = *c.diameter() / 2;
            let delta = Point::new(r, r);
            [c.center() - delta, c.center() + delta]
        }))
        .map(|p| transform.map(&p));
    let pad = Length::new(1_270_000);
    let center = transform.position;
    let delta = Point::new(pad, pad);
    let (mut min, mut max) = (center - delta, center + delta);
    if let Some(first) = points.next() {
        min = first;
        max = first;
        for p in points {
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x), max.y.max(p.y));
        }
        min -= delta;
        max += delta;
    }
    cross(min, max)
}
