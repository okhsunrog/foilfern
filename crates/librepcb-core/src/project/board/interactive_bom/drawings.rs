//! Port of BoardInteractiveHtmlBomGenerator::generate()'s board drawings.
use super::{BoardInteractiveBomGenerator, point, side};
use crate::{
    geometry::Path,
    project::{
        Result,
        board::{BoardDevice, BoardStrokeTextData, ViaView},
    },
    types::{Layer, UnsignedLength},
};
use interactive_html_bom as ib;

impl BoardInteractiveBomGenerator<'_> {
    pub(super) fn add_drawing(
        &self,
        data: &mut ib::InteractiveHtmlBom,
        layer: Layer,
        path: &Path,
        width: UnsignedLength,
        filled: bool,
    ) {
        let mut drawing_layer = match layer {
            Layer::BOARD_OUTLINES | Layer::BOARD_CUTOUTS | Layer::BOARD_PLATED_CUTOUTS => {
                Some(ib::DrawingLayer::Edge)
            }
            Layer::TOP_DOCUMENTATION => Some(ib::DrawingLayer::FabricationFront),
            Layer::BOT_DOCUMENTATION => Some(ib::DrawingLayer::FabricationBack),
            _ => None,
        };
        if self.board.settings().silkscreen_layers_top.contains(&layer) {
            drawing_layer = Some(ib::DrawingLayer::SilkscreenFront);
        }
        if self.board.settings().silkscreen_layers_bot.contains(&layer) {
            drawing_layer = Some(ib::DrawingLayer::SilkscreenBack);
        }
        if let Some(drawing_layer) = drawing_layer {
            let kind = match layer {
                Layer::TOP_NAMES | Layer::BOT_NAMES => ib::DrawingKind::ReferenceText,
                Layer::TOP_VALUES | Layer::BOT_VALUES => ib::DrawingKind::ValueText,
                _ => ib::DrawingKind::Polygon,
            };
            let width = if drawing_layer == ib::DrawingLayer::Edge {
                0.1
            } else {
                width.to_mm() as f32
            };
            data.drawings.push(ib::Drawing::new(
                kind,
                drawing_layer,
                &path.to_svg_path_mm(),
                width,
                filled,
            ));
        } else if let Some(layer) = side(layer)
            && width.to_nm() > 0
        {
            // Source uses straight segments here, including for copper arcs.
            for pair in path.vertices().windows(2) {
                data.tracks.push(ib::Track::new(
                    layer.clone(),
                    point(pair[0].pos),
                    point(pair[1].pos),
                    width.to_mm() as f32,
                    None,
                ));
            }
        }
    }
    pub(super) fn add_text(
        &self,
        data: &mut ib::InteractiveHtmlBom,
        device: Option<&BoardDevice>,
        text: &BoardStrokeTextData,
    ) -> Result<()> {
        for path in text.paths(self.project, self.board, device)? {
            self.add_drawing(
                data,
                text.layer(),
                &text.transform().map(&path),
                text.stroke_width(),
                false,
            );
        }
        Ok(())
    }
    pub(super) fn board_drawings(&self, data: &mut ib::InteractiveHtmlBom) -> Result<()> {
        for polygon in self.board.polygons().values() {
            self.add_drawing(
                data,
                polygon.layer(),
                polygon.path(),
                polygon.line_width(),
                polygon.is_filled(),
            );
        }
        for text in self.board.stroke_texts().values() {
            self.add_text(data, None, text)?;
        }
        for hole in self.board.holes().values() {
            for outline in hole.path().to_outline_strokes(hole.diameter()) {
                self.add_drawing(
                    data,
                    Layer::BOARD_CUTOUTS,
                    &outline,
                    UnsignedLength::ZERO,
                    false,
                );
            }
        }
        for segment in self.board.net_segments().values() {
            for pad in segment.pads().values() {
                for hole in pad.pad().holes().iter() {
                    for outline in hole.path().to_outline_strokes(hole.diameter()) {
                        let transform = crate::utils::transform::Transform::new(
                            pad.pad().position(),
                            pad.pad().rotation(),
                            false,
                        );
                        self.add_drawing(
                            data,
                            Layer::BOARD_CUTOUTS,
                            &transform.map(&outline),
                            UnsignedLength::ZERO,
                            false,
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn copper(&self, data: &mut ib::InteractiveHtmlBom) -> Result<()> {
        for segment in self.board.net_segments().values() {
            let net = self.net(segment.net());
            for trace in segment.traces().values() {
                if let Some(layer) = side(trace.layer()) {
                    let position = |anchor| {
                        self.board
                            .anchor_position(
                                segment,
                                anchor,
                                self.project.library(),
                                self.project.circuit(),
                            )
                            .ok_or(crate::project::Error::InexistentTraceAnchor {
                                anchor,
                                device_missing: false,
                            })
                    };
                    data.tracks.push(ib::Track::new(
                        layer,
                        point(position(trace.p1())?),
                        point(position(trace.p2())?),
                        trace.width().to_mm() as f32,
                        net,
                    ));
                }
            }
            for via in segment.vias().values() {
                let view = ViaView::new(via, self.board, segment.net(), self.project.circuit())?;
                let layers = [Layer::TOP_COPPER, Layer::BOT_COPPER]
                    .into_iter()
                    .filter(|layer| via.is_on_layer(*layer))
                    .filter_map(side)
                    .collect::<Vec<_>>();
                data.vias.push(ib::Via::new(
                    &layers,
                    point(via.position()),
                    view.size().to_mm() as f32,
                    view.drill_diameter().to_mm() as f32,
                    net,
                ));
            }
        }
        for (id, plane) in self.board.planes() {
            if let Some(layer) = side(plane.layer()) {
                for fragment in self
                    .board
                    .derived()
                    .plane_fragments()
                    .get(id)
                    .into_iter()
                    .flatten()
                {
                    let mut fragment = fragment.clone();
                    fragment.close();
                    data.zones.push(ib::Zone::new(
                        layer.clone(),
                        &fragment.to_svg_path_mm(),
                        self.net(plane.net()),
                    ));
                }
            }
        }
        Ok(())
    }
}
