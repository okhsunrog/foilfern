//! Presentation details of imported sheets which have no native LibrePCB field.

use std::collections::{BTreeMap, BTreeSet};

use super::Schematic;
use crate::fileio::FileSystem;
use crate::geometry::{NetLabel, Path};
use crate::project::Project;
use crate::types::{Alignment, Angle, HAlign, Length, Point, Uuid, VAlign};
use crate::utils::overline_markup_parser::extract_overlines;

/// Optional appearance, keyed by native object IDs so electrical data stays in
/// the native project. Stored beside the sheet without extending its format.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ImportAppearance {
    pub global_labels: BTreeMap<Uuid, GlobalLabel>,
    pub pin_numbers: BTreeMap<Uuid, BTreeMap<Uuid, String>>,
    #[serde(default)]
    pub pin_names: BTreeMap<Uuid, BTreeMap<Uuid, String>>,
    pub no_connects: BTreeMap<Uuid, BTreeSet<Uuid>>,
    pub unpopulated: BTreeSet<Uuid>,
}

impl ImportAppearance {
    pub fn asset_path(schematic: &Schematic) -> String {
        format!(
            "schematics/{}/import-appearance.json",
            schematic.directory_name()
        )
    }

    pub fn load(project: &Project, schematic: &Schematic) -> Self {
        project
            .directory()
            .read_if_exists(&Self::asset_path(schematic))
            .ok()
            .flatten()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }
}

/// Shape and size of a KiCad global label.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
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
        let half = height * 3 / 4;
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
