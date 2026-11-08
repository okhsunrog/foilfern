//! Port of libs/librepcb/core/3d/scenedata3d.{h,cpp}, Board::buildScene3D()
//! and the geometric preparation in project/board/realisticboardpainter.cpp.
//!
//! An owned, renderer-independent snapshot feeds vector/2D renderers and
//! future 3D mesh builders. Existing pad rules, stroke fonts and Clipper 1
//! reproduce LibrePCB's physical geometry; there is no toolkit or GPU state.
mod extract;
mod surface;
#[cfg(test)]
mod tests;

use crate::{
    export::BoardSide,
    geometry::{NonEmptyPath, Path},
    project::{BoardId, ComponentInstanceId},
    types::{Angle3D, Layer, PcbColor, Point3D, PositiveLength, Uuid},
    utils::transform::Transform,
};
use std::collections::{BTreeMap, BTreeSet};

/// A drill's requested layer span, retained independently of the flat view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PhysicalHoleDepth {
    Through,
    Layers { start: Layer, end: Layer },
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysicalHole {
    pub path: NonEmptyPath,
    pub diameter: PositiveLength,
    pub plated: bool,
    pub via: bool,
    pub depth: PhysicalHoleDepth,
    /// Resolved physical span on enabled copper layers. Invalid via spans
    /// have no drill; flat exports still follow the requested source span.
    pub drill_layer_span: Option<(Layer, Layer)>,
}
/// A model reference and placement, without reading or tessellating STEP.
#[derive(Debug, Clone)]
pub struct PhysicalDevice {
    pub component: ComponentInstanceId,
    pub name: String,
    pub package: Uuid,
    pub model: Uuid,
    pub transform: Transform,
    pub model_position: Point3D,
    pub model_rotation: Angle3D,
}
/// Prepared flat material regions in board coordinates (nm, Y up).
/// Contours use even-odd winding, retaining holes as separate contours.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardSurfaceGeometry {
    pub body: Vec<Path>,
    pub copper: Vec<Path>,
    pub solder_resist: Vec<Path>,
    pub silkscreen: Vec<Path>,
    pub solder_paste: Vec<Path>,
}
/// Physical board data shared by flat exports and future 3D consumers.
/// Canonical outlines/drill spans remain available for extrusion; flat
/// material regions reproduce upstream's top/bottom projection rules.
#[derive(Debug, Clone)]
pub struct BoardPhysicalGeometry {
    pub board: BoardId,
    pub project_revision: u64,
    pub thickness: PositiveLength,
    pub inner_layer_count: u32,
    pub solder_resist: Option<PcbColor>,
    pub silkscreen: PcbColor,
    pub outlines: Vec<Path>,
    pub cutouts: Vec<Path>,
    pub holes: Vec<PhysicalHole>,
    pub devices: Vec<PhysicalDevice>,
    pub areas: BTreeMap<Layer, Vec<Path>>,
    pub surfaces: [BoardSurfaceGeometry; 2],
    silkscreen_layers: [BTreeSet<Layer>; 2],
}
static_assertions::assert_impl_all!(BoardPhysicalGeometry: Send, Sync);

impl BoardPhysicalGeometry {
    pub fn surface(&self, side: BoardSide) -> &BoardSurfaceGeometry {
        &self.surfaces[usize::from(side == BoardSide::Bottom)]
    }
    /// Source tolerance for physical polygon clipping: 5 micrometres.
    pub fn arc_tolerance() -> PositiveLength {
        PositiveLength::new(crate::types::Length::new(5_000)).expect("positive constant")
    }
}
