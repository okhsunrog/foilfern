//! Port of libs/librepcb/core/geometry/padgeometry.{h,cpp}.
//!
//! Not ported: the `toQPainterPathPx()` family (UI specific). Their
//! geometry is available as paths: [`PadGeometry::to_outlines()`] (filled
//! area) and [`PadGeometry::to_hole_outlines()`] (holes).

use clipper::PolyFillType;

use super::{NonEmptyPath, PadHoleList, Path};
use crate::types::{Angle, Length, PositiveLength, Ratio, UnsignedLength, UnsignedLimitedRatio};
use crate::utils::clipper_helpers;

/// Shape of a [`PadGeometry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PadGeometryShape {
    /// Rectangle with rounded corners.
    RoundedRect,
    /// Octagon with rounded corners.
    RoundedOctagon,
    /// Stroked path.
    Stroke,
    /// Custom outline.
    Custom,
}

/// The geometry of a pad on one layer: a shape with an offset (e.g. for
/// stop mask openings) and the holes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadGeometry {
    shape: PadGeometryShape,
    base_width: Length,
    base_height: Length,
    radius: UnsignedLimitedRatio,
    path: Path,
    offset: Length,
    holes: PadHoleList,
}

impl PadGeometry {
    fn new(
        shape: PadGeometryShape,
        base_width: Length,
        base_height: Length,
        radius: UnsignedLimitedRatio,
        path: Path,
        offset: Length,
        holes: PadHoleList,
    ) -> Self {
        Self {
            shape,
            base_width,
            base_height,
            radius,
            path,
            offset,
            holes,
        }
    }

    /// Maximum arc tolerance used when flattening pad outlines.
    fn max_arc_tolerance() -> PositiveLength {
        PositiveLength::new(Length::new(5000)).expect("constant is positive")
    }

    fn zero_ratio() -> UnsignedLimitedRatio {
        UnsignedLimitedRatio::new(Ratio::from_percent(0)).expect("0% is within 0..1")
    }

    /// Returns the shape.
    pub fn shape(&self) -> PadGeometryShape {
        self.shape
    }

    /// Returns the width including the offset (may be <= 0).
    pub fn width(&self) -> Length {
        self.base_width + (self.offset * 2)
    }

    /// Returns the height including the offset (may be <= 0).
    pub fn height(&self) -> Length {
        self.base_height + (self.offset * 2)
    }

    /// Returns the corner radius including the offset.
    pub fn corner_radius(&self) -> UnsignedLength {
        let size = self.base_width.min(self.base_height) / 2;
        // Radius computed from lengths in range is always in range.
        let radius = Length::from_mm(size.to_mm() * self.radius.to_normalized())
            .unwrap_or_default()
            .min(size)
            .max(Length::ZERO);
        UnsignedLength::new((radius + self.offset).max(Length::ZERO)).unwrap_or_default()
    }

    /// Returns the path (stroke path or custom outline).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the holes.
    pub fn holes(&self) -> &PadHoleList {
        &self.holes
    }

    /// Returns the outlines of the pad area (without holes).
    pub fn to_outlines(&self) -> Result<Vec<Path>, clipper_helpers::Error> {
        let w = self.width();
        let h = self.height();
        let r = self.corner_radius();
        let mut result = Vec::new();
        match self.shape {
            PadGeometryShape::RoundedRect => {
                if let (Ok(w), Ok(h)) = (PositiveLength::new(w), PositiveLength::new(h)) {
                    result.push(Path::centered_rect(w, h, r));
                }
            }
            PadGeometryShape::RoundedOctagon => {
                if let (Ok(w), Ok(h)) = (PositiveLength::new(w), PositiveLength::new(h)) {
                    result.push(Path::octagon(w, h, r));
                }
            }
            PadGeometryShape::Stroke => {
                if let Ok(w) = PositiveLength::new(w) {
                    result = self.path.to_outline_strokes(w);
                    // Unite all outlines to get only a single, non-intersecting
                    // outline. Not needed if there's only one straight line
                    // segment since it cannot be self-intersecting.
                    let first_is_arc = self
                        .path
                        .vertices()
                        .first()
                        .is_some_and(|v| v.angle != Angle::DEG0);
                    if (result.len() > 1) || ((result.len() == 1) && first_is_arc) {
                        let paths =
                            clipper_helpers::paths_to_clipper(&result, Self::max_arc_tolerance());
                        let tree = clipper_helpers::unite_to_tree(&paths, PolyFillType::NonZero)?;
                        let paths = clipper_helpers::flatten_tree(tree.root())?;
                        result = clipper_helpers::paths_from_clipper(&paths);
                    }
                }
            }
            PadGeometryShape::Custom => {
                let mut outline = self.path.clone();
                outline.close();
                if outline.vertices().len() >= 3 {
                    // Note: If the offset is zero, the offset operation sounds
                    // superfluous. However, this operation ensures that invalid
                    // outlines (e.g. overlaps or intersections) will be cleaned
                    // before any further processing of the pad shape (e.g.
                    // Gerber export).
                    let paths = vec![clipper_helpers::path_to_clipper(
                        &outline,
                        Self::max_arc_tolerance(),
                    )];
                    let tree = clipper_helpers::offset_to_tree(
                        &paths,
                        self.offset,
                        Self::max_arc_tolerance(),
                    )?;
                    let paths = clipper_helpers::flatten_tree(tree.root())?;
                    result = clipper_helpers::paths_from_clipper(&paths);
                }
            }
        }
        Ok(result)
    }

    /// Returns the outlines of the holes, one closed path per hole segment
    /// (the geometry of upstream `toHolesQPainterPathPx()`; the paths
    /// overlap for multi-segment slots, so they have to be united with the
    /// non-zero fill rule).
    pub fn to_hole_outlines(&self) -> Vec<Path> {
        self.holes
            .iter()
            .flat_map(|hole| hole.path().to_outline_strokes(hole.diameter()))
            .collect()
    }

    /// Returns a copy with an additional offset.
    pub fn with_offset(&self, offset: Length) -> Self {
        Self {
            offset: self.offset + offset,
            ..self.clone()
        }
    }

    /// Returns a copy without holes.
    pub fn without_holes(&self) -> Self {
        Self {
            holes: PadHoleList::new(),
            ..self.clone()
        }
    }

    /// Creates a rounded rectangle geometry.
    pub fn rounded_rect(
        width: PositiveLength,
        height: PositiveLength,
        radius: UnsignedLimitedRatio,
        holes: PadHoleList,
    ) -> Self {
        Self::new(
            PadGeometryShape::RoundedRect,
            *width,
            *height,
            radius,
            Path::default(),
            Length::ZERO,
            holes,
        )
    }

    /// Creates a rounded octagon geometry.
    pub fn rounded_octagon(
        width: PositiveLength,
        height: PositiveLength,
        radius: UnsignedLimitedRatio,
        holes: PadHoleList,
    ) -> Self {
        Self::new(
            PadGeometryShape::RoundedOctagon,
            *width,
            *height,
            radius,
            Path::default(),
            Length::ZERO,
            holes,
        )
    }

    /// Creates a stroked path geometry.
    pub fn stroke(diameter: PositiveLength, path: NonEmptyPath, holes: PadHoleList) -> Self {
        Self::new(
            PadGeometryShape::Stroke,
            *diameter,
            Length::ZERO,
            Self::zero_ratio(),
            path.into_inner(),
            Length::ZERO,
            holes,
        )
    }

    /// Creates a custom outline geometry.
    pub fn custom(outline: Path, holes: PadHoleList) -> Self {
        Self::new(
            PadGeometryShape::Custom,
            Length::ZERO,
            Length::ZERO,
            Self::zero_ratio(),
            outline,
            Length::ZERO,
            holes,
        )
    }

    /// Returns whether `path` is a valid custom pad outline (non-zero area).
    pub fn is_valid_custom_outline(path: &Path) -> bool {
        let mut outline = path.clone();
        outline.close();
        let p = clipper_helpers::path_to_clipper(&outline, Self::max_arc_tolerance());
        clipper::area(&p).abs() > 1.0
    }
}
