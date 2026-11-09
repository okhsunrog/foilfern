//! Image backend of the graphics export (upstream paints into a `QImage`
//! and saves it with `QImageWriter`): rasterized with vello_cpu, encoded
//! with the `image` crate.
//!
//! Like `QImage::Format_ARGB32_Premultiplied` saved to formats without
//! alpha channel (JPEG, BMP), transparent areas become black there.

use image::{ExtendedColorType, ImageFormat};
use librepcb_canvas::kurbo::Rect;
use librepcb_core::fileio::FilePath;
use vello_cpu::{PixmapMut, RenderContext, RenderSettings, Resources};

use super::{ExportPage, PageLayout, PrimitivePaint, kurbo_stroke};
/// A rendered page: premultiplied RGBA8, row-major.
pub(crate) struct Pixels {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

/// Largest supported image edge (vello_cpu uses 16 bit sizes).
const MAX_EDGE: u32 = u16::MAX as u32;
/// Largest supported image size in bytes (like `QImage`, 2 GiB).
const MAX_BYTES: u64 = i32::MAX as u64;

/// The image format of a (lower case) file extension.
pub(crate) fn format(ext: &str) -> Option<ImageFormat> {
    match ext {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "bmp" => Some(ImageFormat::Bmp),
        _ => None,
    }
}

/// Renders a page into a premultiplied RGBA image, `None` if the page is
/// too large.
pub(crate) fn render(page: &ExportPage, layout: &PageLayout) -> Option<Pixels> {
    let (w, h) = (
        layout.size.width.round() as u32,
        layout.size.height.round() as u32,
    );
    if w == 0
        || h == 0
        || w > MAX_EDGE
        || h > MAX_EDGE
        || u64::from(w) * u64::from(h) * 4 > MAX_BYTES
    {
        return None;
    }
    let (w16, h16) = (u16::try_from(w).ok()?, u16::try_from(h).ok()?);
    let mut ctx = RenderContext::new_with(w16, h16, RenderSettings::default());
    if layout.background.components[3] > 0.0 {
        ctx.set_paint(layout.background);
        ctx.fill_rect(&Rect::new(0.0, 0.0, f64::from(w), f64::from(h)));
    }
    ctx.set_transform(layout.transform);
    ctx.set_fill_rule(librepcb_canvas::peniko::Fill::NonZero);
    for primitive in page.drawing.primitives() {
        match &primitive.paint {
            PrimitivePaint::Fill(color) => {
                ctx.set_paint(*color);
                ctx.fill_path(&primitive.path);
            }
            PrimitivePaint::Stroke(color, style) => {
                ctx.set_paint(*color);
                ctx.set_stroke(kurbo_stroke(style, layout.min_line_width));
                ctx.stroke_path(&primitive.path);
            }
        }
    }
    ctx.flush();
    let mut data = vec![0u8; w as usize * h as usize * 4];
    let pixmap = PixmapMut::new(w16, h16, &mut data)?;
    ctx.render(pixmap, &mut Resources::new());
    Some(Pixels {
        width: w,
        height: h,
        data,
    })
}

/// Saves a premultiplied RGBA image.
pub(crate) fn save(image: &Pixels, format: ImageFormat, path: &FilePath) -> image::ImageResult<()> {
    let (data, color) = match format {
        ImageFormat::Png => {
            // Straight alpha.
            let mut data = image.data.clone();
            for px in data.as_chunks_mut::<4>().0 {
                let a = u32::from(px[3]);
                if a != 0 && a != 255 {
                    for c in &mut px[..3] {
                        *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
            (data, ExtendedColorType::Rgba8)
        }
        _ => {
            // Drop the alpha channel of the premultiplied pixels.
            let data = image
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|px| [px[0], px[1], px[2]])
                .collect();
            (data, ExtendedColorType::Rgb8)
        }
    };
    image::save_buffer_with_format(
        path.as_str(),
        &data,
        image.width,
        image.height,
        color,
        format,
    )
}
