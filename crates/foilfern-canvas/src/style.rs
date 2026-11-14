//! How items are painted: fill and stroke with layer or fixed colors.

use kurbo::{Cap, Join};
use peniko::Color;

/// The color source of a fill or stroke.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Brush {
    /// The color of the item's layer (its highlight color when the item is
    /// selected). Changing the layer color repaints without re-encoding.
    #[default]
    Layer,
    /// A fixed color (selected items still use the layer highlight color).
    Solid(Color),
}

/// A dash pattern in world units (millimetres).
#[derive(Debug, Clone, PartialEq)]
pub struct Dash {
    /// Alternating dash and gap lengths.
    pub pattern: Vec<f64>,
    /// Offset into the pattern at the start of each subpath.
    pub offset: f64,
}

/// Stroke parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeStyle {
    /// Color source.
    pub brush: Brush,
    /// Width in world units (millimetres). `0.0` is a hairline of one
    /// logical pixel at every zoom level (like a cosmetic `QPen`).
    pub width: f64,
    /// Line caps (both ends).
    pub cap: Cap,
    /// Line joins.
    pub join: Join,
    /// Optional dash pattern.
    pub dash: Option<Dash>,
}

impl StrokeStyle {
    /// A solid stroke in the layer color with round caps and joins (what
    /// LibrePCB uses for all outlines, traces and stroke texts).
    pub fn new(width: f64) -> Self {
        Self {
            brush: Brush::Layer,
            width: width.max(0.0),
            cap: Cap::Round,
            join: Join::Round,
            dash: None,
        }
    }

    /// A hairline (one logical pixel wide at every zoom level).
    pub fn hairline() -> Self {
        Self::new(0.0)
    }

    /// Returns the stroke with another brush.
    pub fn with_brush(mut self, brush: Brush) -> Self {
        self.brush = brush;
        self
    }

    /// Returns the stroke with other caps.
    pub fn with_cap(mut self, cap: Cap) -> Self {
        self.cap = cap;
        self
    }

    /// Returns the stroke with other joins.
    pub fn with_join(mut self, join: Join) -> Self {
        self.join = join;
        self
    }

    /// Returns the stroke with a dash pattern (world units).
    pub fn with_dash(mut self, pattern: impl Into<Vec<f64>>, offset: f64) -> Self {
        self.dash = Some(Dash {
            pattern: pattern.into(),
            offset,
        });
        self
    }

    /// Returns whether this is a hairline.
    pub fn is_hairline(&self) -> bool {
        self.width <= 0.0
    }

    /// How far the stroke extends beyond the geometry (world units, without
    /// hairline pixels).
    pub(crate) fn extent(&self) -> f64 {
        let half = self.width / 2.0;
        let factor = match (self.join, self.cap) {
            // kurbo's default miter limit is 4.
            (Join::Miter, _) => 4.0,
            (_, Cap::Square) => std::f64::consts::SQRT_2,
            _ => 1.0,
        };
        half * factor
    }

    /// The kurbo stroke for a given effective width.
    pub(crate) fn to_kurbo(&self, width: f64) -> kurbo::Stroke {
        let mut stroke = kurbo::Stroke::new(width)
            .with_caps(self.cap)
            .with_join(self.join);
        if let Some(dash) = &self.dash
            && dash.pattern.iter().any(|d| *d > 0.0)
        {
            stroke = stroke.with_dashes(dash.offset, dash.pattern.iter().copied());
        }
        stroke
    }
}

/// Fill and/or stroke of an item.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Style {
    /// Fill (non-zero winding; every closed subpath of a filled item is
    /// added to the filled area, see [`Scene::insert`](crate::Scene::insert)).
    pub fill: Option<Brush>,
    /// Stroke.
    pub stroke: Option<StrokeStyle>,
}

impl Style {
    /// Filled with the layer color.
    pub fn fill() -> Self {
        Self {
            fill: Some(Brush::Layer),
            stroke: None,
        }
    }

    /// Stroked with the layer color, round caps and joins.
    pub fn stroke(width: f64) -> Self {
        Self {
            fill: None,
            stroke: Some(StrokeStyle::new(width)),
        }
    }

    /// Filled and stroked with the layer color.
    pub fn fill_and_stroke(width: f64) -> Self {
        Self {
            fill: Some(Brush::Layer),
            stroke: Some(StrokeStyle::new(width)),
        }
    }

    /// Returns the style with another fill.
    pub fn with_fill(mut self, fill: Option<Brush>) -> Self {
        self.fill = fill;
        self
    }

    /// Returns the style with another stroke.
    pub fn with_stroke(mut self, stroke: Option<StrokeStyle>) -> Self {
        self.stroke = stroke;
        self
    }

    /// How far the painted area extends beyond the geometry (world units).
    pub(crate) fn extent(&self) -> f64 {
        self.stroke.as_ref().map_or(0.0, StrokeStyle::extent)
    }
}

/// One paint operation of an item: its fill or its stroke. Items are
/// grouped by pass, so passes are interned and compared by value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum PassKey {
    Fill(BrushKey),
    Stroke(BrushKey, StrokeKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum BrushKey {
    Layer,
    Solid([u32; 4]),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct StrokeKey {
    width: u64,
    cap: u8,
    join: u8,
    dash: Option<(Vec<u64>, u64)>,
}

impl BrushKey {
    pub(crate) fn new(brush: Brush) -> Self {
        match brush {
            Brush::Layer => Self::Layer,
            Brush::Solid(c) => Self::Solid(c.components.map(f32::to_bits)),
        }
    }
}

impl StrokeKey {
    pub(crate) fn new(s: &StrokeStyle) -> Self {
        let cap = match s.cap {
            Cap::Butt => 0,
            Cap::Square => 1,
            Cap::Round => 2,
        };
        let join = match s.join {
            Join::Bevel => 0,
            Join::Miter => 1,
            Join::Round => 2,
        };
        Self {
            width: s.width.to_bits(),
            cap,
            join,
            dash: s.dash.as_ref().map(|d| {
                (
                    d.pattern.iter().map(|v| v.to_bits()).collect(),
                    d.offset.to_bits(),
                )
            }),
        }
    }
}

/// An interned pass: the key plus what the renderer needs to paint it.
#[derive(Debug, Clone)]
pub(crate) struct Pass {
    pub(crate) brush: Brush,
    /// `None` for fills.
    pub(crate) stroke: Option<StrokeStyle>,
}

impl Pass {
    pub(crate) fn is_fill(&self) -> bool {
        self.stroke.is_none()
    }
}
