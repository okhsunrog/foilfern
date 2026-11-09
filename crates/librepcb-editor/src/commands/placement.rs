//! Automatic placement of symbols in schematics and devices on boards
//! (no upstream counterpart: upstream places interactively at the cursor).
//! Used by agents which add parts without coordinates.
//!
//! Hand-written, deterministic algorithms on a grid (no crate implements
//! this against LibrePCB geometry):
//!
//! - symbols ([`AutoPlaceSymbols`]): first-fit packing row by row of the
//!   symbol rectangle (graphics, pins, estimated text extents) extended by
//!   room for a pin stub with a net label in the direction of each pin;
//! - devices ([`AutoPlaceDevices`]): greedy placement by connectivity
//!   (minimizing the air wire length, connectors at the board edge) of the
//!   footprint rectangle (courtyard, pads, texts), with row packing as the
//!   fallback on crowded boards.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::library::pkg::Footprint;
use librepcb_core::library::sym::Symbol;
use librepcb_core::project::board::{Board, BoardDevice};
use librepcb_core::project::{
    BoardId, ComponentInstanceId, NetSignalId, Project, ProjectAttributeLookup, SchematicId,
    SymbolId,
};
use librepcb_core::types::{
    Alignment, Angle, HAlign, Layer, Length, Point, PositiveLength, UnsignedLength, VAlign,
};
use librepcb_core::utils::transform::Transform;
use librepcb_i18n::tr;

use super::component::{AddDevice, MoveDevice, MoveSymbol};
use super::{ComponentRef, resolve};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

/// An axis aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    /// Bottom left corner.
    pub min: Point,
    /// Top right corner.
    pub max: Point,
}

impl Rect {
    /// The rectangle containing all `points`, if any.
    pub fn bounding(points: impl IntoIterator<Item = Point>) -> Option<Self> {
        let mut it = points.into_iter();
        let first = it.next()?;
        let mut r = Rect {
            min: first,
            max: first,
        };
        for p in it {
            r.min.x = r.min.x.min(p.x);
            r.min.y = r.min.y.min(p.y);
            r.max.x = r.max.x.max(p.x);
            r.max.y = r.max.y.max(p.y);
        }
        Some(r)
    }

    /// Width.
    pub fn width(&self) -> Length {
        self.max.x - self.min.x
    }

    /// Height.
    pub fn height(&self) -> Length {
        self.max.y - self.min.y
    }

    /// The rectangle grown by `margin` on each side.
    pub fn expanded(&self, margin: Length) -> Self {
        Rect {
            min: Point::new(self.min.x - margin, self.min.y - margin),
            max: Point::new(self.max.x + margin, self.max.y + margin),
        }
    }

    /// The rectangle moved by `offset`.
    pub fn translated(&self, offset: Point) -> Self {
        Rect {
            min: self.min + offset,
            max: self.max + offset,
        }
    }

    /// Whether the rectangles overlap (touching edges do not overlap).
    pub fn intersects(&self, other: &Rect) -> bool {
        self.min.x < other.max.x
            && other.min.x < self.max.x
            && self.min.y < other.max.y
            && other.min.y < self.max.y
    }

    /// Whether `other` lies completely inside this rectangle.
    pub fn contains(&self, other: &Rect) -> bool {
        self.min.x <= other.min.x
            && self.min.y <= other.min.y
            && other.max.x <= self.max.x
            && other.max.y <= self.max.y
    }

    fn corners(&self) -> [Point; 4] {
        [
            self.min,
            Point::new(self.max.x, self.min.y),
            self.max,
            Point::new(self.min.x, self.max.y),
        ]
    }

    /// The bounding rectangle of this rectangle mapped by `transform`.
    pub(crate) fn mapped(&self, transform: &Transform) -> Self {
        // A rectangle has 4 corners, so the bounding rect always exists.
        Rect::bounding(self.corners().map(|c| transform.map(&c))).unwrap_or(*self)
    }
}

/// Rounds `v` to a multiple of `grid`.
fn snap(v: Length, grid: Length) -> Length {
    let g = grid.to_nm().max(1);
    let n = v.to_nm();
    Length::new((n as f64 / g as f64).round() as i64 * g)
}

/// Bounding rectangle of a library symbol in symbol coordinates: polygons,
/// circles, pins (with their length) and, if `with_texts`, the text
/// anchors.
pub fn library_symbol_rect(symbol: &Symbol, with_texts: bool) -> Rect {
    let mut points = Vec::new();
    for polygon in symbol.polygons().iter() {
        points.extend(polygon.path().vertices().iter().map(|v| v.pos));
    }
    for circle in symbol.circles().iter() {
        let r = *circle.diameter() / 2;
        points.push(Point::new(circle.center().x - r, circle.center().y - r));
        points.push(Point::new(circle.center().x + r, circle.center().y + r));
    }
    for pin in symbol.pins().iter() {
        points.push(pin.position());
        points.push(
            Point::new(pin.position().x + *pin.length(), pin.position().y)
                .rotated(pin.rotation(), pin.position()),
        );
    }
    if with_texts {
        for text in symbol.texts().iter() {
            points.push(text.position());
        }
    }
    Rect::bounding(points).unwrap_or(Rect {
        min: Point::new(Length::new(-2_540_000), Length::new(-2_540_000)),
        max: Point::new(Length::new(2_540_000), Length::new(2_540_000)),
    })
}

/// Estimated advance of a net label character (upstream draws net labels
/// with Noto Sans Mono of 4 px, about 0.85 mm per character; rounded up).
const LABEL_CHAR_WIDTH: Length = Length::new(900_000);
/// Estimated height of net label text above its line.
const LABEL_TEXT_HEIGHT: Length = Length::new(2_000_000);
/// Room reserved in the direction of each symbol pin for a stub wire
/// carrying a net label of about 8 characters.
pub(crate) const PIN_ROOM: Length = Length::new(12_700_000);

/// Estimated length of a net label with `chars` characters.
pub(crate) fn net_label_length(chars: usize) -> Length {
    LABEL_CHAR_WIDTH * chars.max(1) as i64 + Length::new(400_000)
}

/// Estimated rectangle of a net label with `chars` characters at `anchor`
/// whose text runs in `direction` (a unit direction `(dx, dy)`, along the
/// wire it sits on), with the text above horizontal lines and left of
/// vertical lines like upstream's rendering.
pub(crate) fn net_label_rect(anchor: Point, direction: (i64, i64), chars: usize) -> Rect {
    let len = net_label_length(chars);
    let (dx, dy) = direction;
    let margin = Length::new(300_000);
    let (a, b) = if dx != 0 {
        (
            Point::new(anchor.x, anchor.y - margin),
            Point::new(anchor.x + len * dx, anchor.y + LABEL_TEXT_HEIGHT),
        )
    } else {
        (
            Point::new(anchor.x - LABEL_TEXT_HEIGHT, anchor.y),
            Point::new(anchor.x + margin, anchor.y + len * dy),
        )
    };
    Rect::bounding([a, b]).unwrap_or(Rect { min: a, max: a })
}

/// The direction in which the text of a net label with `rotation` and
/// `mirrored` runs: along its wire, towards the point the wire continues
/// to (see [`net_label_orientation()`](super::schematic::net_label_orientation)).
pub(crate) fn net_label_direction(rotation: Angle, mirrored: bool) -> (i64, i64) {
    let unit = Length::new(1_000_000);
    let wanted = (rotation.mapped_to_0_360deg(), mirrored);
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .find(|(dx, dy)| {
            let along_wire = Point::new(unit * *dx, unit * *dy);
            super::schematic::net_label_orientation(Point::ORIGIN, along_wire) == wanted
        })
        .unwrap_or((1, 0))
}

/// Estimated rectangle of a text of `chars` characters (height `height`,
/// about 0.6 x height per character), aligned at `anchor`.
fn text_rect(
    anchor: Point,
    height: Length,
    align: Alignment,
    rotation: Angle,
    chars: usize,
) -> Rect {
    let w = height * 6 * chars.max(1) as i64 / 10;
    let h = height;
    let (x0, x1) = match align.h {
        HAlign::Left => (Length::ZERO, w),
        HAlign::Center => (-w / 2, w / 2),
        HAlign::Right => (-w, Length::ZERO),
    };
    let (y0, y1) = match align.v {
        VAlign::Bottom => (Length::ZERO, h),
        VAlign::Center => (-h / 2, h / 2),
        VAlign::Top => (-h, Length::ZERO),
    };
    Rect {
        min: Point::new(x0, y0),
        max: Point::new(x1, y1),
    }
    .mapped(&Transform::new(anchor, rotation, false))
}

/// Bounding rectangle of a placed symbol in schematic coordinates: the
/// library symbol graphics and pins, and if `with_texts` the estimated
/// extents of its texts (name, value, with attributes substituted).
pub(crate) fn placed_symbol_rect(
    p: &Project,
    schematic: SchematicId,
    symbol: SymbolId,
    with_texts: bool,
) -> Result<Rect> {
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    let sym = s
        .symbols()
        .get(&symbol)
        .ok_or_else(|| Error::not_found("Symbol", symbol))?;
    let resolved = sym.resolve(p.view())?;
    let body = library_symbol_rect(resolved.lib_symbol, false).mapped(&sym.transform());
    if !with_texts {
        return Ok(body);
    }
    let lookup = ProjectAttributeLookup::for_symbol(p, s, sym, None, None, None);
    let mut points = vec![body.min, body.max];
    for text in sym.texts().values() {
        let value = lookup.substitute(text.text());
        // Unresolved attributes: assume a typical value length.
        let chars = if value.contains("{{") {
            8
        } else {
            value.chars().count()
        };
        if chars == 0 {
            continue;
        }
        let r = text_rect(
            text.position(),
            *text.height(),
            text.align(),
            text.rotation(),
            chars,
        );
        points.extend([r.min, r.max]);
    }
    Ok(Rect::bounding(points).unwrap_or(body))
}

/// The rectangle a placed symbol needs for readable wiring: the symbol
/// with its texts (see [`placed_symbol_rect()`]), extended in the
/// direction of each pin by [`PIN_ROOM`] for a stub wire with a net label.
pub(crate) fn symbol_room_rect(
    p: &Project,
    schematic: SchematicId,
    symbol: SymbolId,
) -> Result<Rect> {
    let rect = placed_symbol_rect(p, schematic, symbol, true)?;
    let sym = p
        .schematic(schematic)
        .and_then(|s| s.symbols().get(&symbol))
        .ok_or_else(|| Error::not_found("Symbol", symbol))?;
    let mut points = vec![rect.min, rect.max];
    for pin in sym.pins(p.view())? {
        let pos = pin.position();
        let end = Point::new(pos.x + PIN_ROOM, pos.y).rotated(pin.rotation() + Angle::DEG180, pos);
        let dir = (
            (pos.x - end.x).to_nm().signum(),
            (pos.y - end.y).to_nm().signum(),
        );
        // A label on the stub, running back to the pin.
        let r = net_label_rect(end, dir, 8);
        points.extend([r.min, r.max, end]);
    }
    Ok(Rect::bounding(points).unwrap_or(rect))
}

/// Symbols larger than this (in both dimensions) are considered frames and
/// ignored as obstacles.
const FRAME_SIZE: (Length, Length) = (Length::new(150_000_000), Length::new(100_000_000));

/// Whether a placed symbol is a frame (see [`FRAME_SIZE`]).
pub(crate) fn is_frame(p: &Project, schematic: SchematicId, symbol: SymbolId) -> Result<bool> {
    let r = placed_symbol_rect(p, schematic, symbol, false)?;
    Ok(r.width() > FRAME_SIZE.0 && r.height() > FRAME_SIZE.1)
}

/// Obstacles on a schematic page: all symbols (except `exclude` and
/// frames; with texts, and with the room for pin labels if `room`), net
/// lines, junctions and net labels.
pub(crate) fn schematic_obstacles(
    p: &Project,
    schematic: SchematicId,
    exclude: &BTreeSet<SymbolId>,
    room: bool,
) -> Result<Vec<Rect>> {
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    let mut rects = Vec::new();
    for id in s.symbols().keys() {
        if exclude.contains(id) || is_frame(p, schematic, *id)? {
            continue;
        }
        rects.push(if room {
            symbol_room_rect(p, schematic, *id)?
        } else {
            placed_symbol_rect(p, schematic, *id, true)?
        });
    }
    for (id, segment) in s.net_segments() {
        for line in segment.lines().values() {
            let ends = [line.p1(), line.p2()]
                .map(|a| s.net_line_anchor_position(*id, a, p.view()))
                .into_iter()
                .flatten();
            if let Some(r) = Rect::bounding(ends) {
                rects.push(r);
            }
        }
        for junction in segment.junctions().values() {
            rects.push(Rect {
                min: junction.position(),
                max: junction.position(),
            });
        }
        let chars = p
            .circuit()
            .net_signal(segment.net())
            .map_or(8, |n| n.name().as_str().chars().count());
        for l in segment.labels().values() {
            let direction = net_label_direction(l.rotation(), l.mirrored());
            rects.push(net_label_rect(l.position(), direction, chars));
        }
    }
    Ok(rects)
}

/// Area scanned for free schematic positions: from the top left corner
/// (x, y) to the right by the width, rows downwards.
pub(crate) const SCHEMATIC_AREA: (Length, Length, Length) = (
    Length::new(20_320_000),
    Length::new(160_020_000),
    Length::new(203_200_000),
);

/// Returns the first free position (symbol origin, on the 2.54 mm grid)
/// for a symbol whose rect relative to its origin is `local`, scanning
/// rows from the top left of the schematic area.
fn free_position(local: Rect, obstacles: &[Rect], margin: Length) -> Point {
    let grid = Length::new(2_540_000);
    let step = Length::new(5_080_000);
    let (left, top, width) = SCHEMATIC_AREA;
    let mut y = top;
    // Scan at most ~2.5 m downwards; afterwards place below everything.
    for _ in 0..500 {
        let mut x = left;
        while x + local.width() <= left + width || x == left {
            let origin = Point::new(snap(x - local.min.x, grid), snap(y - local.max.y, grid));
            let r = local.translated(origin).expanded(margin);
            if !obstacles.iter().any(|o| o.intersects(&r)) {
                return origin;
            }
            x += step;
        }
        y -= step;
    }
    let bottom = obstacles
        .iter()
        .map(|o| o.min.y)
        .min()
        .unwrap_or(Length::ZERO);
    Point::new(
        snap(left - local.min.x, grid),
        snap(bottom - margin - local.max.y, grid),
    )
}

/// Moves symbols to free positions on their schematic pages (no overlap
/// with other symbols, wires and labels), in the given order, filling rows
/// of about 200 mm from the top left; gates of a component end up next to
/// each other. Every symbol keeps room for a stub wire with a net label in
/// the direction of each pin (12.7 mm), so labels of neighbors do
/// not collide. Keeps rotation and mirroring.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutoPlaceSymbols {
    /// The symbols to place.
    pub symbols: Vec<SymbolId>,
    /// Free space around each symbol and its pin room (default: 2.54 mm).
    #[serde(default)]
    pub margin: Option<UnsignedLength>,
}

impl Command for AutoPlaceSymbols {
    /// The new positions (in the order of the symbols).
    type Output = Vec<Point>;

    fn text(&self) -> String {
        tr!("CmdMoveSelectedSchematicItems", "Move Schematic Items")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Vec<Point>> {
        let margin = self
            .margin
            .map_or(Length::new(2_540_000), |m| *m)
            .max(Length::ZERO);
        let exclude: BTreeSet<SymbolId> = self.symbols.iter().copied().collect();
        let mut positions = Vec::new();
        let mut obstacles: Vec<(SchematicId, Vec<Rect>)> = Vec::new();
        for symbol in self.symbols {
            let schematic = super::schematic::symbol_schematic(tx.project(), symbol)?;
            let index = match obstacles.iter().position(|(s, _)| *s == schematic) {
                Some(i) => i,
                None => {
                    let rects = schematic_obstacles(tx.project(), schematic, &exclude, true)?;
                    obstacles.push((schematic, rects));
                    obstacles.len() - 1
                }
            };
            let p = tx.project();
            let sym = p
                .schematic(schematic)
                .and_then(|s| s.symbols().get(&symbol))
                .ok_or_else(|| Error::not_found("Symbol", symbol))?;
            let placed = symbol_room_rect(p, schematic, symbol)?;
            let local = placed.translated(Point::ORIGIN - sym.position());
            let origin = free_position(local, &obstacles[index].1, margin);
            obstacles[index].1.push(local.translated(origin));
            tx.run(MoveSymbol {
                symbol,
                position: Some(origin),
                rotation: None,
                mirrored: None,
            })?;
            positions.push(origin);
        }
        Ok(positions)
    }
}

/// Bounding rectangle of a footprint in footprint coordinates: the
/// courtyard (else package outlines, documentation or legend) united with
/// the pads.
pub fn footprint_rect(footprint: &Footprint) -> Rect {
    let (bl, tr) = footprint.calculate_bounding_rect(true);
    let mut points = Vec::new();
    if bl != tr {
        points.extend([bl, tr]);
    }
    for pad in footprint.pads().iter() {
        let pad = pad.pad();
        let r = (*pad.width()).max(*pad.height()) / 2;
        points.push(Point::new(pad.position().x - r, pad.position().y - r));
        points.push(Point::new(pad.position().x + r, pad.position().y + r));
    }
    Rect::bounding(points).unwrap_or(Rect {
        min: Point::new(Length::new(-500_000), Length::new(-500_000)),
        max: Point::new(Length::new(500_000), Length::new(500_000)),
    })
}

/// Bounding rectangle of a placed device in board coordinates.
pub fn device_rect(p: &Project, device: &BoardDevice) -> Result<Rect> {
    let lib = p.library();
    let dev = lib
        .device(&device.lib_device())
        .ok_or_else(|| Error::not_found("Device", device.lib_device()))?;
    let package = lib
        .package(&dev.package_uuid())
        .ok_or_else(|| Error::not_found("Package", dev.package_uuid()))?;
    let footprint = package
        .footprints()
        .by_uuid(&device.lib_footprint())
        .ok_or_else(|| Error::not_found("Footprint", device.lib_footprint()))?;
    let rect = footprint_rect(footprint).mapped(&device.transform());
    // Stroke texts (name, value), roughly: the anchor with the text height
    // around it and an estimated width.
    let mut points = vec![rect.min, rect.max];
    for text in device.stroke_texts().values() {
        let h = *text.height();
        // Texts are mostly attributes like "{{NAME}}" whose values are
        // short (e.g. "R1"); estimate 0.7 x height per character.
        let chars = text
            .text()
            .chars()
            .filter(|c| *c != '{' && *c != '}')
            .count()
            .max(2) as i64;
        let w = h * chars * 7 / 10;
        let pos = text.position();
        points.push(Point::new(pos.x - w / 2, pos.y - h));
        points.push(Point::new(pos.x + w / 2, pos.y + h));
    }
    Ok(Rect::bounding(points).unwrap_or(rect))
}

/// Returns the outline polygons of a board as closed paths (arcs
/// flattened) and their bounding rectangle, or `None` without outline.
fn board_outline(board: &Board) -> Option<(Vec<Vec<Point>>, Rect)> {
    let tolerance = PositiveLength::new(Length::new(10_000)).ok()?;
    let paths: Vec<Vec<Point>> = board
        .polygons()
        .values()
        .filter(|p| p.layer() == Layer::BOARD_OUTLINES)
        .map(|p| {
            p.path()
                .flattened_arcs(tolerance)
                .vertices()
                .iter()
                .map(|v| v.pos)
                .collect::<Vec<_>>()
        })
        .filter(|v| v.len() >= 3)
        .collect();
    // The outer outline is the one with the largest bounding rect.
    let rect = paths
        .iter()
        .filter_map(|v| Rect::bounding(v.iter().copied()))
        .max_by_key(|r| (r.width().to_nm() as i128) * (r.height().to_nm() as i128))?;
    Some((paths, rect))
}

/// Point in polygon (even-odd ray casting).
fn inside(polygon: &[Point], p: Point) -> bool {
    let mut result = false;
    let n = polygon.len();
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + n - 1) % n];
        if (a.y > p.y) != (b.y > p.y) {
            let (ax, ay) = (a.x.to_nm() as f64, a.y.to_nm() as f64);
            let (bx, by) = (b.x.to_nm() as f64, b.y.to_nm() as f64);
            let x = (bx - ax) * (p.y.to_nm() as f64 - ay) / (by - ay) + ax;
            if (p.x.to_nm() as f64) < x {
                result = !result;
            }
        }
    }
    result
}

/// Adds the devices of components to a board inside the board outline,
/// placed by connectivity (strongly connected devices next to each other,
/// minimizing the air wire length; connectors at the board edge; see
/// `place_devices()`) without overlapping other devices, keeping a gap
/// between footprints (`spacing`, by default spread over the board area)
/// and the board clearance of the DRC settings to the board edge.
/// Components without device on the board are placed (default: all which
/// have an assembly option and are not schematic-only), with the device of
/// their first assembly option and the first footprint. Placed components
/// which do not fit are put to the right of the board and reported.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutoPlaceDevices {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The components to place (default: all unplaced ones).
    #[serde(default)]
    pub components: Vec<ComponentRef>,
    /// Space between footprints (default: at least 1 mm, more on boards
    /// with free area, up to 5 mm).
    #[serde(default)]
    pub spacing: Option<UnsignedLength>,
}

/// Result of [`AutoPlaceDevices`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutoPlacedDevices {
    /// The board.
    pub board: Option<BoardId>,
    /// The placed components with their device position, inside the
    /// outline.
    pub placed: Vec<(ComponentInstanceId, Point)>,
    /// Components which did not fit into the outline (placed right of
    /// the board).
    pub outside: Vec<ComponentInstanceId>,
    /// Components skipped because they have no device (schematic-only or
    /// no assembly option).
    pub skipped: Vec<ComponentInstanceId>,
}

impl Command for AutoPlaceDevices {
    type Output = AutoPlacedDevices;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdAddDeviceToBoard",
            "Add device to board"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<AutoPlacedDevices> {
        let p = tx.project();
        let board = resolve::board(p, self.board)?;
        let board_id = board.id();
        let (outlines, outline_rect) = board_outline(board).ok_or_else(|| {
            Error::InvalidArgument("The board has no outline; set the outline first.".to_owned())
        })?;
        let spacing = self.spacing.map_or(Length::new(1_000_000), |s| *s);
        let edge = *board.settings().drc_settings.min_copper_board_clearance() + spacing / 2;

        // Components to place.
        let mut todo: Vec<ComponentInstanceId> = Vec::new();
        let mut skipped = Vec::new();
        if self.components.is_empty() {
            let mut all: Vec<(String, ComponentInstanceId)> = p
                .circuit()
                .component_instances()
                .iter()
                .filter(|(id, _)| board.device(**id).is_none())
                .map(|(id, c)| (c.name().to_string(), *id))
                .collect();
            all.sort_by(|a, b| natural_cmp(&a.0, &b.0));
            for (_, id) in all {
                let c = p.circuit().component_instance(id);
                let schematic_only = c
                    .and_then(|c| p.library().component(&c.lib_component()))
                    .is_some_and(|c| c.schematic_only());
                let has_device = c.is_some_and(|c| !c.assembly_options().is_empty());
                if schematic_only || !has_device {
                    skipped.push(id);
                } else {
                    todo.push(id);
                }
            }
        } else {
            for r in &self.components {
                let id = resolve::component(p, r)?;
                if board.device(id).is_some() {
                    return Err(Error::InvalidArgument(format!(
                        "The component \"{}\" is already placed on the board.",
                        resolve::component_name(p, id)
                    )));
                }
                todo.push(id);
            }
        }
        let existing: Vec<Rect> = board
            .devices()
            .values()
            .map(|d| device_rect(p, d))
            .collect::<Result<_>>()?;

        // Add all devices first (at the board center) to know their size
        // and pads.
        let center = Point::new(
            (outline_rect.min.x + outline_rect.max.x) / 2,
            (outline_rect.min.y + outline_rect.max.y) / 2,
        );
        let mut items = Vec::new();
        for id in &todo {
            tx.run(AddDevice {
                component: ComponentRef::Id(*id),
                board: Some(board_id),
                device: None,
                footprint: None,
                position: center,
                rotation: Angle::DEG0,
                mirrored: false,
            })?;
            let p = tx.project();
            let dev = resolve::board(p, Some(board_id))?
                .device(*id)
                .ok_or_else(|| Error::not_found("Device", id))?;
            let rect = device_rect(p, dev)?.translated(Point::ORIGIN - center);
            let pads = dev
                .pads(p.library(), p.circuit())?
                .iter()
                .filter_map(|pad| Some((pad.net()?, pad.position() - center)))
                .collect();
            items.push(DeviceItem {
                component: *id,
                rect,
                pads,
                connector: is_connector(p, *id),
            });
        }
        // Pads of the devices already on the board (fixed).
        let p = tx.project();
        let mut fixed_pads: Vec<(NetSignalId, Point)> = Vec::new();
        for dev in resolve::board(p, Some(board_id))?.devices().values() {
            if todo.contains(&dev.component()) {
                continue;
            }
            for pad in dev.pads(p.library(), p.circuit())? {
                if let Some(net) = pad.net() {
                    fixed_pads.push((net, pad.position()));
                }
            }
        }

        // Without explicit spacing, spread the devices over the board: the
        // gap grows with the free area per device (up to 5 mm), which also
        // leaves room for traces.
        let area = outline_rect.expanded(-edge);
        let gap = match self.spacing {
            Some(s) => *s,
            None if !items.is_empty() => {
                let board_area = area.width().to_mm() * area.height().to_mm();
                let per_device = (board_area * 0.5 / items.len() as f64).sqrt();
                let size: f64 = items
                    .iter()
                    .map(|i| (i.rect.width().to_mm() * i.rect.height().to_mm()).sqrt())
                    .sum::<f64>()
                    / items.len() as f64;
                let spread = ((per_device - size) / 2.0).clamp(0.0, 5.0);
                spacing.max(Length::from_mm(spread).unwrap_or(spacing))
            }
            None => spacing,
        };
        let fits = |r: &Rect| {
            area.contains(r)
                && r.expanded(edge)
                    .corners()
                    .iter()
                    .all(|c| outlines.iter().filter(|o| inside(o, *c)).count() % 2 == 1)
        };
        // By connectivity; on crowded boards with less gap, else packed in
        // rows (whichever places the most devices).
        let mut best: Option<(usize, Vec<Option<Point>>, Length)> = None;
        let gaps = [gap, (gap + spacing) / 2, spacing];
        for (k, g) in gaps.iter().enumerate() {
            if k > 0 && *g == gaps[k - 1] {
                continue;
            }
            let obstacles: Vec<Rect> = existing.iter().map(|r| r.expanded(*g / 2)).collect();
            let origins = place_devices(&items, &fixed_pads, &obstacles, area, *g, &fits);
            let count = origins.iter().flatten().count();
            if best.as_ref().is_none_or(|(c, _, _)| count > *c) {
                best = Some((count, origins, *g));
            }
            if count == items.len() {
                break;
            }
        }
        if best.as_ref().is_some_and(|(c, _, _)| *c < items.len()) {
            let obstacles: Vec<Rect> = existing.iter().map(|r| r.expanded(spacing / 2)).collect();
            let origins = pack_rows(&items, &obstacles, area, spacing, &fits);
            let count = origins.iter().flatten().count();
            if best.as_ref().is_none_or(|(c, _, _)| count > *c) {
                best = Some((count, origins, spacing));
            }
        }
        let (_, origins, gap) = best.unwrap_or((0, vec![None; items.len()], spacing));

        let mut result = AutoPlacedDevices {
            board: Some(board_id),
            skipped,
            ..Default::default()
        };
        let grid = Length::new(100_000);
        let mut outside_x = outline_rect.max.x + Length::new(5_000_000);
        for (item, origin) in items.iter().zip(origins) {
            let origin = match origin {
                Some(origin) => {
                    result.placed.push((item.component, origin));
                    origin
                }
                None => {
                    let origin = Point::new(
                        snap(outside_x - item.rect.min.x, grid),
                        snap(outline_rect.max.y - item.rect.max.y, grid),
                    );
                    outside_x += item.rect.width() + gap;
                    result.outside.push(item.component);
                    origin
                }
            };
            tx.run(MoveDevice {
                component: ComponentRef::Id(item.component),
                board: Some(board_id),
                position: Some(origin),
                rotation: None,
                mirrored: None,
                locked: None,
            })?;
        }
        Ok(result)
    }
}

/// A device to place: its rectangle and pads relative to its origin.
struct DeviceItem {
    component: ComponentInstanceId,
    rect: Rect,
    pads: Vec<(NetSignalId, Point)>,
    connector: bool,
}

/// Whether a component is a connector (kept at the board edge): its
/// designator prefix is `J`, `X`, `P` or `CN` (IEEE 315 / IEC 81346).
fn is_connector(p: &Project, id: ComponentInstanceId) -> bool {
    let name = resolve::component_name(p, id);
    let prefix: String = name
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    matches!(prefix.to_ascii_uppercase().as_str(), "J" | "X" | "P" | "CN")
}

/// Packs devices in rows from the top left of `area` (largest first),
/// for crowded boards. Returns the origin of each item (`None` if it does
/// not fit).
fn pack_rows(
    items: &[DeviceItem],
    obstacles: &[Rect],
    area: Rect,
    gap: Length,
    fits: &dyn Fn(&Rect) -> bool,
) -> Vec<Option<Point>> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|i| {
        let r = &items[*i].rect;
        std::cmp::Reverse((r.width().to_nm() as i128) * (r.height().to_nm() as i128))
    });
    let step = Length::new(500_000);
    let grid = Length::new(100_000);
    let mut rects = obstacles.to_vec();
    let mut origins = vec![None; items.len()];
    for i in order {
        let local = items[i].rect;
        let mut y = area.max.y;
        'rows: while y - local.height() >= area.min.y {
            let mut x = area.min.x;
            while x + local.width() <= area.max.x {
                let origin = Point::new(snap(x - local.min.x, grid), snap(y - local.max.y, grid));
                let r = local.translated(origin);
                let padded = r.expanded(gap / 2);
                if fits(&r) && !rects.iter().any(|o| o.intersects(&padded)) {
                    rects.push(padded);
                    origins[i] = Some(origin);
                    break 'rows;
                }
                x += step;
            }
            y -= step;
        }
    }
    origins
}

/// Greedy placement of devices by connectivity (hand-written,
/// deterministic): the most connected device first (in the middle of the
/// area), then repeatedly the device most strongly connected to the placed
/// ones at the free position (1 mm steps) which minimizes the weighted
/// Manhattan length of its air wires to placed pads, with a small pull to
/// the center. Connectors only go to positions at the area edge. Returns
/// the origin of each item (`None` if it does not fit).
fn place_devices(
    items: &[DeviceItem],
    fixed_pads: &[(NetSignalId, Point)],
    obstacles: &[Rect],
    area: Rect,
    gap: Length,
    fits: &dyn Fn(&Rect) -> bool,
) -> Vec<Option<Point>> {
    let n = items.len();
    let mut net_count: BTreeMap<NetSignalId, BTreeSet<usize>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        for (net, _) in &item.pads {
            net_count.entry(*net).or_default().insert(i);
        }
    }
    let net_weight =
        |net: &NetSignalId| 1.0 / (net_count.get(net).map_or(2, |m| m.len()).max(2) - 1) as f64;
    let mut weight = vec![vec![0.0; n]; n];
    for members in net_count.values() {
        let w = 1.0 / (members.len().max(2) - 1) as f64;
        for a in members {
            for b in members {
                if a != b {
                    weight[*a][*b] += w;
                }
            }
        }
    }
    let total: Vec<f64> = weight.iter().map(|row| row.iter().sum()).collect();
    let center = Point::new((area.min.x + area.max.x) / 2, (area.min.y + area.max.y) / 2);
    let step = Length::new(1_000_000);
    let grid = Length::new(100_000);
    // Distance of a rectangle to the area border.
    let border_distance = |r: &Rect| {
        [
            r.min.x - area.min.x,
            area.max.x - r.max.x,
            r.min.y - area.min.y,
            area.max.y - r.max.y,
        ]
        .into_iter()
        .min()
        .unwrap_or(Length::ZERO)
    };

    let mut origins: Vec<Option<Point>> = vec![None; n];
    let mut done = vec![false; n];
    let mut rects: Vec<Rect> = obstacles.to_vec();
    let mut placed_pads: Vec<(NetSignalId, Point)> = fixed_pads.to_vec();
    for _ in 0..n {
        let Some(i) = (0..n).filter(|i| !done[*i]).max_by(|a, b| {
            let key = |i: usize| {
                let placed: f64 = (0..n)
                    .filter(|j| origins[*j].is_some())
                    .map(|j| weight[i][j])
                    .sum();
                (placed, total[i])
            };
            key(*a)
                .partial_cmp(&key(*b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.cmp(a))
        }) else {
            break;
        };
        done[i] = true;
        let item = &items[i];
        let cost = |origin: Point| -> f64 {
            let mut c = 0.0;
            for (net, offset) in &item.pads {
                let pad = origin + *offset;
                let nearest = placed_pads
                    .iter()
                    .filter(|(n, _)| n == net)
                    .map(|(_, q)| ((pad.x - q.x).abs() + (pad.y - q.y).abs()).to_mm())
                    .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                if let Some(d) = nearest {
                    c += net_weight(net) * d;
                }
            }
            let mid = origin
                + Point::new(
                    (item.rect.min.x + item.rect.max.x) / 2,
                    (item.rect.min.y + item.rect.max.y) / 2,
                );
            c + 0.02 * ((mid.x - center.x).abs() + (mid.y - center.y).abs()).to_mm()
        };
        let mut best: Option<(f64, Point)> = None;
        let mut y = area.max.y - item.rect.max.y;
        while y + item.rect.min.y >= area.min.y {
            let mut x = area.min.x - item.rect.min.x;
            while x + item.rect.max.x <= area.max.x {
                let origin = Point::new(snap(x, grid), snap(y, grid));
                let r = item.rect.translated(origin);
                let padded = r.expanded(gap / 2);
                let edge_ok = !item.connector || border_distance(&r) <= step;
                if edge_ok && fits(&r) && !rects.iter().any(|o| o.intersects(&padded)) {
                    let c = cost(origin);
                    if best.is_none_or(|(bc, _)| c < bc) {
                        best = Some((c, origin));
                    }
                }
                x += step;
            }
            y -= step;
        }
        // Connectors which fit nowhere at the edge go anywhere.
        if best.is_none() && item.connector {
            let mut y = area.max.y - item.rect.max.y;
            while best.is_none() && y + item.rect.min.y >= area.min.y {
                let mut x = area.min.x - item.rect.min.x;
                while x + item.rect.max.x <= area.max.x {
                    let origin = Point::new(snap(x, grid), snap(y, grid));
                    let r = item.rect.translated(origin);
                    if fits(&r) && !rects.iter().any(|o| o.intersects(&r.expanded(gap / 2))) {
                        best = Some((0.0, origin));
                        break;
                    }
                    x += step;
                }
                y -= step;
            }
        }
        if let Some((_, origin)) = best {
            origins[i] = Some(origin);
            rects.push(item.rect.translated(origin).expanded(gap / 2));
            placed_pads.extend(item.pads.iter().map(|(net, o)| (*net, origin + *o)));
        }
    }
    origins
}

/// Natural string order ("R2" < "R10").
pub(crate) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| {
        let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).collect();
        let prefix: String = s.chars().take_while(|c| !c.is_ascii_digit()).collect();
        (
            prefix,
            digits.parse::<u64>().unwrap_or(u64::MAX),
            s.to_owned(),
        )
    };
    key(a).cmp(&key(b))
}
