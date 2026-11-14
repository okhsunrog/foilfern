//! Toolkit-independent Vello rasterization of ordered compound material paths.
//! Unlike scene items, subpath winding must remain intact: each area can
//! contain nested cutouts, filled with the even-odd rule.
use super::Error;
use kurbo::{Affine, BezPath, Rect};
use peniko::{Color, Fill};
use vello_cpu::{PixmapMut, RenderContext, RenderSettings, Resources};

/// Renders ordered even-odd fills to premultiplied RGBA8. `transform` maps
/// path coordinates directly to device pixels (Y down). No window or GPU
/// context is required; callers retain the same paths for vector/GPU output.
pub fn rasterize_filled_paths<'a>(
    paths: impl IntoIterator<Item = (&'a BezPath, Color)>,
    transform: Affine,
    size: (u32, u32),
    background: Color,
    target: &mut [u8],
) -> Result<(), Error> {
    let (width, height) = size;
    let (Ok(w), Ok(h)) = (u16::try_from(width), u16::try_from(height)) else {
        return Err(Error::TooLarge { width, height });
    };
    let expected = usize::from(w)
        .checked_mul(usize::from(h))
        .and_then(|n| n.checked_mul(4))
        .ok_or(Error::TooLarge { width, height })?;
    if target.len() != expected {
        return Err(Error::TargetSize {
            expected,
            actual: target.len(),
        });
    }
    if w == 0 || h == 0 {
        return Ok(());
    }
    let mut context = RenderContext::new_with(w, h, RenderSettings::default());
    context.set_paint(background);
    context.fill_rect(&Rect::new(0., 0., f64::from(w), f64::from(h)));
    context.set_transform(transform);
    context.set_fill_rule(Fill::EvenOdd);
    for (path, color) in paths {
        context.set_paint(color);
        context.fill_path(path);
    }
    context.flush();
    let pixmap = PixmapMut::new(w, h, target).ok_or(Error::TargetSize {
        expected,
        actual: expected,
    })?;
    context.render(pixmap, &mut Resources::new());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rasterization_replaces_old_pixels_and_checks_sizes_before_rendering() {
        let mut target = [255; 16];
        rasterize_filled_paths(
            [],
            Affine::IDENTITY,
            (2, 2),
            Color::TRANSPARENT,
            &mut target,
        )
        .unwrap();
        assert_eq!(target, [0; 16]);
        assert!(matches!(
            rasterize_filled_paths([], Affine::IDENTITY, (2, 2), Color::BLACK, &mut []),
            Err(Error::TargetSize {
                expected: 16,
                actual: 0
            })
        ));
        assert!(matches!(
            rasterize_filled_paths([], Affine::IDENTITY, (65_536, 1), Color::BLACK, &mut []),
            Err(Error::TooLarge { .. })
        ));
        rasterize_filled_paths([], Affine::IDENTITY, (0, 2), Color::BLACK, &mut []).unwrap();
    }
}
