//! Focused copper view with the evidence from the builtin access probes.

use librepcb_core::project::{BoardId, Project};
use librepcb_core::types::{Layer, PositiveLength};
use librepcb_editor::commands::autoroute::{
    AccessBlocker, ExtractedProblem, RoutingShape, RoutingTerminalId, TerminalAccess,
};
use librepcb_scene::librepcb_canvas::kurbo::{BezPath, Circle, Line, Rect};
use librepcb_scene::librepcb_canvas::peniko::Color;
use librepcb_scene::librepcb_canvas::{Item, Layer as CanvasLayer, LayerId, Scene, Style, convert};
use librepcb_scene::{
    BoardScene, BoardSceneLayer, BoardSide, ColorScheme, RenderOptions, RenderSize,
};
use serde_json::{Value, json};

use crate::error::ToolResult;
use crate::units::mm;

const SIZE: u32 = 640;
const BLOCKER: LayerId = LayerId(1000);
const BLOCKED: LayerId = LayerId(1001);
const CLEAR: LayerId = LayerId(1002);
const ACCESS: LayerId = LayerId(1003);

pub(super) fn render(
    project: &Project,
    board: BoardId,
    extracted: &ExtractedProblem,
    terminal: RoutingTerminalId,
    report: &TerminalAccess,
    layer: Layer,
    reach: PositiveLength,
) -> ToolResult<(Value, Vec<u8>)> {
    let mut scene = BoardScene::build(project, board, BoardSide::Top, &ColorScheme::BOARD_DARK)?;
    for scene_layer in scene.layers().to_vec() {
        let visible = match scene_layer {
            BoardSceneLayer::Board(l) => l == layer || l.is_board_edge(),
            BoardSceneLayer::ThtPads(l) | BoardSceneLayer::Vias(l) | BoardSceneLayer::Zones(l) => {
                l == layer
            }
            BoardSceneLayer::Holes | BoardSceneLayer::HoleFills => true,
            BoardSceneLayer::AirWires => false,
        };
        scene.set_scene_layer_visible(scene_layer, visible);
    }
    let background = scene.background();
    let canvas = scene.scene_mut();
    for (id, color) in [
        (BLOCKER, Color::from_rgba8(255, 209, 102, 150)),
        (BLOCKED, Color::from_rgb8(255, 107, 130)),
        (CLEAR, Color::from_rgb8(110, 231, 165)),
        (ACCESS, Color::from_rgb8(117, 224, 255)),
    ] {
        canvas.set_layer(id, CanvasLayer::new(color));
    }

    let problem = &extracted.problem;
    let center = convert::point(problem.terminals[terminal.0].position);
    let radius = (1.35 * mm(*reach)).max(1.0);
    let bounds = Rect::from_center_size(center, (radius * 2.0, radius * 2.0));
    let stroke = radius / 100.0;
    let mut highlighted = Vec::new();
    for probe in report.probes.iter().filter(|probe| probe.layer == layer) {
        let probe_layer = if let Some(blocker) = probe.blocker {
            if !highlighted.contains(&blocker) {
                highlighted.push(blocker);
                let shape = match blocker {
                    AccessBlocker::Obstacle(i) => Some(&problem.obstacles[i].shape),
                    AccessBlocker::Hole(i) => Some(&problem.holes[i].shape),
                    AccessBlocker::Terminal(id) => Some(&problem.terminals[id.0].shape),
                    AccessBlocker::BoardEdge => None,
                };
                if let Some(shape) = shape {
                    highlight(canvas, shape, stroke);
                }
            }
            BLOCKED
        } else {
            CLEAR
        };
        canvas.insert(
            Item::new(
                probe_layer,
                Line::new(center, convert::point(probe.end)),
                Style::stroke(stroke),
            )
            .with_z(i32::MAX - 1),
        );
    }
    for &(access_layer, position) in &report.access_points {
        if access_layer == layer {
            canvas.insert(
                Item::new(
                    ACCESS,
                    Circle::new(convert::point(position), stroke),
                    Style::fill(),
                )
                .with_z(i32::MAX),
            );
        }
    }
    canvas.insert(
        Item::new(
            ACCESS,
            Circle::new(center, stroke * 3.0),
            Style::stroke(stroke),
        )
        .with_z(i32::MAX),
    );

    let options = RenderOptions {
        size: RenderSize::Fit {
            width: SIZE,
            height: SIZE,
        },
        margin: 0.0,
        background: None,
    };
    let png = librepcb_scene::render_scene(canvas, Some(bounds), false, background, &options)?
        .to_png()?;
    let info = json!({
        "layer": layer.id(), "width": SIZE, "height": SIZE, "mime_type": "image/png",
        "bounds_mm": {
            "min": {"x": bounds.x0, "y": bounds.y0}, "max": {"x": bounds.x1, "y": bounds.y1},
        },
        "legend": {
            "amber": "blocking routing shape", "pink": "blocked probe", "green": "clear probe",
            "cyan_ring": "endpoint", "cyan_dots": "reported grid access points",
        },
    });
    Ok((info, png))
}

fn highlight(canvas: &mut Scene, shape: &RoutingShape, stroke: f64) {
    let item = match shape {
        RoutingShape::Circle { center, diameter } => Item::new(
            BLOCKER,
            Circle::new(convert::point(*center), mm(**diameter) / 2.0),
            Style::stroke(stroke * 2.0),
        ),
        RoutingShape::Segment { start, end, width } => Item::new(
            BLOCKER,
            Line::new(convert::point(*start), convert::point(*end)),
            Style::stroke(mm(**width)),
        ),
        RoutingShape::Polygon(points) => {
            let mut path = BezPath::new();
            if let Some((first, rest)) = points.split_first() {
                path.move_to(convert::point(*first));
                for &point in rest {
                    path.line_to(convert::point(point));
                }
                path.close_path();
            }
            Item::new(BLOCKER, path, Style::stroke(stroke * 2.0))
        }
    };
    canvas.insert(item.with_z(i32::MAX - 2));
}
