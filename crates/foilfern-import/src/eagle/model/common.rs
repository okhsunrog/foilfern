//! Port of parseagle/common/* (libs/parseagle): the geometry elements shared
//! by symbols, packages, schematics and boards.
//!
//! The elements are plain data with public fields (upstream: getters),
//! constructed from a [`DomElement`] with `from_dom()`. Parse errors of
//! enum attributes are appended to `errors` (upstream `QStringList*`).

use super::super::error::Result;
use super::dom::{DomElement, parse_double};
use super::enums::{
    Alignment, AttributeDisplay, Font, GridStyle, GridUnit, PolygonPour, WireCap, WireStyle,
};

/// A point in millimeters.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    /// X coordinate \[mm\].
    pub x: f64,
    /// Y coordinate \[mm\].
    pub y: f64,
}

impl Point {
    fn from_attrs(e: &DomElement, x: &str, y: &str) -> Result<Self> {
        Ok(Self {
            x: e.attr_double(x)?,
            y: e.attr_double(y)?,
        })
    }
}

/// A rotation like `"MR90"` or `"SR180"`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rotation {
    /// Spin flag (`S`, text is not kept upright).
    pub spin: bool,
    /// Mirror flag (`M`).
    pub mirror: bool,
    /// Angle \[°\].
    pub angle: f64,
}

impl Rotation {
    /// Parses a rotation string (an invalid angle gives 0°, like upstream
    /// `QString::toDouble()`).
    pub fn parse(s: &str) -> Self {
        Self {
            spin: s.contains('S'),
            mirror: s.contains('M'),
            angle: parse_double(&s.replace(['M', 'S', 'R'], "")).unwrap_or(0.0),
        }
    }

    fn opt(e: &DomElement) -> Self {
        e.opt_str("rot").map(Self::parse).unwrap_or_default()
    }
}

/// An attribute (`<attribute>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    /// Name.
    pub name: String,
    /// Value.
    pub value: String,
    /// Text position.
    pub position: Point,
    /// Text size \[mm\].
    pub size: f64,
    /// Layer ID.
    pub layer: i32,
    /// Font.
    pub font: Font,
    /// Stroke width ratio \[%\].
    pub ratio: i32,
    /// Rotation.
    pub rotation: Rotation,
    /// Display mode.
    pub display: AttributeDisplay,
    /// Constant flag.
    pub constant: bool,
    /// Alignment.
    pub alignment: Alignment,
}

impl Attribute {
    /// Parses an `<attribute>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            value: e.opt_str("value").unwrap_or_default().to_owned(),
            position: Point {
                x: e.opt_double("x")?.unwrap_or(0.0),
                y: e.opt_double("y")?.unwrap_or(0.0),
            },
            size: e.opt_double("size")?.unwrap_or(0.0),
            layer: e.opt_int("layer")?.unwrap_or(0),
            font: e
                .opt_str("font")
                .map_or(Font::Unknown, |s| Font::parse(s, errors)),
            ratio: e.opt_int("ratio")?.unwrap_or(0),
            rotation: Rotation::opt(e),
            display: e.opt_str("display").map_or(AttributeDisplay::Value, |s| {
                AttributeDisplay::parse(s, errors)
            }),
            constant: e.opt_bool("constant")?.unwrap_or(false),
            alignment: e
                .opt_str("align")
                .map_or(Alignment::BottomLeft, |s| Alignment::parse(s, errors)),
        })
    }
}

/// A circle (`<circle>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Circle {
    /// Layer ID.
    pub layer: i32,
    /// Line width \[mm\] (0 = filled).
    pub width: f64,
    /// Radius \[mm\].
    pub radius: f64,
    /// Center.
    pub position: Point,
}

impl Circle {
    /// Parses a `<circle>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            layer: e.attr_int("layer")?,
            width: e.attr_double("width")?,
            radius: e.attr_double("radius")?,
            position: Point::from_attrs(e, "x", "y")?,
        })
    }
}

/// A drawing frame (`<frame>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Layer ID.
    pub layer: i32,
    /// First corner.
    pub p1: Point,
    /// Second corner.
    pub p2: Point,
    /// Number of columns.
    pub columns: i32,
    /// Number of rows.
    pub rows: i32,
}

impl Frame {
    /// Parses a `<frame>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            layer: e.attr_int("layer")?,
            p1: Point::from_attrs(e, "x1", "y1")?,
            p2: Point::from_attrs(e, "x2", "y2")?,
            columns: e.attr_int("columns")?,
            rows: e.attr_int("rows")?,
        })
    }
}

/// Grid settings (`<grid>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    /// Grid distance (in `unit_distance`).
    pub distance: f64,
    /// Unit of `distance`.
    pub unit_distance: GridUnit,
    /// Display unit.
    pub unit: GridUnit,
    /// Style.
    pub style: GridStyle,
    /// Multiple.
    pub multiple: i32,
    /// Whether the grid is displayed.
    pub display: bool,
    /// Alternative distance.
    pub alt_distance: f64,
    /// Unit of `alt_distance`.
    pub alt_unit_distance: GridUnit,
    /// Alternative display unit.
    pub alt_unit: GridUnit,
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            distance: 0.0,
            unit_distance: GridUnit::Unknown,
            unit: GridUnit::Unknown,
            style: GridStyle::Lines,
            multiple: 1,
            display: false,
            alt_distance: 0.0,
            alt_unit_distance: GridUnit::Unknown,
            alt_unit: GridUnit::Unknown,
        }
    }
}

impl Grid {
    /// Parses a `<grid>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let d = Self::default();
        let unit = |name: &str, errors: &mut Vec<String>| {
            e.opt_str(name)
                .map_or(GridUnit::Unknown, |s| GridUnit::parse(s, errors))
        };
        Ok(Self {
            distance: e.opt_double("distance")?.unwrap_or(d.distance),
            unit_distance: unit("unitdist", errors),
            unit: unit("unit", errors),
            style: e
                .opt_str("style")
                .map_or(d.style, |s| GridStyle::parse(s, errors)),
            multiple: e.opt_int("multiple")?.unwrap_or(d.multiple),
            display: e.opt_bool("display")?.unwrap_or(d.display),
            alt_distance: e.opt_double("altdistance")?.unwrap_or(d.alt_distance),
            alt_unit_distance: unit("altunitdist", errors),
            alt_unit: unit("altunit", errors),
        })
    }
}

/// A polygon vertex (`<vertex>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Vertex {
    /// Position.
    pub position: Point,
    /// Arc angle to the next vertex \[°\].
    pub curve: f64,
}

impl Vertex {
    /// Parses a `<vertex>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            position: Point::from_attrs(e, "x", "y")?,
            curve: e.opt_double("curve")?.unwrap_or(0.0),
        })
    }
}

/// A polygon (`<polygon>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Polygon {
    /// Layer ID.
    pub layer: i32,
    /// Line width \[mm\].
    pub width: f64,
    /// Hatch spacing \[mm\].
    pub spacing: f64,
    /// Pour mode.
    pub pour: PolygonPour,
    /// Isolation \[mm\].
    pub isolate: f64,
    /// Whether orphans are kept.
    pub orphans: bool,
    /// Whether thermals are generated.
    pub thermals: bool,
    /// Rank.
    pub rank: i32,
    /// Vertices.
    pub vertices: Vec<Vertex>,
}

impl Polygon {
    /// Parses a `<polygon>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            layer: e.attr_int("layer")?,
            width: e.attr_double("width")?,
            spacing: e.opt_double("spacing")?.unwrap_or(0.0),
            pour: e
                .opt_str("pour")
                .map_or(PolygonPour::Solid, |s| PolygonPour::parse(s, errors)),
            isolate: e.opt_double("isolate")?.unwrap_or(0.0),
            orphans: e.opt_bool("orphans")?.unwrap_or(false),
            thermals: e.opt_bool("thermals")?.unwrap_or(true),
            rank: e.opt_int("rank")?.unwrap_or(0),
            vertices: e
                .children()
                .iter()
                .map(Vertex::from_dom)
                .collect::<Result<_>>()?,
        })
    }
}

/// A rectangle (`<rectangle>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Rectangle {
    /// Layer ID.
    pub layer: i32,
    /// First corner.
    pub p1: Point,
    /// Second corner.
    pub p2: Point,
    /// Rotation (around the center).
    pub rotation: Rotation,
}

impl Rectangle {
    /// Parses a `<rectangle>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            layer: e.attr_int("layer")?,
            p1: Point::from_attrs(e, "x1", "y1")?,
            p2: Point::from_attrs(e, "x2", "y2")?,
            rotation: Rotation::opt(e),
        })
    }
}

/// A text (`<text>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// Layer ID.
    pub layer: i32,
    /// Font.
    pub font: Font,
    /// Size \[mm\].
    pub size: f64,
    /// Stroke width ratio \[%\].
    pub ratio: i32,
    /// Position.
    pub position: Point,
    /// Rotation.
    pub rotation: Rotation,
    /// Alignment.
    pub alignment: Alignment,
    /// Text.
    pub value: String,
}

impl Text {
    /// Parses a `<text>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            layer: e.attr_int("layer")?,
            font: e
                .opt_str("font")
                .map_or(Font::Proportional, |s| Font::parse(s, errors)),
            size: e.attr_double("size")?,
            ratio: e.opt_int("ratio")?.unwrap_or(8),
            position: Point::from_attrs(e, "x", "y")?,
            rotation: Rotation::opt(e),
            alignment: e
                .opt_str("align")
                .map_or(Alignment::BottomLeft, |s| Alignment::parse(s, errors)),
            value: e.text().to_owned(),
        })
    }
}

/// A wire (line or arc, `<wire>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    /// Layer ID.
    pub layer: i32,
    /// Width \[mm\].
    pub width: f64,
    /// Start point.
    pub p1: Point,
    /// End point.
    pub p2: Point,
    /// Line style.
    pub wire_style: WireStyle,
    /// Arc angle \[°\].
    pub curve: f64,
    /// Cap style.
    pub wire_cap: WireCap,
}

impl Wire {
    /// Parses a `<wire>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            layer: e.attr_int("layer")?,
            width: e.attr_double("width")?,
            p1: Point::from_attrs(e, "x1", "y1")?,
            p2: Point::from_attrs(e, "x2", "y2")?,
            wire_style: e
                .opt_str("style")
                .map_or(WireStyle::Continuous, |s| WireStyle::parse(s, errors)),
            curve: e.opt_double("curve")?.unwrap_or(0.0),
            wire_cap: e
                .opt_str("cap")
                .map_or(WireCap::Round, |s| WireCap::parse(s, errors)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rotation() {
        let r = Rotation::parse("SMR90");
        assert!(r.spin && r.mirror);
        assert_eq!(r.angle, 90.0);
        assert_eq!(Rotation::parse("Rfoo").angle, 0.0);
    }
}
