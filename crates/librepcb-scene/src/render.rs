//! Headless rendering of scenes into RGBA images and PNG files (for the
//! MCP `render` tool and tests). No upstream counterpart (upstream exports
//! images through `GraphicsExport` with Qt).

use librepcb_canvas::kurbo::Rect;
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{CpuRenderer, CpuRendererSettings, Renderer, Scene, View};
use librepcb_core::project::{BoardId, Project, SchematicId};
use librepcb_core::types::Point;

use crate::board::{BoardScene, BoardSide};
use crate::colors::ColorScheme;
use crate::error::{Error, Result};
use crate::schematic::SchematicScene;

/// Largest supported image edge in pixels.
pub const MAX_IMAGE_SIZE: u32 = 16384;

/// Size of the rendered image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderSize {
    /// A fixed image size; the content is scaled to fit (keeping the aspect
    /// ratio) and centered.
    Fit {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// A fixed resolution; the image size follows from the content size
    /// (limited to [`MAX_IMAGE_SIZE`] per edge, reducing the resolution if
    /// needed).
    Dpi(f64),
}

/// Options of the PNG rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// Image size or resolution.
    pub size: RenderSize,
    /// Margin around the content in pixels.
    pub margin: f64,
    /// Background color; `None` takes the background of the color scheme.
    pub background: Option<Color>,
}

impl Default for RenderOptions {
    /// 1600 × 1200 pixels, fitted, with a margin of 20 pixels.
    fn default() -> Self {
        Self {
            size: RenderSize::Fit {
                width: 1600,
                height: 1200,
            },
            margin: 20.0,
            background: None,
        }
    }
}

/// A rendered image: straight (not premultiplied) RGBA8, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// Encodes the image as PNG.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header()?;
            writer.write_image_data(&self.data)?;
            writer.finish()?;
        }
        Ok(out)
    }
}

/// Bounding box of the items on visible layers.
pub fn visible_bounds(scene: &Scene) -> Option<Rect> {
    scene
        .items()
        .filter(|(_, item)| scene.is_layer_visible(item.layer) && !item.geometry.is_empty())
        .map(|(_, item)| item.bounding_box())
        .reduce(|a, b| a.union(b))
}

/// Renders `bounds` (world coordinates) of a scene, mirrored horizontally
/// if `mirrored` (board bottom view).
pub fn render_scene(
    scene: &Scene,
    bounds: Option<Rect>,
    mirrored: bool,
    background: Color,
    options: &RenderOptions,
) -> Result<RgbaImage> {
    Ok(render_scene_with_view(scene, bounds, mirrored, background, options)?.0)
}

fn render_scene_with_view(
    scene: &Scene,
    bounds: Option<Rect>,
    mirrored: bool,
    background: Color,
    options: &RenderOptions,
) -> Result<(RgbaImage, View)> {
    // Show 10 mm around the origin if there is nothing to show.
    let bounds = bounds
        .filter(|r| r.width().is_finite() && r.height().is_finite())
        .unwrap_or(Rect::new(-5.0, -5.0, 5.0, 5.0));
    let margin = if options.margin.is_finite() {
        options.margin.max(0.0)
    } else {
        0.0
    };
    let (width, height, scale) = match options.size {
        RenderSize::Fit { width, height } => (width, height, None),
        RenderSize::Dpi(dpi) => {
            let mut scale = dpi / 25.4; // pixels per mm
            let content = bounds.width().max(bounds.height()).max(1e-6);
            let max_scale = (f64::from(MAX_IMAGE_SIZE) - 2.0 * margin) / content;
            if !(scale.is_finite() && scale > 0.0) {
                return Err(Error::InvalidSize {
                    width: 0,
                    height: 0,
                });
            }
            scale = scale.min(max_scale.max(1e-6));
            let px = |mm: f64| {
                (mm * scale + 2.0 * margin)
                    .ceil()
                    .clamp(1.0, f64::from(MAX_IMAGE_SIZE)) as u32
            };
            (px(bounds.width()), px(bounds.height()), Some(scale))
        }
    };
    if width == 0 || height == 0 || width > MAX_IMAGE_SIZE || height > MAX_IMAGE_SIZE {
        return Err(Error::InvalidSize { width, height });
    }
    let mut view = View::new((f64::from(width), f64::from(height)), 1.0);
    view.set_mirrored(mirrored);
    match scale {
        None => view.fit(bounds, margin),
        Some(scale) => {
            view.set_scale(scale);
            view.center_on(bounds.center());
        }
    }
    let mut renderer = CpuRenderer::new(CpuRendererSettings {
        background,
        frame_cache: false,
        ..CpuRendererSettings::default()
    });
    let premultiplied = renderer.render(scene, &view)?;
    let mut data = premultiplied.to_vec();
    for px in data.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Ok((
        RgbaImage {
            width,
            height,
            data,
        },
        view,
    ))
}

/// Renders a schematic page with the default color scheme
/// ([`ColorScheme::SCHEMATIC_LIGHT`]) into a PNG, fitted to its content.
pub fn render_schematic_png(
    project: &Project,
    schematic: SchematicId,
    options: &RenderOptions,
) -> Result<Vec<u8>> {
    let scene = SchematicScene::build(project, schematic, &ColorScheme::SCHEMATIC_LIGHT)?;
    render_schematic_scene(&scene, options)?.to_png()
}

/// Renders a schematic scene into an image, fitted to its content.
pub fn render_schematic_scene(
    scene: &SchematicScene,
    options: &RenderOptions,
) -> Result<RgbaImage> {
    render_scene(
        scene.scene(),
        scene.content_bounds(),
        false,
        options.background.unwrap_or(scene.background()),
        options,
    )
}

/// Renders a schematic region in sheet coordinates. Pixels outside the
/// requested rectangle are filled with the scene background.
pub fn render_schematic_region(
    scene: &SchematicScene,
    min: Point,
    max: Point,
    width: u32,
    height: u32,
) -> Result<RgbaImage> {
    let bounds = Rect::new(min.x.to_mm(), min.y.to_mm(), max.x.to_mm(), max.y.to_mm());
    if bounds.width() <= 0. || bounds.height() <= 0. {
        return Err(Error::InvalidRegion);
    }
    let options = RenderOptions {
        size: RenderSize::Fit { width, height },
        margin: 0.,
        background: None,
    };
    let background = scene.background();
    let (mut image, view) =
        render_scene_with_view(scene.scene(), Some(bounds), false, background, &options)?;
    let scale = view.scale();
    let x0 = (f64::from(width) - bounds.width() * scale) / 2.;
    let y0 = (f64::from(height) - bounds.height() * scale) / 2.;
    let x1 = f64::from(width) - x0;
    let y1 = f64::from(height) - y0;
    let color = background.to_rgba8();
    let rgba = [color.r, color.g, color.b, color.a];
    let edge = |coordinate: f64| (coordinate - 0.5).ceil().clamp(0., f64::from(width)) as usize;
    let left = edge(x0);
    let right = edge(x1);
    for (index, row) in image
        .data
        .as_chunks_mut::<4>()
        .0
        .chunks_mut(width as usize)
        .enumerate()
    {
        let y = index as f64 + 0.5;
        if y < y0 || y >= y1 {
            row.fill(rgba);
        } else {
            row[..left].fill(rgba);
            row[right..].fill(rgba);
        }
    }
    Ok(image)
}

/// Renders a board seen from `side` with the default color scheme
/// ([`ColorScheme::BOARD_DARK`]) and layer visibility into a PNG, fitted to
/// its content.
pub fn render_board_png(
    project: &Project,
    board: BoardId,
    side: BoardSide,
    options: &RenderOptions,
) -> Result<Vec<u8>> {
    let mut scene = BoardScene::build(project, board, side, &ColorScheme::BOARD_DARK)?;
    render_board_scene(&mut scene, options)?.to_png()
}

/// Renders a board scene into an image, fitted to its content.
pub fn render_board_scene(scene: &mut BoardScene, options: &RenderOptions) -> Result<RgbaImage> {
    let background = options.background.unwrap_or(scene.background());
    // Hole fills are painted with the background color.
    scene.set_background(background);
    render_scene(
        scene.scene(),
        scene.content_bounds(),
        scene.side() == BoardSide::Bottom,
        background,
        options,
    )
}
