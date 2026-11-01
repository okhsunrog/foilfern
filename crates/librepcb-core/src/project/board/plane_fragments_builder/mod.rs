//! Port of libs/librepcb/core/project/board/boardplanefragmentsbuilder.{h,cpp}.
//!
//! Uses the existing exact Clipper 1 port and path conversions. Priority,
//! tolerance, thermal relief, minimum width and canonicalization follow
//! upstream because they define existing boards' copper geometry.
//! Computation is synchronous with progress/cancellation between planes;
//! callers may run it on their own worker threads. Results are applied only
//! after the entire build succeeds, retaining dirty state on error/cancel.

mod data;
use crate::geometry::{PadGeometry, PadGeometryShape, Path};
use crate::project::board::{Board, BoardPlane, PlaneConnectStyle};
use crate::project::{BoardId, EntityKind, Error, PlaneId, Project, Result};
use crate::types::{Angle, Layer, Length, Point, PositiveLength, UnsignedLength};
use crate::utils::clipper_helpers::{self as ch, ClipperPaths};
use crate::utils::toolbox;
use clipper::{JoinType, PolyFillType as Fill};
use data::Data;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

fn tolerance() -> PositiveLength {
    PositiveLength::new(Length::new(5_000)).expect("positive constant arc tolerance")
}
fn pos(length: Length) -> Result<PositiveLength> {
    Ok(PositiveLength::new(length)?)
}
fn offset(paths: &mut ClipperPaths, length: Length) -> Result<()> {
    Ok(ch::offset(paths, length, tolerance(), JoinType::Round)?)
}
fn convert(paths: &[Path]) -> ClipperPaths {
    ch::paths_to_clipper(paths, tolerance())
}
fn closed(path: &Path) -> Path {
    let mut path = path.clone();
    path.close();
    path
}
fn optional_clearance(clearance: UnsignedLength) -> Option<Length> {
    match clearance.to_nm() {
        0 => None,
        1 => Some(Length::ZERO),
        _ => Some(*clearance),
    }
}

/// A complete calculated result. Paths are in reproducible canonical order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneFragments {
    layers: BTreeSet<Layer>,
    fragments: BTreeMap<PlaneId, Vec<Path>>,
}
impl PlaneFragments {
    /// Returns the processed copper layers.
    pub fn layers(&self) -> &BTreeSet<Layer> {
        &self.layers
    }
    /// Returns the calculated paths, including empty planes.
    pub fn fragments(&self) -> &BTreeMap<PlaneId, Vec<Path>> {
        &self.fragments
    }
}

/// Calculates copper fills from an immutable project read view.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoardPlaneFragmentsBuilder;
impl BoardPlaneFragmentsBuilder {
    /// Rebuilds all planes, or only visible planes on dirty filtered layers.
    pub fn build(
        project: &Project,
        board: &Board,
        filter: Option<&BTreeSet<Layer>>,
    ) -> Result<PlaneFragments> {
        Self::build_with_progress(project, board, filter, |_, _| ControlFlow::Continue(()))
    }
    /// Reports completed/total plane counts; `Break(())` cancels without
    /// modifying the board or clearing its dirty layers.
    pub fn build_with_progress(
        project: &Project,
        board: &Board,
        filter: Option<&BTreeSet<Layer>>,
        mut progress: impl FnMut(usize, usize) -> ControlFlow<()>,
    ) -> Result<PlaneFragments> {
        let eligible: BTreeSet<_> = board
            .planes()
            .values()
            .filter(|p| filter.is_none_or(|f| p.visible() && f.contains(&p.layer())))
            .map(|p| p.layer())
            .collect();
        let mut layers: BTreeSet<_> = board
            .derived()
            .dirty_plane_layers()
            .intersection(&eligible)
            .copied()
            .collect();
        if filter.is_none() {
            layers.extend(eligible);
        }
        let mut output = PlaneFragments {
            layers,
            fragments: BTreeMap::new(),
        };
        if output.layers.is_empty() {
            return Ok(output);
        }
        let data = Data::collect(project, board, &output.layers)?;
        for (index, plane) in data.planes.iter().enumerate() {
            if progress(index, data.planes.len()).is_break() {
                return Err(Error::PlaneBuildCancelled);
            }
            let paths = build_plane(&data, plane, &output.fragments)?;
            output.fragments.insert(plane.id(), paths);
        }
        if progress(data.planes.len(), data.planes.len()).is_break() {
            return Err(Error::PlaneBuildCancelled);
        }
        Ok(output)
    }
}
impl Project {
    /// Rebuilds plane fragments atomically, journals changed planes and
    /// schedules affected air wires through the existing derived-data API.
    pub fn rebuild_planes(
        &mut self,
        board: BoardId,
        filter: Option<&BTreeSet<Layer>>,
    ) -> Result<()> {
        self.rebuild_planes_with_progress(board, filter, |_, _| ControlFlow::Continue(()))
    }
    /// Rebuilds with a caller-provided progress and cancellation callback.
    pub fn rebuild_planes_with_progress(
        &mut self,
        board: BoardId,
        filter: Option<&BTreeSet<Layer>>,
        progress: impl FnMut(usize, usize) -> ControlFlow<()>,
    ) -> Result<()> {
        let value = self.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let result =
            BoardPlaneFragmentsBuilder::build_with_progress(self, value, filter, progress)?;
        self.apply_plane_fragments(board, result.fragments)?;
        self.take_dirty_plane_layers(board, &result.layers)?;
        Ok(())
    }
}

fn build_plane(
    data: &Data,
    plane: &BoardPlane,
    previous: &BTreeMap<PlaneId, Vec<Path>>,
) -> Result<Vec<Path>> {
    let plane_outline = ch::path_to_clipper(&closed(plane.outline()), tolerance());
    let mut fragments = if let Some(clearance) = optional_clearance(plane.min_clearance_to_board())
    {
        let mut area = data.board_area.clone();
        if clearance != Length::ZERO {
            offset(&mut area, -clearance)?;
        }
        ch::intersect(
            &mut area,
            std::slice::from_ref(&plane_outline),
            Fill::EvenOdd,
            Fill::EvenOdd,
        )?;
        area
    } else {
        vec![plane_outline.clone()]
    };
    let full_area = fragments.clone();
    let mut removed = ClipperPaths::new();
    let mut connected = ClipperPaths::new();
    for other in &data.planes {
        if other.id() == plane.id() {
            break;
        }
        if other.layer() == plane.layer() && other.net() != plane.net() {
            let mut paths = convert(previous.get(&other.id()).map_or(&[], Vec::as_slice));
            offset(
                &mut paths,
                *plane
                    .min_clearance_to_copper()
                    .max(other.min_clearance_to_copper()),
            )?;
            removed.extend(paths);
        }
    }
    for (layers, outline) in &data.zones {
        if layers.contains(&plane.layer()) {
            removed.push(ch::path_to_clipper(outline, tolerance()));
        }
    }
    if let Some(clearance) = optional_clearance(plane.min_clearance_to_npth()) {
        for (diameter, path) in &data.holes {
            removed.extend(convert(
                &path.to_outline_strokes(pos(*diameter + clearance * 2)?),
            ));
        }
    }
    for via in &data.vias {
        if via.start.copper_number() > plane.layer().copper_number()
            || via.end.copper_number() < plane.layer().copper_number()
        {
            continue;
        }
        if plane.net().is_some() && via.net == plane.net() {
            connected.extend(convert(std::slice::from_ref(&via.path)));
        } else {
            // Resize the circle around the same center via its known origin.
            let center = via.position;
            let path = Path::circle(pos(*via.diameter + plane.min_clearance_to_copper() * 2)?)
                .translated(center);
            removed.push(ch::path_to_clipper(&path, tolerance()));
        }
    }
    for polygon in &data.polygons {
        if polygon.layer != plane.layer() {
            continue;
        }
        let same_net = plane.net().is_some() && plane.net() == polygon.net;
        let target = if same_net {
            &mut connected
        } else {
            &mut removed
        };
        if polygon.filled {
            let mut paths = vec![ch::path_to_clipper(&polygon.path, tolerance())];
            if !same_net {
                offset(&mut paths, *plane.min_clearance_to_copper())?;
            }
            target.extend(paths);
        }
        if !polygon.filled || polygon.width.to_nm() > 0 {
            let width = if same_net {
                *polygon.width
            } else {
                polygon.width + plane.min_clearance_to_copper() * 2
            };
            target.extend(convert(
                &polygon
                    .path
                    .to_outline_strokes(pos(width.max(Length::new(1)))?),
            ));
        }
    }
    let mut thermal = ClipperPaths::new();
    let mut shrunk_thermal = ClipperPaths::new();
    let mut thermal_clearance = ClipperPaths::new();
    for pad in &data.pads {
        let same_net = plane.net().is_some() && pad.net == plane.net();
        let Some(geometries) = pad.geometries.get(&plane.layer()) else {
            continue;
        };
        for geometry in geometries {
            if same_net {
                connected.extend(convert(&pad.transform.map(&geometry.to_outlines()?)));
            }
            if same_net && plane.connect_style() == PlaneConnectStyle::Solid {
                continue;
            }
            let clearance = if same_net {
                *plane.thermal_gap()
            } else {
                *plane.min_clearance_to_copper()
            }
            .max(*pad.clearance);
            let mut cutout = convert(
                &pad.transform
                    .map(&geometry.with_offset(clearance).to_outlines()?),
            );
            if same_net
                && plane.connect_style() == PlaneConnectStyle::ThermalRelief
                && ch::any_points_of_paths_inside(&cutout, &plane_outline)
            {
                let width =
                    pos((*plane.thermal_spoke_width()).max(*plane.min_width()) + Length::new(10))?;
                for (origin, angle) in thermal_spokes(geometry) {
                    // Upstream rotates/translates spokes without mirroring.
                    let a = origin.rotated(pad.transform.rotation, Point::ORIGIN)
                        + pad.transform.position;
                    let b = (Point::new(Length::new(100_000_000), Length::ZERO)
                        .rotated(angle, Point::ORIGIN)
                        + origin)
                        .rotated(pad.transform.rotation, Point::ORIGIN)
                        + pad.transform.position;
                    ch::subtract(
                        &mut cutout,
                        &convert(&[Path::obround_line(a, b, width)]),
                        Fill::EvenOdd,
                        Fill::NonZero,
                    )?;
                }
                let mut copper = convert(&pad.transform.map(&geometry.to_outlines()?));
                if copper.len() > 1 {
                    ch::unite(&mut copper, Fill::NonZero)?;
                }
                thermal.extend(copper);
                let mut clearance_paths = convert(
                    &pad.transform.map(
                        &geometry
                            .with_offset(
                                clearance + plane.min_width() - *tolerance() - Length::new(10),
                            )
                            .to_outlines()?,
                    ),
                );
                if clearance_paths.len() > 1 {
                    ch::unite(&mut clearance_paths, Fill::NonZero)?;
                }
                thermal_clearance.extend(clearance_paths);
                shrunk_thermal.extend(convert(
                    &pad.transform.map(
                        &geometry
                            .with_offset(-*tolerance() - Length::new(10))
                            .to_outlines()?,
                    ),
                ));
            }
            removed.extend(cutout);
            if !same_net {
                for hole in geometry.holes() {
                    removed.extend(convert(
                        &pad.transform.map(
                            &hole
                                .path()
                                .to_outline_strokes(pos(hole.diameter() + clearance * 2)?),
                        ),
                    ));
                }
            }
        }
    }
    ch::subtract(&mut fragments, &removed, Fill::EvenOdd, Fill::NonZero)?;
    let min_width_offset = (*plane.min_width() / 2) - Length::new(1);
    if min_width_offset > Length::ZERO {
        offset(&mut fragments, -min_width_offset)?;
        offset(&mut fragments, min_width_offset)?;
    }
    let tree = ch::subtract_to_tree(
        &fragments,
        &shrunk_thermal,
        Fill::EvenOdd,
        Fill::NonZero,
        true,
    )?;
    fragments = ch::flatten_tree(tree.root())?;
    if thermal.len() != thermal_clearance.len() {
        return Err(Error::ThermalPadsInconsistent);
    }
    fragments.retain(|fragment| {
        let touching: Vec<_> = thermal
            .iter()
            .enumerate()
            .filter(|(_, area)| ch::any_points_inside(fragment, area))
            .map(|(i, _)| i)
            .take(2)
            .collect();
        !(touching.len() == 1 && ch::all_points_inside(fragment, &thermal_clearance[touching[0]]))
    });
    ch::intersect(&mut thermal, &full_area, Fill::NonZero, Fill::EvenOdd)?;
    let tree = ch::unite_with_to_tree(&fragments, &thermal, Fill::EvenOdd, Fill::NonZero)?;
    fragments = ch::flatten_tree(tree.root())?;
    if plane.net().is_some() && !plane.keep_islands() {
        let mut kept = Vec::new();
        for fragment in fragments {
            let mut intersections = vec![fragment.clone()];
            ch::intersect(&mut intersections, &connected, Fill::NonZero, Fill::NonZero)?;
            if !intersections.is_empty() {
                kept.push(fragment);
            }
        }
        fragments = kept;
    }
    for path in &mut fragments {
        if let Some((index, _)) = path.iter().enumerate().min_by_key(|(_, p)| (p.x, p.y)) {
            path.rotate_left(index);
        }
    }
    fragments.sort_by_key(|p| p.first().map(|p| (p.x, p.y)));
    Ok(ch::paths_from_clipper(&fragments))
}

fn thermal_spokes(geometry: &PadGeometry) -> Vec<(Point, Angle)> {
    let vertices = geometry.path().vertices();
    let circular = matches!(
        geometry.shape(),
        PadGeometryShape::RoundedRect | PadGeometryShape::RoundedOctagon
    ) && geometry.width() == geometry.height()
        && *geometry.corner_radius() >= geometry.width() / 2;
    let circular_stroke = geometry.shape() == PadGeometryShape::Stroke && vertices.len() == 1;
    if circular || circular_stroke {
        let center = if circular_stroke {
            vertices[0].pos
        } else {
            Point::ORIGIN
        };
        return [45, 135, 225, 315]
            .map(|a| (center, Angle::from_deg(a as f64).expect("constant angle")))
            .into();
    }
    let obround = geometry.shape() == PadGeometryShape::Stroke
        && vertices.len() == 2
        && vertices[0].angle == Angle::DEG0;
    if geometry.shape() != PadGeometryShape::Stroke || obround {
        let (center, angle) = if obround {
            (
                (vertices[0].pos + vertices[1].pos) / 2,
                toolbox::angle_between_points(vertices[0].pos, vertices[1].pos),
            )
        } else {
            (Point::ORIGIN, Angle::DEG0)
        };
        return [Angle::DEG0, Angle::DEG90, Angle::DEG180, Angle::DEG270]
            .map(|a| (center, angle + a))
            .into();
    }
    if vertices.len() > 1 {
        let first = &vertices[0];
        let last = &vertices[vertices.len() - 1];
        let angle = toolbox::angle_between_points(first.pos, last.pos);
        let a = Angle::from_deg(135.0).expect("constant angle");
        let b = Angle::from_deg(45.0).expect("constant angle");
        return vec![
            (first.pos, angle + a),
            (first.pos, angle - a),
            (last.pos, angle + b),
            (last.pos, angle - b),
        ];
    }
    Vec::new()
}
