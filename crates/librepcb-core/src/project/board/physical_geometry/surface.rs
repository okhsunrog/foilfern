//! Port of RealisticBoardPainter::getContent()'s boolean material regions.
use super::{BoardPhysicalGeometry, BoardSurfaceGeometry, PhysicalHoleDepth};
use crate::{export::BoardSide, project::Result, types::Layer, utils::clipper_helpers as ch};
use clipper::PolyFillType::{EvenOdd, NonZero};

impl BoardPhysicalGeometry {
    pub(super) fn prepare_surfaces(&mut self) -> Result<()> {
        for (index, side) in [BoardSide::Top, BoardSide::Bottom].into_iter().enumerate() {
            let bottom = side == BoardSide::Bottom;
            let mut holes = ch::paths_to_clipper(&self.cutouts, Self::arc_tolerance());
            let mut copper_holes = Vec::new();
            for hole in &self.holes {
                let visible_blind = match hole.depth {
                    PhysicalHoleDepth::Through => false,
                    PhysicalHoleDepth::Layers { start, .. } if start.is_top() => !bottom,
                    PhysicalHoleDepth::Layers { end, .. } if end.is_bottom() => bottom,
                    PhysicalHoleDepth::Layers { .. } => continue, // buried, invisible on both faces
                };
                let outlines = ch::paths_to_clipper(
                    &hole.path.to_outline_strokes(hole.diameter),
                    Self::arc_tolerance(),
                );
                if visible_blind {
                    copper_holes.extend(outlines);
                } else {
                    holes.extend(outlines);
                }
            }
            ch::unite(&mut holes, NonZero)?;
            let outlines = ch::paths_to_clipper(&self.outlines, Self::arc_tolerance());
            let mut body = outlines.clone();
            ch::subtract(&mut body, &holes, NonZero, NonZero)?;
            let mut copper = body.clone();
            ch::subtract(&mut copper, &copper_holes, EvenOdd, NonZero)?;
            let (cu, mask, paste) = if bottom {
                (
                    Layer::BOT_COPPER,
                    Layer::BOT_STOP_MASK,
                    Layer::BOT_SOLDER_PASTE,
                )
            } else {
                (
                    Layer::TOP_COPPER,
                    Layer::TOP_STOP_MASK,
                    Layer::TOP_SOLDER_PASTE,
                )
            };
            let paths = |layer| {
                ch::paths_to_clipper(
                    self.areas
                        .get(&layer)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    Self::arc_tolerance(),
                )
            };
            ch::intersect(&mut copper, &paths(cu), EvenOdd, NonZero)?;
            let mut mask_openings = paths(mask);
            mask_openings.extend(ch::paths_to_clipper(&self.cutouts, Self::arc_tolerance()));
            let mut resist = outlines;
            ch::subtract(&mut resist, &mask_openings, EvenOdd, NonZero)?;
            let mut silk = Vec::new();
            for layer in &self.silkscreen_layers[index] {
                silk.extend(paths(*layer));
            }
            ch::intersect(&mut silk, &resist, NonZero, EvenOdd)?;
            let mut solder_paste = paths(paste);
            ch::intersect(&mut solder_paste, &body, NonZero, EvenOdd)?;
            self.surfaces[index] = BoardSurfaceGeometry {
                body: ch::paths_from_clipper(&body),
                copper: ch::paths_from_clipper(&copper),
                solder_resist: ch::paths_from_clipper(&resist),
                silkscreen: ch::paths_from_clipper(&silk),
                solder_paste: ch::paths_from_clipper(&solder_paste),
            };
        }
        Ok(())
    }
}
