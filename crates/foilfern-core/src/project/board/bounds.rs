//! Port of libs/librepcb/core/project/board/board.cpp:
//! Board::calculateBoundingRect(). Uses the existing painter-path Bézier
//! bounds because upstream's outline bounds affect exported JSON bytes.

use super::Board;
use super::device::ResolvedDevice;
use crate::geometry::Path;
use crate::project::{ProjectLibrary, Result};
use crate::types::{Layer, Point};
use crate::utils::painter_path::bounding_rect_px;

impl Board {
    /// Returns opposite corners of the outline bounding rectangle, or
    /// `None` if there are no outlines. Includes device footprint outlines
    /// and circles with placement applied; ignores stroke widths.
    pub fn bounding_rect(&self, library: &ProjectLibrary) -> Result<Option<(Point, Point)>> {
        let mut outlines: Vec<_> = self
            .polygons()
            .values()
            .filter(|p| p.layer() == Layer::BOARD_OUTLINES && !p.path().vertices().is_empty())
            .map(|p| p.path().clone())
            .collect();
        for device in self.devices().values() {
            let footprint = ResolvedDevice::resolve(device, library)?.footprint;
            let transform = device.transform();
            for polygon in footprint.polygons() {
                if polygon.layer() == Layer::BOARD_OUTLINES && !polygon.path().vertices().is_empty()
                {
                    outlines.push(transform.map(polygon.path()));
                }
            }
            for circle in footprint.circles() {
                if circle.layer() == Layer::BOARD_OUTLINES {
                    outlines.push(
                        transform.map(&Path::circle(circle.diameter()).translated(circle.center())),
                    );
                }
            }
        }
        if outlines.is_empty() {
            return Ok(None);
        }
        let bounds = bounding_rect_px(&outlines);
        Ok(Some((
            Point::from_px(bounds.left(), bounds.bottom())?,
            Point::from_px(bounds.right(), bounds.top())?,
        )))
    }
}
