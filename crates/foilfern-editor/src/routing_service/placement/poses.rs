//! Deterministic sampling and ordering of authorized footprint centers.

use std::collections::BTreeMap;

use foilfern_core::project::board::Board;
use foilfern_core::project::{ComponentInstanceId, Project};
use foilfern_core::types::{Angle, Length, Point, PositiveLength};
use foilfern_core::utils::transform::Transform;

use super::{PlacementTarget, PlacementTrace, invalid};
use crate::Result;
use crate::commands::RoutingBounds;

pub(super) const MAX_TARGET_POSES: usize = 64;

#[derive(Debug)]
pub enum PlacementSites {
    Explicit(Vec<Point>),
    Grid {
        bounds: RoutingBounds,
        pitch: PositiveLength,
    },
}

impl PlacementSites {
    fn positions(&self) -> Result<Vec<Point>> {
        match self {
            Self::Explicit(positions) => Ok(positions.clone()),
            Self::Grid { bounds, pitch } => {
                bounds.region()?;
                let pitch = i128::from(pitch.to_nm());
                let count = |min: Length, max: Length| {
                    (i128::from(max.to_nm()) - i128::from(min.to_nm())) / pitch + 1
                };
                let columns = count(bounds.min.x, bounds.max.x);
                let rows = count(bounds.min.y, bounds.max.y);
                if columns > MAX_TARGET_POSES as i128
                    || rows > MAX_TARGET_POSES as i128
                    || columns * rows > MAX_TARGET_POSES as i128
                {
                    return Err(invalid(
                        "The region grid exceeds 64 centers; increase pitch_mm or reduce the region.",
                    ));
                }
                let coordinate = |min: Length, step| {
                    // Range checked by deriving the count from max.
                    Length::new((i128::from(min.to_nm()) + step * pitch) as i64)
                };
                Ok((0..rows)
                    .flat_map(|row| {
                        (0..columns).map(move |column| {
                            Point::new(
                                coordinate(bounds.min.x, column),
                                coordinate(bounds.min.y, row),
                            )
                        })
                    })
                    .collect())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Pose {
    pub component: ComponentInstanceId,
    pub position: Point,
    pub rotation: Angle,
}

pub(super) fn poses(targets: &[PlacementTarget]) -> Result<Vec<Vec<Pose>>> {
    let mut joint = vec![Vec::new()];
    for target in targets {
        let mut expanded = Vec::new();
        let positions = target.sites.positions()?;
        if positions.is_empty()
            || target.rotations.is_empty()
            || positions.len().saturating_mul(target.rotations.len()) > MAX_TARGET_POSES
        {
            return Err(invalid(
                "Supply 1–64 center/rotation combinations per target.",
            ));
        }
        let unique_positions: std::collections::HashSet<_> = positions.iter().collect();
        let unique_angles: std::collections::HashSet<_> = target
            .rotations
            .iter()
            .map(|angle| angle.mapped_to_180deg())
            .collect();
        if unique_positions.len() != positions.len()
            || unique_angles.len() != target.rotations.len()
        {
            return Err(invalid(
                "Positions and rotations must be distinct within a target.",
            ));
        }
        for prefix in joint {
            for &position in &positions {
                for &rotation in &target.rotations {
                    let mut combined = prefix.clone();
                    combined.push(Pose {
                        component: target.component,
                        position,
                        rotation,
                    });
                    expanded.push(combined);
                }
            }
        }
        joint = expanded;
    }
    Ok(joint)
}

/// Order generated combinations by optimistic pad-to-pad distance. Use the
/// region center as a tie-breaker; neither estimate makes a pose pass or fail.
pub(super) fn rank_generated(
    poses: &mut [Vec<Pose>],
    targets: &[PlacementTarget],
    traces: &[PlacementTrace],
    board: &Board,
    project: &Project,
) -> Result<()> {
    let centers: BTreeMap<_, _> = targets
        .iter()
        .filter_map(|target| {
            let PlacementSites::Grid { bounds, .. } = &target.sites else {
                return None;
            };
            let (x0, y0) = bounds.min.to_mm();
            let (x1, y1) = bounds.max.to_mm();
            Some((target.component, ((x0 + x1) / 2.0, (y0 + y1) / 2.0)))
        })
        .collect();
    if centers.is_empty() {
        return Ok(());
    }
    let endpoint = |(component, id)| -> Result<_> {
        let device = board
            .device(component)
            .ok_or_else(|| crate::Error::not_found("Device", component))?;
        let pad = device
            .pad(&id, project.library(), project.circuit())
            .map_err(crate::Error::from)?
            .ok_or_else(|| crate::Error::not_found("Pad", id))?;
        Ok((
            component,
            pad.position(),
            pad.properties().position(),
            device.mirrored(),
        ))
    };
    let paths = traces
        .iter()
        .filter(|trace| trace.minimize)
        .map(|trace| Ok((endpoint(trace.path.from)?, endpoint(trace.path.to)?)))
        .collect::<Result<Vec<_>>>()?;
    poses.sort_by_cached_key(|combination| {
        let distance = paths
            .iter()
            .map(|&(from, to)| {
                let position = |(component, fixed, local, mirrored): (
                    ComponentInstanceId,
                    Point,
                    Point,
                    bool,
                )| {
                    combination
                        .iter()
                        .find(|pose| pose.component == component)
                        .map_or(fixed, |pose| {
                            Transform::new(pose.position, pose.rotation, mirrored).map(&local)
                        })
                        .to_mm()
                };
                let (x0, y0) = position(from);
                let (x1, y1) = position(to);
                (x1 - x0).hypot(y1 - y0)
            })
            .sum::<f64>();
        let center_distance = combination
            .iter()
            .filter_map(|pose| {
                let &(x0, y0) = centers.get(&pose.component)?;
                let (x1, y1) = pose.position.to_mm();
                Some((x1 - x0).hypot(y1 - y0))
            })
            .sum::<f64>();
        // Nanometer resolution absorbs differences in floating point ties.
        (
            (distance * 1e6).round() as u64,
            (center_distance * 1e6).round() as u64,
        )
    });
    Ok(())
}
