//! Conventional outlines for rectangular placeholders in an embedded project library.

use librepcb_core::geometry::{Path, Polygon};
use librepcb_core::library::{
    LibraryBaseElement,
    sym::{Symbol, SymbolPin},
};
use librepcb_core::project::SymbolArtwork;
use librepcb_core::types::{Angle, Layer, Length, Point, UnsignedLength, Uuid};

use crate::{Error, Result};

/// Artwork templates that retain the original pin interface.
#[derive(Debug, Clone, Copy)]
pub enum SymbolStyle {
    Capacitor,
    NormallyOpenPushButton,
    Crystal {
        resonator_pins: [Uuid; 2],
        case_pins: [Uuid; 2],
    },
}

impl SymbolStyle {
    /// Replaces a plain rectangular outline. If the symbol already has the
    /// desired outline, preserves its artwork UUIDs and returns no edit.
    pub fn plan(self, symbol: &Symbol) -> Result<Option<SymbolArtwork>> {
        let id = symbol.metadata().uuid();
        let (main, case) = match self {
            Self::Crystal {
                resonator_pins,
                case_pins,
            } => {
                let [a, b] = resonator_pins;
                let [c, d] = case_pins;
                if symbol.pins().len() != 4
                    || a == b
                    || c == d
                    || case_pins.contains(&a)
                    || case_pins.contains(&b)
                {
                    return Err(unsupported(
                        symbol,
                        "expected four distinct pins in the crystal roles",
                    ));
                }
                let resonator = HorizontalPair::by_id(symbol, resonator_pins)?;
                let case = HorizontalPair::by_id(symbol, case_pins)?;
                if (resonator.x0, resonator.x1) != (case.x0, case.x1)
                    || i128::from(resonator.y) - i128::from(case.y) < 2_540_000
                {
                    return Err(unsupported(
                        symbol,
                        "expected aligned case pins below the resonator pins",
                    ));
                }
                (resonator, Some(case))
            }
            Self::Capacitor | Self::NormallyOpenPushButton => {
                let [left, right] = symbol.pins().as_slice() else {
                    return Err(unsupported(symbol, "expected exactly two pins"));
                };
                (HorizontalPair::new(symbol, left, right)?, None)
            }
        };
        let HorizontalPair {
            left,
            right,
            x0,
            x1,
            y,
        } = main;

        let middle = x0.midpoint(x1);
        let points = |vertices: &[(i64, i64)]| path_at_y(y, vertices);
        let mut paths = match self {
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
            Self::Crystal { .. } => {
                let a = middle - 1_905_000;
                let b = middle + 1_905_000;
                vec![
                    points(&[(x0, 0), (a, 0)]),
                    points(&[(a, -1_524_000), (a, 1_524_000)]),
                    Path::rect(
                        Point::from_nm(middle - 1_016_000, y - 1_143_000),
                        Point::from_nm(middle + 1_016_000, y + 1_143_000),
                    ),
                    points(&[(b, -1_524_000), (b, 1_524_000)]),
                    points(&[(b, 0), (x1, 0)]),
                ]
            }
        };
        if let Some(case) = &case {
            // Leave a gap so the artwork never implies an internal connection
            // between case terminals. The schematic wires carry that connection.
            paths.extend([
                path_at_y(
                    case.y,
                    &[
                        (x0, 0),
                        (middle - 3_175_000, 0),
                        (middle - 3_175_000, -762_000),
                        (middle - 635_000, -762_000),
                    ],
                ),
                path_at_y(
                    case.y,
                    &[
                        (x1, 0),
                        (middle + 3_175_000, 0),
                        (middle + 3_175_000, -762_000),
                        (middle + 635_000, -762_000),
                    ],
                ),
            ]);
        }
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
        if has_other_artwork
            || !is_box_outline(outline, x0, x1, case.as_ref().map_or(y, |p| p.y), y)
        {
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
            pin_name_positions: [(left, 1_270_000), (right, -1_270_000)]
                .into_iter()
                .chain(
                    case.iter()
                        .flat_map(|p| [(p.left, -1_016_000), (p.right, 1_016_000)]),
                )
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

struct HorizontalPair<'a> {
    left: &'a SymbolPin,
    right: &'a SymbolPin,
    x0: i64,
    x1: i64,
    y: i64,
}

impl<'a> HorizontalPair<'a> {
    fn by_id(symbol: &'a Symbol, [a, b]: [Uuid; 2]) -> Result<Self> {
        let pin = |id| {
            symbol
                .pins()
                .by_uuid(&id)
                .ok_or_else(|| unsupported(symbol, &format!("pin {id} is missing")))
        };
        Self::new(symbol, pin(a)?, pin(b)?)
    }

    fn new(symbol: &Symbol, a: &'a SymbolPin, b: &'a SymbolPin) -> Result<Self> {
        let (left, right) = if a.position().x < b.position().x {
            (a, b)
        } else {
            (b, a)
        };
        let x0 = left.position().x.to_nm().checked_add(left.length().to_nm());
        let x1 = right
            .position()
            .x
            .to_nm()
            .checked_sub(right.length().to_nm());
        let Some((x0, x1)) = x0.zip(x1) else {
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
        Ok(Self {
            left,
            right,
            x0,
            x1,
            y,
        })
    }
}

fn path_at_y(y: i64, vertices: &[(i64, i64)]) -> Path {
    Path::new(
        vertices
            .iter()
            .map(|&(x, dy)| Point::from_nm(x, y + dy).into())
            .collect(),
    )
}

fn unsupported(symbol: &Symbol, reason: &str) -> Error {
    Error::InvalidArgument(format!(
        "Cannot style symbol {}: {reason}.",
        symbol.metadata().uuid()
    ))
}

fn is_box_outline(
    outline: &Polygon,
    left: i64,
    right: i64,
    low_pin_y: i64,
    high_pin_y: i64,
) -> bool {
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
        && bottom < low_pin_y
        && high_pin_y < top
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

#[cfg(test)]
mod tests {
    use super::*;
    use librepcb_core::library::BaseMetadata;
    use librepcb_core::types::{CircuitIdentifier, ElementName};

    #[test]
    fn crystal_requires_distinct_roles_and_preserves_existing_artwork_on_replay() {
        let mut symbol = Symbol::new(BaseMetadata::new(
            Uuid::new_random(),
            "0.1".parse().unwrap(),
            "test",
            "2026-01-01T00:00:00Z".parse().unwrap(),
            ElementName::new("Test Crystal").unwrap(),
            "",
            "",
        ))
        .unwrap();
        let [a, b, c, d] = [(); 4].map(|()| Uuid::new_random());
        for (index, (id, x, y, rotation)) in [
            (a, -10_160_000, 1_270_000, Angle::DEG0),
            (b, 10_160_000, 1_270_000, Angle::DEG180),
            (c, -10_160_000, -1_270_000, Angle::DEG0),
            (d, 10_160_000, -1_270_000, Angle::DEG180),
        ]
        .into_iter()
        .enumerate()
        {
            symbol.pins_mut().push(SymbolPin::new(
                id,
                CircuitIdentifier::new(format!("P{index}")).unwrap(),
                Point::from_nm(x, y),
                UnsignedLength::new(Length::new(2_540_000)).unwrap(),
                rotation,
                Point::ORIGIN,
                Angle::DEG0,
                SymbolPin::default_name_height(),
                SymbolPin::default_name_alignment(),
            ));
        }
        let outline_id = Uuid::new_random();
        symbol.polygons_mut().push(Polygon::new(
            outline_id,
            Layer::SYMBOL_OUTLINES,
            UnsignedLength::new(Length::new(254_000)).unwrap(),
            false,
            true,
            Path::rect(
                Point::from_nm(-7_620_000, -2_540_000),
                Point::from_nm(7_620_000, 2_540_000),
            ),
        ));

        for invalid in [
            SymbolStyle::Crystal {
                resonator_pins: [a, b],
                case_pins: [a, d],
            },
            SymbolStyle::Crystal {
                resonator_pins: [c, d],
                case_pins: [a, b],
            },
            SymbolStyle::Crystal {
                resonator_pins: [a, b],
                case_pins: [c, Uuid::new_random()],
            },
        ] {
            assert!(invalid.plan(&symbol).is_err());
        }
        let artwork = SymbolStyle::Crystal {
            resonator_pins: [a, b],
            case_pins: [c, d],
        }
        .plan(&symbol)
        .unwrap()
        .unwrap();
        assert_eq!(artwork.polygons.first().unwrap().uuid(), outline_id);
        assert_eq!(artwork.polygons.len(), 7);
        assert_eq!(artwork.pin_name_positions.len(), 4);
        *symbol.polygons_mut() = artwork.polygons;
        assert!(
            SymbolStyle::Crystal {
                resonator_pins: [b, a],
                case_pins: [d, c],
            }
            .plan(&symbol)
            .unwrap()
            .is_none()
        );
    }
}
