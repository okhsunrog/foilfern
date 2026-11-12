//! Conventional outlines for two-pin placeholders in an embedded project library.

use librepcb_core::geometry::{Path, Polygon};
use librepcb_core::library::{LibraryBaseElement, sym::Symbol};
use librepcb_core::project::SymbolArtwork;
use librepcb_core::types::{Angle, Layer, Length, Point, UnsignedLength, Uuid};

use crate::{Error, Result};

/// Artwork templates that retain the original pin interface.
#[derive(Debug, Clone, Copy)]
pub enum SymbolStyle {
    Capacitor,
    NormallyOpenPushButton,
}

impl SymbolStyle {
    /// Replaces a plain rectangular outline. If the symbol already has the
    /// desired outline, preserves its artwork UUIDs and returns no edit.
    pub fn plan(self, symbol: &Symbol) -> Result<Option<SymbolArtwork>> {
        let id = symbol.metadata().uuid();
        let mut pins = symbol.pins().iter().collect::<Vec<_>>();
        pins.sort_by_key(|pin| pin.position().x);
        let [left, right] = pins.as_slice() else {
            return Err(unsupported(symbol, "expected exactly two pins"));
        };
        let ends = left
            .position()
            .x
            .to_nm()
            .checked_add(left.length().to_nm())
            .zip(
                right
                    .position()
                    .x
                    .to_nm()
                    .checked_sub(right.length().to_nm()),
            );
        let Some((x0, x1)) = ends else {
            return Err(unsupported(
                symbol,
                "pin line extends outside coordinate range",
            ));
        };
        let y = left.position().y.to_nm();
        if left.rotation().mapped_to_0_360deg() != Angle::DEG0
            || right.rotation().mapped_to_0_360deg() != Angle::DEG180
            || left.position().y != right.position().y
            || i128::from(x1) - i128::from(x0) < 7_620_000
            || y.checked_add(3_810_000).is_none()
            || y.checked_sub(3_810_000).is_none()
        {
            return Err(unsupported(
                symbol,
                "expected opposing horizontal pins on the same axis with room between their ends",
            ));
        }

        let middle = x0.midpoint(x1);
        let points = |vertices: &[(i64, i64)]| {
            Path::new(
                vertices
                    .iter()
                    .map(|&(x, dy)| Point::from_nm(x, y + dy).into())
                    .collect(),
            )
        };
        let paths = match self {
            Self::Capacitor => {
                let a = middle - 635_000;
                let b = middle + 635_000;
                vec![
                    points(&[(x0, 0), (a, 0)]),
                    points(&[(a, -1_905_000), (a, 1_905_000)]),
                    points(&[(b, -1_905_000), (b, 1_905_000)]),
                    points(&[(b, 0), (x1, 0)]),
                ]
            }
            Self::NormallyOpenPushButton => {
                let a = middle - 1_905_000;
                let b = middle + 1_905_000;
                vec![
                    points(&[(x0, 0), (a, 0), (a, 508_000)]),
                    points(&[(x1, 0), (b, 0), (b, 508_000)]),
                    points(&[(a, 1_524_000), (b, 1_524_000)]),
                    points(&[(middle, 1_524_000), (middle, 3_048_000)]),
                    points(&[(middle - 635_000, 3_048_000), (middle + 635_000, 3_048_000)]),
                ]
            }
        };
        let width = UnsignedLength::new(Length::new(254_000)).expect("constant is nonnegative");
        let existing = symbol.polygons().as_slice();
        let has_other_artwork = !symbol.circles().is_empty() || !symbol.images().is_empty();
        if !has_other_artwork
            && existing.len() == paths.len()
            && existing.iter().zip(&paths).all(|(polygon, path)| {
                polygon.layer() == Layer::SYMBOL_OUTLINES
                    && !polygon.is_filled()
                    && !polygon.is_grab_area()
                    && polygon.line_width() == width
                    && polygon.path() == path
            })
        {
            return Ok(None);
        }

        let [outline] = existing else {
            return Err(unsupported(
                symbol,
                "expected a plain rectangular placeholder",
            ));
        };
        if has_other_artwork || !is_box_outline(outline, x0, x1, y) {
            return Err(unsupported(
                symbol,
                "expected a plain rectangular placeholder",
            ));
        }
        let ids = std::iter::once(outline.uuid()).chain(std::iter::repeat_with(Uuid::new_random));
        let polygons = ids
            .zip(paths)
            .map(|(uuid, path)| {
                Polygon::new(uuid, Layer::SYMBOL_OUTLINES, width, false, false, path)
            })
            .collect();
        Ok(Some(SymbolArtwork {
            symbol: id,
            polygons,
            pin_name_positions: [(*left, 1_270_000), (*right, -1_270_000)]
                .into_iter()
                .filter(|(pin, _)| pin.name_position().y == Length::ZERO)
                .map(|(pin, offset)| {
                    (
                        pin.uuid(),
                        Point::new(pin.name_position().x, Length::new(offset)),
                    )
                })
                .collect(),
        }))
    }
}

fn unsupported(symbol: &Symbol, reason: &str) -> Error {
    Error::InvalidArgument(format!(
        "Cannot style symbol {}: {reason}.",
        symbol.metadata().uuid()
    ))
}

fn is_box_outline(outline: &Polygon, left: i64, right: i64, axis_y: i64) -> bool {
    if outline.layer() != Layer::SYMBOL_OUTLINES || outline.is_filled() {
        return false;
    }
    let vertices = outline.path().vertices();
    let [first, second, third, fourth, last] = vertices else {
        return false;
    };
    let bottom = first
        .pos
        .y
        .min(second.pos.y)
        .min(third.pos.y)
        .min(fourth.pos.y)
        .to_nm();
    let top = first
        .pos
        .y
        .max(second.pos.y)
        .max(third.pos.y)
        .max(fourth.pos.y)
        .to_nm();
    first == last
        && bottom < axis_y
        && axis_y < top
        && vertices.iter().all(|vertex| {
            vertex.angle == Angle::DEG0
                && [left, right].contains(&vertex.pos.x.to_nm())
                && [bottom, top].contains(&vertex.pos.y.to_nm())
        })
        && first.pos.x != third.pos.x
        && first.pos.y != third.pos.y
        && second.pos.x != fourth.pos.x
        && second.pos.y != fourth.pos.y
        && vertices
            .windows(2)
            .all(|edge| (edge[0].pos.x == edge[1].pos.x) != (edge[0].pos.y == edge[1].pos.y))
}
