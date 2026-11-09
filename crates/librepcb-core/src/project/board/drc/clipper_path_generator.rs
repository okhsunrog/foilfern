//! Port of libs/librepcb/core/project/board/drc/boardclipperpathgenerator.{h,cpp}.
//!
//! Builds Clipper paths (united areas) of the copper and stop mask openings
//! of the DRC input data. The polygon operations are exactly those of
//! upstream (same fill types and order), so the resulting paths are
//! identical.

use clipper::PolyFillType::{EvenOdd, NonZero};
use clipper::{JoinType, PolyFillType};

use super::data::{BoardDrcData, DrcCircle, DrcPad, DrcStrokeText, DrcTrace, DrcVia};
use super::error::Result;
use crate::geometry::{NonEmptyPath, Path, Via};
use crate::types::{Layer, Length, PositiveLength, UnsignedLength, Uuid};
use crate::utils::clipper_helpers::{self, ClipperPath, ClipperPaths};
use crate::utils::transform::Transform;

/// Accumulates the areas of board objects (upstream
/// `BoardClipperPathGenerator`).
#[derive(Debug, Clone)]
pub struct BoardClipperPathGenerator {
    max_arc_tolerance: PositiveLength,
    paths: ClipperPaths,
}

impl BoardClipperPathGenerator {
    /// Creates an empty generator.
    pub fn new(max_arc_tolerance: PositiveLength) -> Self {
        Self {
            max_arc_tolerance,
            paths: ClipperPaths::new(),
        }
    }

    /// Returns the accumulated paths.
    pub fn paths(&self) -> &ClipperPaths {
        &self.paths
    }

    /// Returns the accumulated paths and clears the generator (upstream
    /// `takePathsTo()`).
    pub fn take_paths(&mut self) -> ClipperPaths {
        std::mem::take(&mut self.paths)
    }

    fn unite(&mut self, paths: &[ClipperPath], clip_fill: PolyFillType) -> Result<()> {
        clipper_helpers::unite_with(&mut self.paths, paths, EvenOdd, clip_fill)?;
        Ok(())
    }

    fn convert(&self, paths: &[Path]) -> ClipperPaths {
        clipper_helpers::paths_to_clipper(paths, self.max_arc_tolerance)
    }

    /// Adds all copper objects on `layer`; if `net_signals` is not empty,
    /// only those of the given nets (`None` = objects without net).
    pub fn add_copper(
        &mut self,
        data: &BoardDrcData,
        layer: Layer,
        net_signals: &[Option<Uuid>],
        ignore_planes: bool,
    ) -> Result<()> {
        let net_ok = |net: Option<Uuid>| net_signals.is_empty() || net_signals.contains(&net);

        // Board polygons.
        for polygon in &data.polygons {
            if (polygon.layer == layer) && net_ok(None) {
                self.add_polygon(
                    &polygon.path,
                    polygon.line_width,
                    polygon.filled,
                    Length::ZERO,
                )?;
            }
        }

        // Stroke texts.
        for st in &data.stroke_texts {
            if (st.layer == layer) && net_ok(None) {
                self.add_stroke_text(st, Length::ZERO)?;
            }
        }

        // Planes.
        if !ignore_planes {
            for plane in &data.planes {
                if (plane.layer == layer) && net_ok(plane.net) {
                    self.add_plane(&plane.fragments)?;
                }
            }
        }

        // Devices.
        for dev in data.devices.values() {
            let transform = dev.transform();

            // Polygons.
            for polygon in &dev.polygons {
                if (transform.map(&polygon.layer) == layer) && net_ok(None) {
                    self.add_polygon(
                        &transform.map(&polygon.path),
                        polygon.line_width,
                        polygon.filled,
                        Length::ZERO,
                    )?;
                }
            }

            // Circles.
            for circle in &dev.circles {
                if (transform.map(&circle.layer) == layer) && net_ok(None) {
                    self.add_circle(circle, &transform, Length::ZERO)?;
                }
            }

            // Stroke texts.
            for st in &dev.stroke_texts {
                // Do *not* mirror layer since it is independent of the device!
                if (st.layer == layer) && net_ok(None) {
                    self.add_stroke_text(st, Length::ZERO)?;
                }
            }

            // Pads.
            for pad in dev.pads.values() {
                if has_geometry(pad, layer) && net_ok(pad.net) {
                    self.add_pad(pad, layer, Length::ZERO)?;
                }
            }
        }

        // Net segment items.
        for ns in data.segments.values() {
            if net_ok(ns.net) {
                // Pads.
                for pad in ns.pads.values() {
                    if has_geometry(pad, layer) {
                        self.add_pad(pad, layer, Length::ZERO)?;
                    }
                }

                // Vias.
                for via in ns.vias.values() {
                    if Via::is_on_layer_between(layer, via.start_layer, via.end_layer) {
                        self.add_via(via, Length::ZERO)?;
                    }
                }

                // Net lines.
                for trace in &ns.traces {
                    if trace.layer == layer {
                        self.add_trace(trace, Length::ZERO)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Adds the stop mask openings on the stop mask `layer`, expanded by
    /// `offset`.
    pub fn add_stop_mask_openings(
        &mut self,
        data: &BoardDrcData,
        layer: Layer,
        offset: Length,
    ) -> Result<()> {
        // Board polygons.
        for polygon in &data.polygons {
            if polygon.layer == layer {
                self.add_polygon(&polygon.path, polygon.line_width, polygon.filled, offset)?;
            }
        }

        // Stroke texts.
        for st in &data.stroke_texts {
            if st.layer == layer {
                self.add_stroke_text(st, offset)?;
            }
        }

        // Holes.
        for hole in &data.holes {
            if let Some(mask_offset) = hole.stop_mask_offset {
                let mask_dia = PositiveLength::new(hole.diameter + (mask_offset * 2))?;
                self.add_hole(mask_dia, &hole.path, &Transform::default(), offset)?;
            }
        }

        // Devices.
        for dev in data.devices.values() {
            let transform = dev.transform();

            // Polygons.
            for polygon in &dev.polygons {
                if transform.map(&polygon.layer) == layer {
                    self.add_polygon(
                        &transform.map(&polygon.path),
                        polygon.line_width,
                        polygon.filled,
                        offset,
                    )?;
                }
            }

            // Circles.
            for circle in &dev.circles {
                if transform.map(&circle.layer) == layer {
                    self.add_circle(circle, &transform, offset)?;
                }
            }

            // Stroke texts.
            for st in &dev.stroke_texts {
                // Do *not* mirror layer since it is independent of the device!
                if st.layer == layer {
                    self.add_stroke_text(st, offset)?;
                }
            }

            // Holes.
            for hole in &dev.holes {
                if let Some(mask_offset) = hole.stop_mask_offset {
                    let mask_dia = PositiveLength::new(hole.diameter + (mask_offset * 2))?;
                    self.add_hole(mask_dia, &hole.path, &transform, offset)?;
                }
            }

            // Pads.
            for pad in dev.pads.values() {
                self.add_pad_outlines(pad, layer, offset)?;
            }
        }

        // Net segment items.
        for ns in data.segments.values() {
            // Pads.
            for pad in ns.pads.values() {
                self.add_pad_outlines(pad, layer, offset)?;
            }

            // Vias.
            for via in ns.vias.values() {
                let stop_mask_dia = if layer.is_top() {
                    via.stop_mask_diameter_top
                } else {
                    via.stop_mask_diameter_bot
                };
                if let Some(dia) = stop_mask_dia {
                    self.add_hole(
                        dia,
                        &NonEmptyPath::from_point(via.position),
                        &Transform::default(),
                        offset,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Adds the (offset) outlines of the pad geometries on `layer`, without
    /// the holes.
    fn add_pad_outlines(&mut self, pad: &DrcPad, layer: Layer, offset: Length) -> Result<()> {
        let pad_transform = Transform::new(pad.position, pad.rotation, pad.mirror);
        for geometry in pad.geometries.get(&layer).into_iter().flatten() {
            let geometry = if offset != Length::ZERO {
                geometry.with_offset(offset)
            } else {
                geometry.clone()
            };
            let outlines = pad_transform.map(&geometry.to_outlines()?);
            let paths = self.convert(&outlines);
            self.unite(&paths, NonZero)?;
        }
        Ok(())
    }

    /// Adds the area of a via, expanded by `offset`.
    pub fn add_via(&mut self, via: &DrcVia, offset: Length) -> Result<()> {
        let size = via.size + (offset * 2);
        if let Ok(size) = PositiveLength::new(size) {
            let outline = Path::circle(size).translated(via.position);
            let paths = vec![clipper_helpers::path_to_clipper(
                &outline,
                self.max_arc_tolerance,
            )];
            self.unite(&paths, EvenOdd)?;
        }
        Ok(())
    }

    /// Adds the area of a trace, expanded by `offset`.
    pub fn add_trace(&mut self, trace: &DrcTrace, offset: Length) -> Result<()> {
        let width = trace.width + (offset * 2);
        if let Ok(width) = PositiveLength::new(width) {
            let outline = Path::obround_line(trace.p1, trace.p2, width);
            let paths = vec![clipper_helpers::path_to_clipper(
                &outline,
                self.max_arc_tolerance,
            )];
            self.unite(&paths, EvenOdd)?;
        }
        Ok(())
    }

    /// Adds the fragments of a plane.
    pub fn add_plane(&mut self, fragments: &[Path]) -> Result<()> {
        for p in fragments {
            let paths = vec![clipper_helpers::path_to_clipper(p, self.max_arc_tolerance)];
            self.unite(&paths, EvenOdd)?;
        }
        Ok(())
    }

    /// Adds the area of a polygon (outline stroke and, if filled and
    /// closed, its area), expanded by `offset`.
    pub fn add_polygon(
        &mut self,
        path: &Path,
        line_width: UnsignedLength,
        filled: bool,
        offset: Length,
    ) -> Result<()> {
        // Outline.
        let total_width = line_width + (offset * 2);
        if let (true, Ok(width)) = (*line_width > Length::ZERO, PositiveLength::new(total_width)) {
            let paths = self.convert(&path.to_outline_strokes(width));
            self.unite(&paths, NonZero)?;
        }

        // Area (only fill closed paths, for consistency with the appearance
        // in the board editor and Gerber output).
        if filled && path.is_closed() {
            let mut paths = vec![clipper_helpers::path_to_clipper(
                path,
                self.max_arc_tolerance,
            )];
            if offset != Length::ZERO {
                clipper_helpers::offset(
                    &mut paths,
                    offset,
                    self.max_arc_tolerance,
                    JoinType::Round,
                )?;
            }
            self.unite(&paths, EvenOdd)?;
        }
        Ok(())
    }

    /// Adds the area of a footprint circle (placed with `transform`),
    /// expanded by `offset`.
    pub fn add_circle(
        &mut self,
        circle: &DrcCircle,
        transform: &Transform,
        offset: Length,
    ) -> Result<()> {
        let diameter = PositiveLength::new((circle.diameter + (offset * 2)).max(Length::new(1)))?;
        let path = Path::circle(diameter).translated(transform.map(&circle.center));

        // Outline.
        if let Ok(width) = PositiveLength::new(*circle.line_width) {
            let paths = self.convert(&path.to_outline_strokes(width));
            self.unite(&paths, NonZero)?;
        }

        // Area.
        if circle.filled {
            let paths = vec![clipper_helpers::path_to_clipper(
                &path,
                self.max_arc_tolerance,
            )];
            self.unite(&paths, EvenOdd)?;
        }
        Ok(())
    }

    /// Adds the strokes of a stroke text, expanded by `offset`.
    pub fn add_stroke_text(&mut self, st: &DrcStrokeText, offset: Length) -> Result<()> {
        let width = PositiveLength::new((st.stroke_width + (offset * 2)).max(Length::new(1)))?;
        let transform = Transform::new(st.position, st.rotation, st.mirror);
        for path in transform.map(&st.paths) {
            let paths = self.convert(&path.to_outline_strokes(width));
            self.unite(&paths, NonZero)?;
        }
        Ok(())
    }

    /// Adds the area of a hole (placed with `transform`), expanded by
    /// `offset`.
    pub fn add_hole(
        &mut self,
        diameter: PositiveLength,
        path: &NonEmptyPath,
        transform: &Transform,
        offset: Length,
    ) -> Result<()> {
        let width = PositiveLength::new((diameter + offset + offset).max(Length::new(1)))?;
        let outlines = transform.map(path.get()).to_outline_strokes(width);
        let paths = self.convert(&outlines);
        self.unite(&paths, NonZero)?;
        Ok(())
    }

    /// Adds the copper area of a pad on `layer`, expanded by `offset`.
    pub fn add_pad(&mut self, pad: &DrcPad, layer: Layer, offset: Length) -> Result<()> {
        let transform = Transform::new(pad.position, pad.rotation, pad.mirror);
        for geometry in pad.geometries.get(&layer).into_iter().flatten() {
            let geometry = if offset != Length::ZERO {
                geometry.with_offset(offset)
            } else {
                geometry.clone()
            };
            let outlines = transform.map(&geometry.to_outlines()?);
            let paths = self.convert(&outlines);
            self.unite(&paths, NonZero)?;

            // Also add each hole to ensure correct copper areas even if
            // the pad outline is too small or invalid.
            for hole in geometry.holes().iter() {
                let strokes = transform.map(&hole.path().to_outline_strokes(hole.diameter()));
                let paths = self.convert(&strokes);
                self.unite(&paths, NonZero)?;
            }
        }
        Ok(())
    }
}

/// Whether the pad has any geometry on `layer`.
pub(super) fn has_geometry(pad: &DrcPad, layer: Layer) -> bool {
    pad.geometries.get(&layer).is_some_and(|g| !g.is_empty())
}
