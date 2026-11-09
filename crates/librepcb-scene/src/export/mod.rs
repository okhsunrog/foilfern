//! Port of libs/librepcb/core/export/graphicsexport.{h,cpp} (and the page
//! setup parts of graphicspainter.{h,cpp}): exports pages to PDF, SVG and
//! image files.
//!
//! A page is a [`Drawing`] (filled and stroked paths in world coordinates,
//! usually taken from a scene with [`Drawing::from_scene()`]) plus its
//! [`GraphicsExportSettings`]. [`GraphicsExport::export()`] lays the pages
//! out like upstream (page size and orientation, margins, rotation,
//! mirroring, scale or fit, background, minimum line width), names the
//! files like upstream (one PDF with all pages; one SVG/image file per page
//! with the page number appended if there are several pages) and reports
//! the written files and errors with upstream's messages.
//!
//! Output backends (no Qt, so the bytes differ from upstream's):
//! - PDF: vector paths through the `pdf-writer` crate ([`pdf`]).
//! - SVG: a small hand-written writer (one `<path>` per paint).
//! - Images (PNG, JPEG, BMP): rasterized with vello_cpu, encoded with the
//!   `image` crate ([`raster`]).
//!
//! [`ProjectGraphicsExporter`] builds the drawings of schematic and board
//! pages for the core [`OutputJobRunner`]; [`board_rendering`] ports
//! upstream's `RealisticBoardPainter`.
//!
//! Printing and previews (`startPrint()`, `startPreview()`) and the
//! asynchronous API are not ported: exports run synchronously on the
//! caller's thread.
//!
//! [`OutputJobRunner`]: librepcb_core::project::OutputJobRunner

pub mod board_rendering;
mod pdf;
mod project;
mod raster;
mod svg;

use librepcb_canvas::kurbo::{Affine, BezPath, Rect, Shape, Size};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{LayerId, Paint, Scene, StrokeStyle};
use librepcb_core::export::{GraphicsExportSettings, PageOrientation, PageSize};
use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::project::GraphicsExportResult;
use librepcb_i18n::tr;

pub use project::{
    ProjectGraphicsExporter, drawing_for_page, export_color_scheme, footprint_drawing,
    symbol_drawing,
};

/// Translation context of the upstream class.
const TR_CONTEXT: &str = "librepcb::GraphicsExport";

/// File extensions supported by [`GraphicsExport::export()`] (upstream
/// `getSupportedExtensions()`: PDF, SVG and the image formats).
pub const SUPPORTED_EXTENSIONS: &[&str] = &["pdf", "svg", "bmp", "jpeg", "jpg", "png"];

/// How a path is painted.
#[derive(Debug, Clone, PartialEq)]
pub enum PrimitivePaint {
    /// Filled (non-zero winding rule).
    Fill(Color),
    /// Stroked; hairlines and thinner lines are drawn with the minimum line
    /// width of the export settings.
    Stroke(Color, StrokeStyle),
}

/// A painted path.
#[derive(Debug, Clone, PartialEq)]
pub struct Primitive {
    /// The path in world coordinates (millimeters, Y axis up).
    pub path: BezPath,
    /// The paint.
    pub paint: PrimitivePaint,
}

/// The content of a page: paths in paint order (upstream: what a
/// `GraphicsPagePainter` paints).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    primitives: Vec<Primitive>,
}

impl Drawing {
    /// Creates an empty drawing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the render groups of the visible layers of a scene in draw
    /// order (fully transparent paints are skipped, like invalid colors
    /// upstream).
    pub fn from_scene(scene: &Scene) -> Self {
        Self::from_scene_sorted(scene, |_| 0)
    }

    /// Like [`from_scene()`](Self::from_scene), but the render groups are
    /// sorted (stably) by a key of their layer, e.g. the paint order of the
    /// color roles.
    pub fn from_scene_sorted(scene: &Scene, key: impl Fn(LayerId) -> usize) -> Self {
        let mut groups: Vec<_> = scene.draw_groups().collect();
        groups.sort_by_key(|g| key(g.layer()));
        let mut drawing = Self::new();
        for group in groups {
            let paint = match group.paint() {
                Paint::Fill(color) => PrimitivePaint::Fill(color),
                Paint::Stroke(color, style) => PrimitivePaint::Stroke(color, style.clone()),
            };
            let mut path = BezPath::new();
            group.encode(&mut path);
            drawing.push(path, paint);
        }
        drawing
    }

    /// Appends a path (ignored if empty or fully transparent).
    pub fn push(&mut self, path: BezPath, paint: PrimitivePaint) {
        let color = match &paint {
            PrimitivePaint::Fill(c) | PrimitivePaint::Stroke(c, _) => *c,
        };
        if path.elements().is_empty() || color.components[3] <= 0.0 {
            return;
        }
        self.primitives.push(Primitive { path, paint });
    }

    /// The paths in paint order.
    pub fn primitives(&self) -> &[Primitive] {
        &self.primitives
    }

    /// Returns whether there is nothing to paint.
    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }

    /// Bounding box of the painted area (world coordinates), including the
    /// stroke widths (upstream `QPicture::boundingRect()`).
    pub fn bounds(&self, min_line_width: f64) -> Option<Rect> {
        self.primitives
            .iter()
            .map(|p| {
                let bbox = p.path.bounding_box();
                match &p.paint {
                    PrimitivePaint::Fill(_) => bbox,
                    PrimitivePaint::Stroke(_, style) => {
                        let w = stroke_width(style, min_line_width) / 2.0;
                        bbox.inflate(w, w)
                    }
                }
            })
            .reduce(|a, b| a.union(b))
    }
}

/// The effective stroke width (upstream `getPenWidthPx()`).
pub(crate) fn stroke_width(style: &StrokeStyle, min_line_width: f64) -> f64 {
    if style.is_hairline() {
        min_line_width
    } else {
        style.width.max(min_line_width)
    }
}

/// The kurbo stroke of a style with the effective width.
pub(crate) fn kurbo_stroke(
    style: &StrokeStyle,
    min_line_width: f64,
) -> librepcb_canvas::kurbo::Stroke {
    let mut stroke = librepcb_canvas::kurbo::Stroke::new(stroke_width(style, min_line_width))
        .with_caps(style.cap)
        .with_join(style.join);
    if let Some(dash) = &style.dash
        && dash.pattern.iter().any(|d| *d > 0.0)
    {
        stroke = stroke.with_dashes(dash.offset, dash.pattern.iter().copied());
    }
    stroke
}

/// Converts a core color.
pub(crate) fn to_color(c: librepcb_core::types::Color) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

/// A page of an export: the drawing and its settings (upstream
/// `GraphicsExport::Page`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportPage {
    /// What to paint.
    pub drawing: Drawing,
    /// Page layout and colors (the colors are already applied to the
    /// drawing).
    pub settings: GraphicsExportSettings,
}

/// The layout of a page on the output device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PageLayout {
    /// Page size in device units.
    pub size: Size,
    /// World (mm, Y up) → device (Y down).
    pub transform: Affine,
    /// Background color (painted if not fully transparent).
    pub background: Color,
    /// Minimum line width in world units (mm).
    pub min_line_width: f64,
}

/// Upstream `getOrientation()`: portrait if higher than wide.
fn is_portrait(size: Size) -> bool {
    size.height > size.width
}

/// Lays out a page for a device with `dpi` device units per inch (upstream
/// `GraphicsExport::run()`, per page).
pub(crate) fn layout(page: &ExportPage, dpi: f64) -> PageLayout {
    let s = &page.settings;
    let min_line_width = s.min_line_width.to_mm();

    // Source rect in upstream's pixel space (Y down), in millimeters.
    let world = page.drawing.bounds(min_line_width).unwrap_or(Rect::ZERO);
    let source = Rect::new(world.x0, -world.y1, world.x1, -world.y0);

    // Source transformation (upstream `getSourceTransformation()`).
    let mut source_transform = Affine::IDENTITY;
    if s.rotate {
        source_transform *= Affine::rotate(-std::f64::consts::FRAC_PI_2);
    }
    if s.mirror {
        source_transform *= Affine::scale_non_uniform(-1.0, 1.0);
    }
    if let Some(scale) = s.scale {
        source_transform *= Affine::scale(scale.to_normalized());
    }
    let transformed = source_transform.transform_rect_bbox(source);

    // Page size: fixed or derived from the content.
    let page_size = s
        .page_size
        .as_deref()
        .and_then(PageSize::from_key)
        .unwrap_or_else(|| {
            let mm = |v: f64| librepcb_core::types::Length::from_mm(v).unwrap_or_default();
            PageSize::custom(
                mm(transformed.width()) + *s.margin_left + *s.margin_right,
                mm(transformed.height()) + *s.margin_top + *s.margin_bottom,
            )
        });

    // Orientation.
    let portrait = match s.orientation {
        PageOrientation::Landscape => false,
        PageOrientation::Portrait => true,
        PageOrientation::Auto => is_portrait(transformed.size()),
    };

    // Page rect in device units.
    let (w, h) = page_size.size_pixels(dpi.round() as u32);
    let mut size = Size::new(f64::from(w), f64::from(h));
    if is_portrait(size) != portrait {
        size = Size::new(size.height, size.width);
    }
    let to_dev = |l: librepcb_core::types::UnsignedLength| l.to_mm() / 25.4 * dpi;
    let content = Rect::new(
        to_dev(s.margin_left),
        to_dev(s.margin_top),
        size.width - to_dev(s.margin_right),
        size.height - to_dev(s.margin_bottom),
    );

    // Scale factor (device units per millimeter).
    let px_per_mm = dpi / 25.4;
    let scale = if s.scale.is_some() {
        px_per_mm
    } else {
        let fit =
            (content.width() / transformed.width()).min(content.height() / transformed.height());
        if fit.is_finite() && fit > 0.0 {
            fit
        } else {
            px_per_mm
        }
    };

    let transform = Affine::translate(content.center().to_vec2())
        * source_transform
        * Affine::scale(scale)
        * Affine::translate(-source.center().to_vec2())
        * Affine::FLIP_Y;
    PageLayout {
        size,
        transform,
        background: to_color(s.background_color),
        min_line_width,
    }
}

/// Graphics export (upstream `GraphicsExport`).
#[derive(Debug, Clone)]
pub struct GraphicsExport {
    creator: String,
    document_name: String,
}

impl GraphicsExport {
    /// Creates an export; `creator` is written into PDF files (upstream
    /// `"LibrePCB <version>"`).
    pub fn new(creator: impl Into<String>) -> Self {
        Self {
            creator: creator.into(),
            document_name: String::new(),
        }
    }

    /// Sets the document name used as title of PDF and SVG files.
    pub fn set_document_name(&mut self, name: impl Into<String>) {
        self.document_name = name.into();
    }

    /// Exports the pages to a file (upstream `startExport()` +
    /// `waitForFinished()`). The file type is determined by the suffix;
    /// SVG and image exports of several pages append the page number to
    /// the file name. Output directories are created.
    pub fn export(&self, pages: &[ExportPage], file_path: &FilePath) -> GraphicsExportResult {
        let mut result = GraphicsExportResult::default();
        if let Err(e) = self.run(pages, file_path, &mut result.written_files) {
            result.errors.push(e);
        }
        result
    }

    fn run(
        &self,
        pages: &[ExportPage],
        file_path: &FilePath,
        written: &mut Vec<FilePath>,
    ) -> Result<(), String> {
        // Determine file basename and extension.
        let suffix = file_path.suffix().to_owned();
        let ext = suffix.to_lowercase();
        // Upstream chops the suffix and the character before it (the dot).
        let template = (pages.len() > 1).then(|| {
            let s = file_path.as_str();
            let mut base = s.strip_suffix(suffix.as_str()).unwrap_or(s).to_owned();
            base.pop();
            base
        });
        let page_path = |index: usize| -> FilePath {
            match &template {
                Some(t) => FilePath::new(format!("{t}{}.{suffix}", index + 1))
                    .unwrap_or_else(|| file_path.clone()),
                None => file_path.clone(),
            }
        };

        // Create output directory first.
        if let Some(parent) = file_path.parent_dir() {
            file_utils::make_path(&parent).map_err(|e| e.to_string())?;
        }

        if ext == "pdf" {
            written.push(file_path.clone());
            if pages.is_empty() {
                return Err(tr!(TR_CONTEXT, "No pages to export/print."));
            }
            let bytes = pdf::write(pages, &self.creator, &self.document_name);
            return file_utils::write_file(file_path, &bytes).map_err(|_| {
                tr!(
                    TR_CONTEXT,
                    "Failed to finish PDF export. Check permissions of output file."
                )
            });
        }

        for (index, page) in pages.iter().enumerate() {
            let out = page_path(index);
            if ext == "svg" {
                log::debug!("Export page {} as SVG to {}...", index + 1, out.as_str());
                let dpi = f64::from(page.settings.pixmap_dpi.max(1));
                let svg = svg::write(page, &layout(page, dpi), dpi, &self.document_name);
                written.push(out.clone());
                file_utils::write_file(&out, svg.as_bytes()).map_err(|_| {
                    "Failed to start printing - invalid printer or output file?".to_owned()
                })?;
                continue;
            }
            log::debug!("Export page {} as pixmap to {}...", index + 1, out.as_str());
            let format = raster::format(&out.suffix().to_lowercase()).ok_or_else(|| {
                tr!(
                    TR_CONTEXT,
                    "Failed to export image '{0}' due to unknown file extension. Supported extensions: {1}",
                    out.to_native(),
                    SUPPORTED_EXTENSIONS.join(", ")
                )
            })?;
            let dpi = f64::from(page.settings.pixmap_dpi.max(1));
            let image = raster::render(page, &layout(page, dpi)).ok_or_else(|| {
                "Failed to start printing - invalid printer or output file?".to_owned()
            })?;
            raster::save(&image, format, &out).map_err(|_| {
                tr!(
                    TR_CONTEXT,
                    "Failed to export image '{0}'. Check file permissions.",
                    out.to_native()
                )
            })?;
            written.push(out);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use librepcb_canvas::kurbo::{Circle, Point};

    fn page(settings: GraphicsExportSettings) -> ExportPage {
        let mut drawing = Drawing::new();
        drawing.push(
            Rect::new(0.0, 0.0, 100.0, 50.0).to_path(1e-3),
            PrimitivePaint::Fill(Color::BLACK),
        );
        ExportPage { drawing, settings }
    }

    #[test]
    fn auto_page_size() {
        let settings = GraphicsExportSettings::default();
        let l = layout(&page(settings), 72.0);
        // 100 x 50 mm plus 2 x 10 mm margins, in points.
        assert_eq!(l.size, Size::new(340.0, 198.0));
        // The top left corner of the content is at the margins.
        let p = l.transform * Point::new(0.0, 50.0);
        assert!(
            (p.x - 28.35).abs() < 0.5 && (p.y - 28.35).abs() < 0.5,
            "{p:?}"
        );
    }

    #[test]
    fn fixed_page_size_and_orientation() {
        let settings = GraphicsExportSettings {
            page_size: Some("A4".into()),
            ..GraphicsExportSettings::default()
        };
        let l = layout(&page(settings.clone()), 72.0);
        assert_eq!(l.size, Size::new(842.0, 595.0)); // landscape content
        let rotated = GraphicsExportSettings {
            rotate: true,
            ..settings
        };
        let l = layout(&page(rotated), 72.0);
        assert_eq!(l.size, Size::new(595.0, 842.0));
        // Rotated counterclockwise: the right edge is on top.
        let right = l.transform * Point::new(100.0, 25.0);
        let left = l.transform * Point::new(0.0, 25.0);
        assert!(right.y < left.y);
    }

    #[test]
    fn mirror_and_scale() {
        let settings = GraphicsExportSettings {
            mirror: true,
            scale: Some(
                librepcb_core::types::UnsignedRatio::new(
                    librepcb_core::types::Ratio::from_normalized(2.0),
                )
                .expect("positive"),
            ),
            ..GraphicsExportSettings::default()
        };
        let l = layout(&page(settings), 72.0);
        // Scale 2: 200 x 100 mm plus margins, in points.
        assert_eq!(l.size.width.round(), 624.0);
        let right = l.transform * Point::new(100.0, 25.0);
        let left = l.transform * Point::new(0.0, 25.0);
        assert!((left.x - right.x - 200.0 * 72.0 / 25.4).abs() < 1e-6);
    }

    #[test]
    fn bounds_include_strokes() {
        let mut d = Drawing::new();
        d.push(
            Circle::new((0.0, 0.0), 1.0).to_path(1e-3),
            PrimitivePaint::Stroke(Color::BLACK, StrokeStyle::new(0.0)),
        );
        let b = d.bounds(0.5).expect("bounds");
        assert!((b.x0 + 1.25).abs() < 1e-6);
        d.push(
            Circle::new((0.0, 0.0), 5.0).to_path(1e-3),
            PrimitivePaint::Fill(Color::TRANSPARENT),
        );
        assert_eq!(d.primitives().len(), 1);
    }
}
