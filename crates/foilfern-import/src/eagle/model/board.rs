//! Port of parseagle/board/* (libs/parseagle): EAGLE boards (`*.brd`).

use super::super::error::Result;
use super::common::{Attribute, Circle, Grid, Point, Polygon, Rectangle, Rotation, Text, Wire};
use super::dom::{DomElement, parse_double};
use super::enums::ViaShape;
use super::library::{Hole, Library};

fn point(e: &DomElement) -> Result<Point> {
    Ok(Point {
        x: e.attr_double("x")?,
        y: e.attr_double("y")?,
    })
}

/// A design rule parameter (`<param>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// Name.
    pub name: String,
    /// Value (e.g. `"0.3mm"` or `"0.25"`).
    pub value: String,
}

impl Param {
    /// Parses a `<param>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            value: e.attr_str("value")?.to_owned(),
        })
    }

    /// Returns the value as integer, if valid.
    pub fn value_as_int(&self) -> Option<i32> {
        self.value.trim().parse().ok()
    }

    /// Returns the value as floating point number, if valid.
    pub fn value_as_double(&self) -> Option<f64> {
        parse_double(&self.value)
    }

    /// Splits the value into number and (trailing letters) unit, e.g.
    /// `"0.3mm"` into `(0.3, "mm")`; `None` if the number is invalid.
    pub fn value_as_double_with_unit(&self) -> Option<(f64, String)> {
        let number = self.value.trim_end_matches(char::is_alphabetic);
        let unit = &self.value[number.len()..];
        parse_double(number).map(|v| (v, unit.to_owned()))
    }
}

/// Design rules (`<designrules>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DesignRules {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Parameters.
    pub params: Vec<Param>,
}

impl DesignRules {
    /// Parses a `<designrules>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        // Note: Although the EAGLE DTD files specify the "name" attribute as
        // required, there seem to be EAGLE boards without this attribute and
        // EAGLE opens them anyway. So we have to support this case too.
        let mut obj = Self {
            name: e.opt_str("name").unwrap_or_default().to_owned(),
            ..Self::default()
        };
        for child in e.children() {
            match child.tag_name() {
                "description" => obj.description = child.text().to_owned(),
                "param" => obj.params.push(Param::from_dom(child)?),
                other => errors.push(format!("Unknown design rules child: {other}")),
            }
        }
        Ok(obj)
    }

    /// Returns the parameter `name`, if it exists.
    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.name == name)
    }
}

/// A placed package (`<element>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// Name (designator).
    pub name: String,
    /// Library name.
    pub library: String,
    /// Library URN.
    pub library_urn: String,
    /// Package name.
    pub package: String,
    /// Value.
    pub value: String,
    /// Position.
    pub position: Point,
    /// Rotation.
    pub rotation: Rotation,
    /// Locked flag.
    pub locked: bool,
    /// Populate flag.
    pub populate: bool,
    /// Smashed flag.
    pub smashed: bool,
    /// Attributes.
    pub attributes: Vec<Attribute>,
}

impl Element {
    /// Parses an `<element>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut attributes = Vec::new();
        for child in e.children() {
            if child.tag_name() == "attribute" {
                attributes.push(Attribute::from_dom(child, errors)?);
            } else {
                errors.push(format!("Unknown element child: {}", child.tag_name()));
            }
        }
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            library: e.attr_str("library")?.to_owned(),
            library_urn: e.opt_str("library_urn").unwrap_or_default().to_owned(),
            package: e.attr_str("package")?.to_owned(),
            value: e.attr_str("value")?.to_owned(),
            position: point(e)?,
            rotation: e.opt_str("rot").map(Rotation::parse).unwrap_or_default(),
            locked: e.opt_bool("locked")?.unwrap_or(false),
            populate: e.opt_bool("populate")?.unwrap_or(true),
            smashed: e.opt_bool("smashed")?.unwrap_or(false),
            attributes,
        })
    }
}

/// A pad connected to a signal (`<contactref>`).
#[derive(Debug, Clone, PartialEq)]
pub struct ContactRef {
    /// Element name.
    pub element: String,
    /// Pad name.
    pub pad: String,
}

impl ContactRef {
    /// Parses a `<contactref>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            element: e.attr_str("element")?.to_owned(),
            pad: e.attr_str("pad")?.to_owned(),
        })
    }
}

/// A via (`<via>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Via {
    /// Position.
    pub position: Point,
    /// Layer extent (e.g. `"1-16"`).
    pub extent: String,
    /// Drill diameter \[mm\].
    pub drill: f64,
    /// Outer diameter \[mm\] (0 = auto).
    pub diameter: f64,
    /// Shape.
    pub shape: ViaShape,
    /// Always stop flag.
    pub always_stop: bool,
}

impl Via {
    /// Parses a `<via>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            position: point(e)?,
            extent: e.attr_str("extent")?.to_owned(),
            drill: e.attr_double("drill")?,
            diameter: e.opt_double("diameter")?.unwrap_or(0.0),
            shape: e
                .opt_str("shape")
                .map_or(ViaShape::Round, |s| ViaShape::parse(s, errors)),
            always_stop: e.opt_bool("alwaysstop")?.unwrap_or(false),
        })
    }

    /// Returns the start layer of the extent.
    pub fn start_layer(&self) -> Option<i32> {
        self.extent.split('-').next()?.parse().ok()
    }

    /// Returns the end layer of the extent.
    pub fn end_layer(&self) -> Option<i32> {
        self.extent.split('-').nth(1)?.parse().ok()
    }
}

/// A signal (net) of a board (`<signal>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Signal {
    /// Name.
    pub name: String,
    /// Net class.
    pub class: i32,
    /// Connected pads.
    pub contact_refs: Vec<ContactRef>,
    /// Planes.
    pub polygons: Vec<Polygon>,
    /// Traces.
    pub wires: Vec<Wire>,
    /// Vias.
    pub vias: Vec<Via>,
}

impl Signal {
    /// Parses a `<signal>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut obj = Self {
            name: e.attr_str("name")?.to_owned(),
            class: e.opt_int("class")?.unwrap_or(0),
            ..Self::default()
        };
        for child in e.children() {
            match child.tag_name() {
                "contactref" => obj.contact_refs.push(ContactRef::from_dom(child)?),
                "polygon" => obj.polygons.push(Polygon::from_dom(child, errors)?),
                "wire" => obj.wires.push(Wire::from_dom(child, errors)?),
                "via" => obj.vias.push(Via::from_dom(child, errors)?),
                other => errors.push(format!("Unknown signal child: {other}")),
            }
        }
        Ok(obj)
    }
}

/// An EAGLE board (`*.brd`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Board {
    /// Grid settings.
    pub grid: Grid,
    /// Design rules.
    pub design_rules: DesignRules,
    /// Embedded libraries.
    pub libraries: Vec<Library>,
    /// Wires.
    pub wires: Vec<Wire>,
    /// Rectangles.
    pub rectangles: Vec<Rectangle>,
    /// Circles.
    pub circles: Vec<Circle>,
    /// Polygons.
    pub polygons: Vec<Polygon>,
    /// Texts.
    pub texts: Vec<Text>,
    /// Holes.
    pub holes: Vec<Hole>,
    /// Placed packages.
    pub elements: Vec<Element>,
    /// Signals.
    pub signals: Vec<Signal>,
}

impl Board {
    /// Parses the content of a `*.brd` file.
    pub fn from_bytes(content: &[u8], errors: &mut Vec<String>) -> Result<Self> {
        let root = DomElement::parse_document(content)?;
        let drawing = root.first_child("drawing")?;
        let mut obj = Self::default();
        if let Some(grid) = drawing.child("grid") {
            obj.grid = Grid::from_dom(grid, &mut Vec::new())?;
        }
        let board = drawing.first_child("board")?;
        if let Some(rules) = board.child("designrules") {
            obj.design_rules = DesignRules::from_dom(rules, errors)?;
        }
        for child in board.children_of("libraries") {
            obj.libraries
                .push(Library::from_dom(child, &mut Vec::new())?);
        }
        for child in board.children_of("plain") {
            match child.tag_name() {
                "wire" => obj.wires.push(Wire::from_dom(child, errors)?),
                "rectangle" => obj.rectangles.push(Rectangle::from_dom(child)?),
                "circle" => obj.circles.push(Circle::from_dom(child)?),
                "polygon" => obj.polygons.push(Polygon::from_dom(child, errors)?),
                "text" => obj.texts.push(Text::from_dom(child, errors)?),
                "hole" => obj.holes.push(Hole::from_dom(child)?),
                "dimension" => {}
                other => errors.push(format!("Unknown board child: {other}")),
            }
        }
        for child in board.children_of("elements") {
            obj.elements.push(Element::from_dom(child, errors)?);
        }
        for child in board.children_of("signals") {
            obj.signals.push(Signal::from_dom(child, errors)?);
        }
        Ok(obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_param_with_unit() {
        let p = Param {
            name: "x".into(),
            value: "0.3mm".into(),
        };
        assert_eq!(p.value_as_double_with_unit(), Some((0.3, "mm".to_owned())));
        assert_eq!(p.value_as_double(), None);
    }
}
