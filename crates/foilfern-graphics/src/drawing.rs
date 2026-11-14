//! Port of libs/librepcb/core/export/graphicspainter.{h,cpp}.
//! SVG nodes replace QPainter calls; paths reuse canvas's core conversions.
use crate::{Error, GraphicsSettings, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use foilfern_canvas::convert;
use foilfern_core::{
    geometry::{Circle, Image, Path, Polygon, Text},
    types::{Alignment, Angle, Color, HAlign, Point, VAlign},
    utils::{toolbox, transform::Transform},
};
use kurbo::{Affine, BezPath, Shape};
use std::sync::{Arc, OnceLock};
use svg::node::element::{
    Group, Image as SvgImage, Path as SvgPath, Rectangle, TSpan, Text as SvgText,
};
/// An owned page display list, in model millimetres with Y up.
#[derive(Debug, Clone)]
pub struct Drawing {
    pub(crate) elements: Vec<String>,
    filled_areas: Option<Vec<FilledArea>>,
}
/// An ordered even-odd fill, in model millimetres (Y up). Retained paths
/// let raster and vector backends consume exactly the same material regions.
#[derive(Debug, Clone)]
pub struct FilledArea {
    pub path: BezPath,
    pub color: Color,
}
impl Default for Drawing {
    fn default() -> Self {
        Self {
            elements: Vec::new(),
            filled_areas: Some(Vec::new()),
        }
    }
}
static_assertions::assert_impl_all!(Drawing:Send,Sync);
fn paint(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}
pub(crate) fn svg_options() -> resvg::usvg::Options<'static> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    let fontdb = FONTS
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone();
    let mut options = resvg::usvg::Options {
        fontdb,
        font_family: "sans-serif".into(),
        ..Default::default()
    };
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options
}
fn font_metrics(mono: bool) -> (f64, f64) {
    use resvg::usvg::fontdb::{Family, Query};
    let options = svg_options();
    let families = if mono {
        [
            Family::Name("DejaVu Sans Mono"),
            Family::Name("Noto Sans Mono"),
            Family::Monospace,
        ]
    } else {
        [
            Family::Name("DejaVu Sans"),
            Family::Name("Noto Sans"),
            Family::SansSerif,
        ]
    };
    options
        .fontdb
        .query(&Query {
            families: &families,
            ..Default::default()
        })
        .and_then(|id| {
            options
                .fontdb
                .with_face_data(id, |bytes, index| {
                    ttf_parser::Face::parse(bytes, index).ok().map(|f| {
                        let upm = f64::from(f.units_per_em());
                        let ascent = f64::from(f.ascender()) / upm;
                        let descent = f64::from(f.descender()) / upm;
                        let gap = f64::from(f.line_gap()) / upm;
                        (ascent, (ascent - descent + gap).max(0.1))
                    })
                })
                .flatten()
        })
        .unwrap_or((0.8, 1.2))
}
impl Drawing {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn append(&mut self, other: Drawing) {
        match (&mut self.filled_areas, other.filled_areas) {
            (Some(areas), Some(other)) => areas.extend(other),
            _ => self.filled_areas = None,
        }
        self.elements.extend(other.elements);
    }
    /// Returns retained material fills when the drawing contains only areas.
    /// Text, strokes and images require the general SVG rendering path.
    pub fn filled_areas(&self) -> Option<&[FilledArea]> {
        self.filled_areas.as_deref()
    }
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
    /// Adds a path in world coordinates. Visible strokes use round caps/joins.
    pub fn path(
        &mut self,
        path: BezPath,
        width: f64,
        line: Option<Color>,
        fill: Option<Color>,
        settings: &GraphicsSettings,
    ) {
        if line.is_none() && fill.is_none() {
            return;
        }
        let path = Affine::FLIP_Y * path;
        let mut node = SvgPath::new()
            .set("d", path.to_svg())
            .set("fill-rule", "nonzero");
        if let Some(c) = fill {
            node = node
                .set("fill", paint(c))
                .set("fill-opacity", f64::from(c.a) / 255.);
        } else {
            node = node.set("fill", "none");
        }
        if let Some(c) = line.filter(|_| width > 0. || fill.is_none()) {
            node = node
                .set("stroke", paint(c))
                .set("stroke-opacity", f64::from(c.a) / 255.)
                .set(
                    "stroke-width",
                    width
                        .max(settings.min_line_width)
                        .max(25.4 / f64::from(settings.dpi.max(1))),
                )
                .set("stroke-linecap", "round")
                .set("stroke-linejoin", "round");
        }
        self.filled_areas = None;
        self.elements.push(node.to_string());
    }
    /// A compound filled area, including holes, with even-odd winding.
    pub fn area(&mut self, path: BezPath, color: Option<Color>) {
        let Some(color) = color else { return };
        if let Some(areas) = &mut self.filled_areas {
            areas.push(FilledArea {
                path: path.clone(),
                color,
            });
        }
        self.elements.push(
            SvgPath::new()
                .set("d", (Affine::FLIP_Y * path).to_svg())
                .set("fill-rule", "evenodd")
                .set("fill", paint(color))
                .set("fill-opacity", f64::from(color.a) / 255.)
                .to_string(),
        );
    }
    pub fn polygon(&mut self, p: &Polygon, transform: &Transform, settings: &GraphicsSettings) {
        let role = transform.map(&p.layer()).color_role();
        if p.path().is_zero_length() {
            if let Some(vertex) = p.path().vertices().first()
                && p.line_width().to_nm() > 0
            {
                let point = transform.map(&vertex.pos);
                self.path(
                    kurbo::Circle::new(convert::point(point), p.line_width().to_mm() / 2.)
                        .to_path(0.0001),
                    0.,
                    None,
                    settings.color(role),
                    settings,
                );
            }
            return;
        }
        self.path(
            convert::transform(transform) * convert::path(p.path()),
            p.line_width().to_mm(),
            settings.color(role),
            settings.fill(role, p.is_filled(), p.is_grab_area()),
            settings,
        );
    }
    pub fn circle(&mut self, c: &Circle, transform: &Transform, settings: &GraphicsSettings) {
        let role = transform.map(&c.layer()).color_role();
        let circle = kurbo::Circle::new(
            convert::point(transform.map(&c.center())),
            c.diameter().to_mm() / 2.,
        );
        self.path(
            circle.to_path(0.0001),
            c.line_width().to_mm(),
            settings.color(role),
            settings.fill(role, c.is_filled(), c.is_grab_area()),
            settings,
        );
    }
    pub fn line(
        &mut self,
        start: Point,
        end: Point,
        width: f64,
        color: Option<Color>,
        settings: &GraphicsSettings,
    ) {
        let mut path = BezPath::new();
        path.move_to(convert::point(start));
        path.line_to(convert::point(end));
        if start == end {
            let circle = kurbo::Circle::new(
                convert::point(start),
                width.max(settings.min_line_width) / 2.,
            );
            self.path(circle.to_path(0.0001), 0., None, color, settings);
        } else {
            self.path(path, width, color, None, settings);
        }
    }
    pub fn slot(
        &mut self,
        path: &Path,
        diameter: foilfern_core::types::PositiveLength,
        line: Option<Color>,
        fill: Option<Color>,
        settings: &GraphicsSettings,
    ) {
        for outline in path.to_outline_strokes(diameter) {
            self.path(convert::path(&outline), 0., line, fill, settings);
        }
    }
    pub fn junction(&mut self, position: Point, color: Option<Color>, settings: &GraphicsSettings) {
        self.path(
            kurbo::Circle::new(convert::point(position), 0.6).to_path(0.0001),
            0.,
            None,
            color,
            settings,
        );
    }
    pub fn text(&mut self, t: &Text, value: &str, settings: &GraphicsSettings) {
        self.label(
            t.position(),
            t.rotation(),
            t.height().to_mm(),
            t.align(),
            value,
            settings.color(t.layer().color_role()),
            true,
            false,
            false,
        );
    }
    /// Text height is the font's complete line height, as in the source.
    #[allow(clippy::too_many_arguments)]
    pub fn label(
        &mut self,
        position: Point,
        rotation: Angle,
        height: f64,
        alignment: Alignment,
        value: &str,
        color: Option<Color>,
        auto_rotate: bool,
        mirror: bool,
        mono: bool,
    ) {
        self.label_impl(
            position,
            rotation,
            height,
            alignment,
            value,
            color,
            auto_rotate,
            mirror,
            mono,
            false,
        );
    }
    #[allow(clippy::too_many_arguments)]
    pub fn markup_label(
        &mut self,
        position: Point,
        rotation: Angle,
        height: f64,
        alignment: Alignment,
        value: &str,
        color: Option<Color>,
        auto_rotate: bool,
        mirror: bool,
        mono: bool,
    ) {
        self.label_impl(
            position,
            rotation,
            height,
            alignment,
            value,
            color,
            auto_rotate,
            mirror,
            mono,
            true,
        );
    }
    #[allow(clippy::too_many_arguments)]
    fn label_impl(
        &mut self,
        position: Point,
        rotation: Angle,
        height: f64,
        mut alignment: Alignment,
        value: &str,
        color: Option<Color>,
        auto_rotate: bool,
        mirror: bool,
        mono: bool,
        markup: bool,
    ) {
        let Some(color) = color else { return };
        if value.trim().is_empty() || !height.is_finite() || height <= 0. {
            return;
        }
        let flipped = auto_rotate && toolbox::is_text_upside_down(rotation);
        if flipped {
            alignment = alignment.mirrored();
        }
        if mirror {
            alignment = alignment.mirrored_h();
        }
        let (ascent, line_height) = font_metrics(mono);
        let size = height / line_height;
        let lines: Vec<_> = value.split('\n').collect();
        let block = height * lines.len() as f64;
        let y = match alignment.v {
            VAlign::Top => 0.,
            VAlign::Center => -block / 2.,
            VAlign::Bottom => -block,
        } + ascent * size;
        let mut group = Group::new().set(
            "transform",
            format!(
                "translate({} {}) rotate({}) scale({} 1)",
                position.x.to_mm(),
                -position.y.to_mm(),
                -rotation.to_deg() + if flipped { 180. } else { 0. },
                if mirror { -1 } else { 1 }
            ),
        );
        for (i, line) in lines.iter().enumerate() {
            let node = if markup {
                let (text, ranges) =
                    foilfern_core::utils::overline_markup_parser::extract_overlines(line);
                let mut boundaries = vec![0, text.len()];
                for range in &ranges {
                    boundaries.extend([range.start, range.end]);
                }
                boundaries.sort_unstable();
                boundaries.dedup();
                let mut node = SvgText::new("");
                for span in boundaries.windows(2) {
                    let mut segment = TSpan::new(&text[span[0]..span[1]]);
                    if ranges.iter().any(|r| r.contains(&span[0])) {
                        segment = segment.set("text-decoration", "overline");
                    }
                    node = node.add(segment);
                }
                node
            } else {
                SvgText::new(*line)
            };
            group = group.add(
                node.set("xml:space", "preserve")
                    .set("x", 0)
                    .set("y", y + i as f64 * height)
                    .set(
                        "font-family",
                        if mono {
                            "DejaVu Sans Mono, Noto Sans Mono, monospace"
                        } else {
                            "DejaVu Sans, Noto Sans, sans-serif"
                        },
                    )
                    .set("font-size", size)
                    .set(
                        "text-anchor",
                        match alignment.h {
                            HAlign::Left => "start",
                            HAlign::Center => "middle",
                            HAlign::Right => "end",
                        },
                    )
                    .set("fill", paint(color))
                    .set("fill-opacity", f64::from(color.a) / 255.),
            );
        }
        self.filled_areas = None;
        self.elements.push(group.to_string());
    }
    pub fn image(
        &mut self,
        image: &Image,
        bytes: &[u8],
        settings: &GraphicsSettings,
    ) -> Result<()> {
        let invalid = |reason: String| {
            Error::Image(format!(
                "Failed to load image '{}': {reason}",
                image.file_name()
            ))
        };
        if image.file_extension() == "svg" {
            resvg::usvg::Tree::from_data(bytes, &svg_options())
                .map_err(|e| invalid(e.to_string()))?;
        } else {
            let format = if image.file_extension() == "jpg" {
                image::ImageFormat::Jpeg
            } else {
                image::ImageFormat::Png
            };
            image::load_from_memory_with_format(bytes, format)
                .map_err(|e| invalid(e.to_string()))?;
        }
        let mime = match image.file_extension() {
            "svg" => "image/svg+xml",
            "jpg" => "image/jpeg",
            _ => "image/png",
        };
        let x = image.position().x.to_mm();
        let y = -image.position().y.to_mm();
        let mut node = SvgImage::new()
            .set("x", 0)
            .set("y", -image.height().to_mm())
            .set("width", image.width().to_mm())
            .set("height", image.height().to_mm())
            .set("preserveAspectRatio", "none")
            .set(
                "href",
                format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
            );
        self.filled_areas = None;
        if settings.monochrome {
            let matrix = if settings.background == Color::BLACK {
                "-.299 -.587 -.114 0 1 -.299 -.587 -.114 0 1 -.299 -.587 -.114 0 1 0 0 0 1 0"
            } else {
                ".299 .587 .114 0 0 .299 .587 .114 0 0 .299 .587 .114 0 0 0 0 0 1 0"
            };
            self.elements.push(format!("<defs><filter id=\"gray\" color-interpolation-filters=\"sRGB\"><feColorMatrix type=\"matrix\" values=\"{matrix}\"/></filter></defs>"));
            node = node.set("filter", "url(#gray)");
        }
        let mut group = Group::new()
            .set(
                "transform",
                format!("translate({x} {y}) rotate({})", -image.rotation().to_deg()),
            )
            .add(node);
        if let Some(width) = image.border_width()
            && let Some(color) = settings.color("schematic_image_borders")
        {
            let width = width.to_mm().max(settings.min_line_width);
            group = group.add(
                Rectangle::new()
                    .set("x", -width / 2.)
                    .set("y", -image.height().to_mm() - width / 2.)
                    .set("width", image.width().to_mm() + width)
                    .set("height", image.height().to_mm() + width)
                    .set("fill", "none")
                    .set("stroke", paint(color))
                    .set("stroke-opacity", f64::from(color.a) / 255.)
                    .set("stroke-width", width),
            );
        }
        self.elements.push(group.to_string());
        Ok(())
    }
    pub(crate) fn body(&self) -> String {
        self.elements.join("\n")
    }
}
