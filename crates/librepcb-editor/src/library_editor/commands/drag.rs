//! Port of libs/librepcb/editor/library/cmd/cmddragselectedsymbolitems.{h,cpp}
//! and cmddragselectedfootprintitems.{h,cpp}: moving, rotating, mirroring
//! and snapping a set of items of a symbol or footprint around their common
//! center.
//!
//! [`DragSelectedItems`] modifies the element immediately (upstream: the
//! child edit commands with `immediate = true`); it is used by the editor
//! FSMs inside an open undo group (live preview) and by the
//! [`TransformSymbolItems`](super::TransformSymbolItems) and
//! [`TransformFootprintItems`](super::TransformFootprintItems) commands.

use std::collections::BTreeSet;

use librepcb_core::types::{Angle, Orientation, Point, PositiveLength};

use super::transform::Transformable;
use crate::error::{Error, Result};

/// A container of selectable, transformable items (a symbol or a
/// footprint).
pub trait ItemContainer<I> {
    /// Calls `f` for every selected object, in upstream's order (per kind,
    /// in list order).
    fn for_each_selected(&mut self, items: &BTreeSet<I>, f: &mut dyn FnMut(&mut dyn Transformable));

    /// The points which make up the center of the selection (upstream: one
    /// per pin/pad/circle/text/hole, one per polygon/zone vertex).
    fn center_points(&self, items: &BTreeSet<I>) -> Vec<Point>;

    /// The positions of the selected items for "move/align" (upstream
    /// `getPositions()`: pads, circles, texts, holes).
    fn positions(&self, items: &BTreeSet<I>) -> Vec<Point>;

    /// Moves the items of [`positions()`](Self::positions) to new
    /// positions (same order).
    fn set_positions(&mut self, items: &BTreeSet<I>, positions: &[Point]) -> Result<()>;
}

/// An interactive transformation of selected items (upstream
/// `CmdDragSelected*Items`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragSelectedItems<I> {
    items: BTreeSet<I>,
    center: Point,
    delta: Point,
    rotation: Angle,
    mirrored_geometry: bool,
    mirrored_layer: bool,
    snapped: bool,
    positions_set: bool,
    has_off_grid_elements: bool,
    count: usize,
}

impl<I: Ord + Copy> DragSelectedItems<I> {
    /// Starts transforming `items` of `container` (upstream constructor:
    /// computes the center; for more than one point it is the average
    /// snapped to the grid).
    pub fn new<C: ItemContainer<I> + ?Sized>(
        container: &C,
        items: BTreeSet<I>,
        grid: PositiveLength,
    ) -> Self {
        let points = container.center_points(&items);
        let has_off_grid_elements = points.iter().any(|p| !p.is_on_grid(grid));
        let count = points.len();
        let mut center = points.iter().fold(Point::ORIGIN, |acc, p| acc + *p);
        if count > 1 {
            center = (center / count as i64).mapped_to_grid(grid);
        }
        Self {
            items,
            center,
            delta: Point::ORIGIN,
            rotation: Angle::DEG0,
            mirrored_geometry: false,
            mirrored_layer: false,
            snapped: false,
            positions_set: false,
            has_off_grid_elements,
            count,
        }
    }

    /// The transformed items.
    pub fn items(&self) -> &BTreeSet<I> {
        &self.items
    }

    /// Number of selected objects (upstream `getSelectedItemsCount()`
    /// counts objects, here: center points).
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The current center of the transformation.
    pub fn center(&self) -> Point {
        self.center
    }

    /// Whether any selected item is not on the grid.
    pub fn has_off_grid_elements(&self) -> bool {
        self.has_off_grid_elements
    }

    /// Whether anything was transformed (upstream: otherwise the command
    /// is discarded).
    pub fn is_modified(&self) -> bool {
        !(self.delta == Point::ORIGIN
            && self.rotation == Angle::DEG0
            && !self.mirrored_geometry
            && !self.mirrored_layer
            && !self.snapped
            && !self.positions_set)
    }

    /// Moves the items so that their total offset is `delta`.
    pub fn set_delta_to_start_pos<C: ItemContainer<I> + ?Sized>(
        &mut self,
        container: &mut C,
        delta: Point,
    ) {
        self.translate(container, delta - self.delta);
    }

    /// Moves the items by `delta`.
    pub fn translate<C: ItemContainer<I> + ?Sized>(&mut self, container: &mut C, delta: Point) {
        if delta != Point::ORIGIN {
            container.for_each_selected(&self.items, &mut |o| o.translate(delta));
            self.delta += delta;
            self.center += delta;
        }
    }

    /// Rotates the items around the center.
    pub fn rotate<C: ItemContainer<I> + ?Sized>(&mut self, container: &mut C, angle: Angle) {
        let center = self.center;
        container.for_each_selected(&self.items, &mut |o| o.rotate(angle, center));
        self.rotation += angle;
    }

    /// Mirrors the geometry of the items at the center.
    pub fn mirror_geometry<C: ItemContainer<I> + ?Sized>(
        &mut self,
        container: &mut C,
        orientation: Orientation,
    ) {
        let center = self.center;
        container.for_each_selected(&self.items, &mut |o| o.mirror_geometry(orientation, center));
        self.mirrored_geometry = !self.mirrored_geometry;
    }

    /// Moves the items to the other board side.
    pub fn mirror_layer<C: ItemContainer<I> + ?Sized>(&mut self, container: &mut C) {
        container.for_each_selected(&self.items, &mut |o| o.mirror_layer());
        self.mirrored_layer = !self.mirrored_layer;
    }

    /// Snaps the items to the grid.
    pub fn snap_to_grid<C: ItemContainer<I> + ?Sized>(
        &mut self,
        container: &mut C,
        grid: PositiveLength,
    ) {
        container.for_each_selected(&self.items, &mut |o| o.snap_to_grid(grid));
        self.snapped = true;
    }

    /// The positions of the items for "move/align".
    pub fn positions<C: ItemContainer<I> + ?Sized>(&self, container: &C) -> Vec<Point> {
        container.positions(&self.items)
    }

    /// Moves the items to new positions (upstream `setNewPositions()`).
    pub fn set_new_positions<C: ItemContainer<I> + ?Sized>(
        &mut self,
        container: &mut C,
        positions: &[Point],
    ) -> Result<()> {
        if positions.len() != container.positions(&self.items).len() {
            return Err(Error::InvalidArgument(
                "The number of positions does not match the selection.".to_owned(),
            ));
        }
        container.set_positions(&self.items, positions)?;
        self.positions_set = true;
        Ok(())
    }
}

/// One step of a transformation (the parameters of the transform
/// commands).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformOp {
    /// Move by an offset.
    Translate(Point),
    /// Rotate around the center of the selection.
    Rotate(Angle),
    /// Mirror the geometry at the center of the selection.
    Mirror(Orientation),
    /// Move to the other board side (footprints only).
    MirrorLayer,
    /// Snap to the grid.
    SnapToGrid,
    /// Set the positions of the items (see [`ItemContainer::positions()`]).
    SetPositions(Vec<Point>),
}

/// Applies `ops` to `items` of `container`; returns whether anything was
/// transformed.
pub(crate) fn apply_ops<I: Ord + Copy, C: ItemContainer<I> + ?Sized>(
    container: &mut C,
    items: BTreeSet<I>,
    grid: PositiveLength,
    ops: &[TransformOp],
) -> Result<bool> {
    let mut drag = DragSelectedItems::new(container, items, grid);
    for op in ops {
        match op {
            TransformOp::Translate(delta) => drag.translate(container, *delta),
            TransformOp::Rotate(angle) => drag.rotate(container, *angle),
            TransformOp::Mirror(orientation) => drag.mirror_geometry(container, *orientation),
            TransformOp::MirrorLayer => drag.mirror_layer(container),
            TransformOp::SnapToGrid => drag.snap_to_grid(container, grid),
            TransformOp::SetPositions(p) => drag.set_new_positions(container, p)?,
        }
    }
    Ok(drag.is_modified())
}
