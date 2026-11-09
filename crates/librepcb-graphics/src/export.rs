//! Port of libs/librepcb/core/export/graphicsexport.{h,cpp}.
//! SVG/resvg/svg2pdf implement formats; lopdf combines vector PDF pages.
use crate::{Drawing, Error, GraphicsSettings, Result, drawing::svg_options};
use kurbo::{Affine, Rect};
use librepcb_core::{
    export::PageOrientation,
    fileio::{FilePath, file_utils},
};
use std::path::Path;
struct PageLayout {
    svg: String,
    width: f64,
    height: f64,
    transform: Affine,
}
/// A display list with page-specific output settings.
#[derive(Debug, Clone)]
pub struct GraphicsPage {
    pub drawing: Drawing,
    pub settings: GraphicsSettings,
}
/// An encoded output; its name can acquire a page suffix for SVG/PNG.
#[derive(Debug, Clone)]
pub struct GraphicsFile {
    pub name: String,
    pub contents: Vec<u8>,
}
/// Stateless encoder with explicit metadata and a configurable raster budget.
#[derive(Debug, Clone)]
pub struct GraphicsExport {
    pub title: String,
    pub creator: String,
    pub max_pixels: u64,
}
impl Default for GraphicsExport {
    fn default() -> Self {
        Self {
            title: String::new(),
            creator: concat!("LibrePCB ", env!("CARGO_PKG_VERSION")).into(),
            max_pixels: 64_000_000,
        }
    }
}
impl GraphicsExport {
    pub fn generate(&self, pages: &[GraphicsPage], name: &str) -> Result<Vec<GraphicsFile>> {
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !["svg", "png", "pdf", "jpg", "jpeg", "bmp"].contains(&ext.as_str()) {
            return Err(Error::UnsupportedFormat(name.into()));
        }
        if ext == "pdf" && pages.is_empty() {
            return Err(Error::NoPages);
        }
        let mut result = Vec::new();
        let mut pdf_pages = Vec::new();
        for (i, page) in pages.iter().enumerate() {
            let PageLayout {
                svg,
                width,
                height,
                transform: page_transform,
            } = self.page_layout(page)?;
            let out = if ext != "pdf" && pages.len() > 1 {
                let path = Path::new(name);
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                path.with_file_name(format!(
                    "{stem}{}.{}",
                    i + 1,
                    path.extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                ))
                .to_string_lossy()
                .into_owned()
            } else {
                name.into()
            };
            let contents = match ext.as_str() {
                "svg" => svg.into_bytes(),
                "png" | "jpg" | "jpeg" | "bmp" => {
                    let w = (width * f64::from(page.settings.dpi) / 25.4)
                        .round()
                        .max(1.);
                    let h = (height * f64::from(page.settings.dpi) / 25.4)
                        .round()
                        .max(1.);
                    if !w.is_finite()
                        || !h.is_finite()
                        || w > f64::from(i32::MAX)
                        || h > f64::from(i32::MAX)
                        || w * h > self.max_pixels as f64
                    {
                        return Err(Error::Dimensions);
                    }
                    // Check Vello's dimension limit before allocating a buffer.
                    let areas = page.drawing.filled_areas();
                    if areas.is_some() && (w > f64::from(u16::MAX) || h > f64::from(u16::MAX)) {
                        return Err(Error::Dimensions);
                    }
                    let mut pixmap = resvg::tiny_skia::Pixmap::new(w as u32, h as u32)
                        .ok_or(Error::Dimensions)?;
                    if let Some(areas) = areas {
                        let color = |c: librepcb_core::types::Color| {
                            librepcb_canvas::peniko::Color::from_rgba8(c.r, c.g, c.b, c.a)
                        };
                        librepcb_canvas::render::rasterize_filled_paths(
                            areas.iter().map(|area| (&area.path, color(area.color))),
                            Affine::scale_non_uniform(w / width, h / height)
                                * page_transform
                                * Affine::FLIP_Y,
                            (w as u32, h as u32),
                            color(page.settings.background),
                            pixmap.data_mut(),
                        )?;
                    } else {
                        let tree = resvg::usvg::Tree::from_str(&svg, &svg_options())
                            .map_err(|e| Error::Svg(e.to_string()))?;
                        let transform = resvg::tiny_skia::Transform::from_scale(
                            w as f32 / tree.size().width(),
                            h as f32 / tree.size().height(),
                        );
                        resvg::render(&tree, transform, &mut pixmap.as_mut());
                    }
                    if ext == "png" {
                        pixmap
                            .encode_png()
                            .map_err(|e| Error::Image(e.to_string()))?
                    } else {
                        // Match the existing scene exporter: formats without
                        // alpha retain the premultiplied RGB channels.
                        let pixels = pixmap
                            .data()
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|pixel| pixel[..3].iter().copied())
                            .collect();
                        let image = image::RgbImage::from_raw(w as u32, h as u32, pixels)
                            .ok_or(Error::Dimensions)?;
                        let mut encoded = std::io::Cursor::new(Vec::new());
                        image
                            .write_to(
                                &mut encoded,
                                if ext == "bmp" {
                                    image::ImageFormat::Bmp
                                } else {
                                    image::ImageFormat::Jpeg
                                },
                            )
                            .map_err(|error| Error::Image(error.to_string()))?;
                        encoded.into_inner()
                    }
                }
                "pdf" => {
                    let mut options = svg2pdf::usvg::Options::default();
                    options.fontdb_mut().load_system_fonts();
                    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
                    let tree = svg2pdf::usvg::Tree::from_str(&svg, &options)
                        .map_err(|e| Error::Svg(e.to_string()))?;
                    let pdf = svg2pdf::to_pdf(
                        &tree,
                        svg2pdf::ConversionOptions::default(),
                        svg2pdf::PageOptions { dpi: 96. },
                    )
                    .map_err(|e| Error::Pdf(e.to_string()))?;
                    pdf_pages.push(pdf);
                    continue;
                }
                _ => return Err(Error::UnsupportedFormat(name.into())),
            };
            result.push(GraphicsFile {
                name: out,
                contents,
            });
        }
        if ext == "pdf" {
            result.push(GraphicsFile {
                name: name.into(),
                contents: self.merge_pdf(pdf_pages)?,
            });
        }
        Ok(result)
    }
    pub fn write(&self, pages: &[GraphicsPage], path: &FilePath) -> Result<Vec<FilePath>> {
        let files = self.generate(pages, path.as_str())?;
        let mut written = Vec::new();
        for file in files {
            let path = FilePath::new(file.name).ok_or(Error::Dimensions)?;
            file_utils::write_file(&path, &file.contents)?;
            written.push(path);
        }
        Ok(written)
    }
    /// Computes a tight visible-content crop, then applies page transform,
    /// orientation, margins and either explicit scale or fit-to-page.
    pub fn page_svg(&self, page: &GraphicsPage) -> Result<(String, f64, f64)> {
        let layout = self.page_layout(page)?;
        Ok((layout.svg, layout.width, layout.height))
    }
    fn page_layout(&self, page: &GraphicsPage) -> Result<PageLayout> {
        let settings = &page.settings;
        if settings.dpi == 0 || settings.margins.iter().any(|m| !m.is_finite() || *m < 0.) {
            return Err(Error::Dimensions);
        }
        let body = page.drawing.body();
        let source = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"1000\" height=\"1000\"><g id=\"content\">{body}</g></svg>"
        );
        let tree = resvg::usvg::Tree::from_str(&source, &svg_options())
            .map_err(|e| Error::Svg(e.to_string()))?;
        let bbox = tree
            .node_by_id("content")
            .map(|g| g.abs_stroke_bounding_box())
            .map(|r| {
                Rect::new(
                    f64::from(r.left()),
                    f64::from(r.top()),
                    f64::from(r.right()),
                    f64::from(r.bottom()),
                )
            })
            .unwrap_or(Rect::new(0., 0., 1., 1.));
        // Ensure even a page containing only a point/line has a positive extent.
        let bbox = if bbox.width() <= 0. || bbox.height() <= 0. {
            bbox.inflate(0.5, 0.5)
        } else {
            bbox
        };
        let rotation = if settings.rotate {
            Affine::new([0., -1., 1., 0., 0., 0.])
        } else {
            Affine::IDENTITY
        };
        let mirror = if settings.mirror {
            Affine::FLIP_X
        } else {
            Affine::IDENTITY
        };
        let size_scale = settings.scale.unwrap_or(1.);
        if !size_scale.is_finite() || size_scale <= 0. {
            return Err(Error::Dimensions);
        }
        let source_transform = rotation * mirror * Affine::scale(size_scale);
        let transformed = source_transform.transform_rect_bbox(bbox);
        let [left, top, right, bottom] = settings.margins;
        let paper = settings.paper_size()?;
        let (mut w, mut h) = paper.unwrap_or((
            transformed.width() + left + right,
            transformed.height() + top + bottom,
        ));
        let landscape = match settings.orientation {
            PageOrientation::Auto => transformed.width() >= transformed.height(),
            PageOrientation::Landscape => true,
            PageOrientation::Portrait => false,
        };
        if (w >= h) != landscape {
            std::mem::swap(&mut w, &mut h);
        }
        let available = (w - left - right, h - top - bottom);
        if !w.is_finite() || !h.is_finite() || available.0 <= 0. || available.1 <= 0. {
            return Err(Error::Dimensions);
        }
        let fit = if settings.scale.is_some() {
            1.
        } else {
            (available.0 / transformed.width()).min(available.1 / transformed.height())
        };
        let center = bbox.center();
        let transform = Affine::translate((left + available.0 / 2., top + available.1 / 2.))
            * source_transform
            * Affine::scale(fit)
            * Affine::translate((-center.x, -center.y));
        let c = transform.as_coeffs();
        let background = if settings.background.a > 0 {
            format!(
                "<rect width=\"{w}\" height=\"{h}\" fill=\"#{:02x}{:02x}{:02x}\" fill-opacity=\"{}\"/>",
                settings.background.r,
                settings.background.g,
                settings.background.b,
                f64::from(settings.background.a) / 255.
            )
        } else {
            String::new()
        };
        let title = svg::node::element::Title::new(self.title.clone());
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">{title}{background}<g transform=\"matrix({} {} {} {} {} {})\">{body}</g></svg>",
            c[0], c[1], c[2], c[3], c[4], c[5]
        );
        Ok(PageLayout {
            svg,
            width: w,
            height: h,
            transform,
        })
    }
    fn merge_pdf(&self, buffers: Vec<Vec<u8>>) -> Result<Vec<u8>> {
        use lopdf::{Document, Object, dictionary};
        let mut output = Document::with_version("1.7");
        let pages_id = output.new_object_id();
        let mut pages = Vec::new();
        for bytes in buffers {
            let mut document = Document::load_mem(&bytes).map_err(|e| Error::Pdf(e.to_string()))?;
            document.renumber_objects_with(output.max_id + 1);
            let ids: Vec<_> = document.get_pages().into_values().collect();
            for id in ids {
                document
                    .get_object_mut(id)
                    .and_then(Object::as_dict_mut)
                    .map_err(|e| Error::Pdf(e.to_string()))?
                    .set("Parent", pages_id);
                pages.push(Object::Reference(id));
            }
            output.max_id = document.max_id;
            output.objects.extend(document.objects);
        }
        let count = i64::try_from(pages.len()).map_err(|_| Error::Dimensions)?;
        output.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {"Type"=>"Pages","Kids"=>pages,"Count"=>count}),
        );
        let catalog = output.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages_id});
        output.trailer.set("Root", catalog);
        let text = |s: &str| {
            let mut bytes = vec![0xfe, 0xff];
            for c in s.encode_utf16() {
                bytes.extend(c.to_be_bytes());
            }
            Object::string_literal(bytes)
        };
        let info = output
            .add_object(dictionary! {"Title"=>text(&self.title),"Creator"=>text(&self.creator)});
        output.trailer.set("Info", info);
        output.prune_objects();
        let mut bytes = Vec::new();
        output
            .save_to(&mut bytes)
            .map_err(|e| Error::Pdf(e.to_string()))?;
        Ok(bytes)
    }
}
