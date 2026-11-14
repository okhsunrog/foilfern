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
            let mut copper_body = body.clone();
            ch::subtract(&mut copper_body, &copper_holes, EvenOdd, NonZero)?;
            let mask = if bottom {
                Layer::BOT_STOP_MASK
            } else {
                Layer::TOP_STOP_MASK
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
            let mut copper = std::collections::BTreeMap::new();
            for layer in self.areas.keys().copied().filter(|layer| layer.is_copper()) {
                let mut region = copper_body.clone();
                ch::intersect(&mut region, &paths(layer), EvenOdd, NonZero)?;
                if !region.is_empty() {
                    copper.insert(layer, ch::paths_from_clipper(&region));
                }
            }
            let mut mask_openings = paths(mask);
            mask_openings.extend(ch::paths_to_clipper(&self.cutouts, Self::arc_tolerance()));
            let mut resist = outlines;
            ch::subtract(&mut resist, &mask_openings, EvenOdd, NonZero)?;
            let mut silk = Vec::new();
            for layer in &self.silkscreen_layers[index] {
                silk.extend(paths(*layer));
            }
            ch::intersect(&mut silk, &resist, NonZero, EvenOdd)?;
            let mut solder_paste = std::collections::BTreeMap::new();
            for layer in [Layer::TOP_SOLDER_PASTE, Layer::BOT_SOLDER_PASTE] {
                let mut region = paths(layer);
                ch::intersect(&mut region, &body, NonZero, EvenOdd)?;
                if !region.is_empty() {
                    solder_paste.insert(layer, ch::paths_from_clipper(&region));
                }
            }
            self.surfaces[index] = BoardSurfaceGeometry {
                body: ch::paths_from_clipper(&body),
                copper,
                solder_resist: ch::paths_from_clipper(&resist),
                silkscreen: ch::paths_from_clipper(&silk),
                solder_paste,
            };
        }
        Ok(())
    }
}
