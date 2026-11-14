//! Port of parseagle/schematic/* (libs/parseagle): EAGLE schematics
//! (`*.sch`).

use super::super::error::Result;
use super::common::{
    Attribute, Circle, Frame, Grid, Point, Polygon, Rectangle, Rotation, Text, Wire,
};
use super::dom::DomElement;
use super::enums::Alignment;
use super::library::Library;

fn point(e: &DomElement) -> Result<Point> {
    Ok(Point {
        x: e.attr_double("x")?,
        y: e.attr_double("y")?,
    })
}

fn rotation(e: &DomElement) -> Rotation {
    e.opt_str("rot").map(Rotation::parse).unwrap_or_default()
}

/// A module (`<module>`, only the name is parsed).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// Name.
    pub name: String,
}

/// A part (component instance, `<part>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// Name (designator).
    pub name: String,
    /// Library name.
    pub library: String,
    /// Library URN.
    pub library_urn: String,
    /// Device set name.
    pub device_set: String,
    /// Device name.
    pub device: String,
    /// Technology name.
    pub technology: String,
    /// Value.
    pub value: String,
    /// Attributes.
    pub attributes: Vec<Attribute>,
}

impl Part {
    /// Parses a `<part>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut attributes = Vec::new();
        for child in e.children() {
            if child.tag_name() == "attribute" {
                attributes.push(Attribute::from_dom(child, errors)?);
            } else {
                errors.push(format!("Unknown part child: {}", child.tag_name()));
            }
        }
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            library: e.attr_str("library")?.to_owned(),
            library_urn: e.opt_str("library_urn").unwrap_or_default().to_owned(),
            device_set: e.attr_str("deviceset")?.to_owned(),
            device: e.attr_str("device")?.to_owned(),
            technology: e.opt_str("technology").unwrap_or_default().to_owned(),
            value: e.opt_str("value").unwrap_or_default().to_owned(),
            attributes,
        })
    }
}

/// A placed gate (`<instance>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// Part name.
    pub part: String,
    /// Gate name.
    pub gate: String,
    /// Position.
    pub position: Point,
    /// Rotation.
    pub rotation: Rotation,
    /// Smashed flag.
    pub smashed: bool,
    /// Attributes.
    pub attributes: Vec<Attribute>,
}

impl Instance {
    /// Parses an `<instance>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut attributes = Vec::new();
        for child in e.children() {
            if child.tag_name() == "attribute" {
                attributes.push(Attribute::from_dom(child, errors)?);
            } else {
                errors.push(format!("Unknown instance child: {}", child.tag_name()));
            }
        }
        Ok(Self {
            part: e.attr_str("part")?.to_owned(),
            gate: e.attr_str("gate")?.to_owned(),
            position: point(e)?,
            rotation: rotation(e),
            smashed: e.opt_bool("smashed")?.unwrap_or(false),
            attributes,
        })
    }
}

/// A net label (`<label>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    /// Position.
    pub position: Point,
    /// Text size \[mm\].
    pub size: f64,
    /// Layer ID.
    pub layer: i32,
    /// Stroke width ratio \[%\].
    pub ratio: i32,
    /// Rotation.
    pub rotation: Rotation,
    /// Alignment.
    pub alignment: Alignment,
    /// Cross-reference flag.
    pub xref: bool,
}

impl Label {
    /// Parses a `<label>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            position: point(e)?,
            size: e.attr_double("size")?,
            layer: e.attr_int("layer")?,
            ratio: e.opt_int("ratio")?.unwrap_or(8),
            rotation: rotation(e),
            alignment: e.opt_str("align").map_or(Alignment::BottomLeft, |s| {
                Alignment::parse(s, &mut Vec::new())
            }),
            xref: e.opt_bool("xref")?.unwrap_or(false),
        })
    }
}

/// A pin connected to a net segment (`<pinref>`).
#[derive(Debug, Clone, PartialEq)]
pub struct PinRef {
    /// Part name.
    pub part: String,
    /// Gate name.
    pub gate: String,
    /// Pin name.
    pub pin: String,
}

/// A net or bus segment (`<segment>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Segment {
    /// Connected pins.
    pub pin_refs: Vec<PinRef>,
    /// Wires.
    pub wires: Vec<Wire>,
    /// Junctions.
    pub junctions: Vec<Point>,
    /// Labels.
    pub labels: Vec<Label>,
}

impl Segment {
    /// Parses a `<segment>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut obj = Self::default();
        for child in e.children() {
            match child.tag_name() {
                "pinref" => obj.pin_refs.push(PinRef {
                    part: child.attr_str("part")?.to_owned(),
                    gate: child.attr_str("gate")?.to_owned(),
                    pin: child.attr_str("pin")?.to_owned(),
                }),
                "wire" => obj.wires.push(Wire::from_dom(child, errors)?),
                "junction" => obj.junctions.push(point(child)?),
                "label" => obj.labels.push(Label::from_dom(child)?),
                other => errors.push(format!("Unknown net segment child: {other}")),
            }
        }
        Ok(obj)
    }
}

/// A net (`<net>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Net {
    /// Name.
    pub name: String,
    /// Net class.
    pub class: i32,
    /// Segments.
    pub segments: Vec<Segment>,
}

/// A bus (`<bus>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bus {
    /// Name.
    pub name: String,
    /// Segments.
    pub segments: Vec<Segment>,
}

fn segments(e: &DomElement, what: &str, errors: &mut Vec<String>) -> Result<Vec<Segment>> {
    let mut result = Vec::new();
    for child in e.children() {
        if child.tag_name() == "segment" {
            result.push(Segment::from_dom(child, errors)?);
        } else {
            errors.push(format!("Unknown {what} child: {}", child.tag_name()));
        }
    }
    Ok(result)
}

/// A schematic sheet (`<sheet>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    /// Description.
    pub description: String,
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
    /// Frames.
    pub frames: Vec<Frame>,
    /// Placed gates.
    pub instances: Vec<Instance>,
    /// Buses.
    pub buses: Vec<Bus>,
    /// Nets.
    pub nets: Vec<Net>,
}

impl Sheet {
    /// Parses a `<sheet>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut obj = Self::default();
        for child in e.children() {
            match child.tag_name() {
                "description" => obj.description = child.text().to_owned(),
                "plain" => {
                    for c in child.children() {
                        match c.tag_name() {
                            "wire" => obj.wires.push(Wire::from_dom(c, errors)?),
                            "rectangle" => obj.rectangles.push(Rectangle::from_dom(c)?),
                            "circle" => obj.circles.push(Circle::from_dom(c)?),
                            "polygon" => obj.polygons.push(Polygon::from_dom(c, errors)?),
                            "text" => obj.texts.push(Text::from_dom(c, errors)?),
                            "frame" => obj.frames.push(Frame::from_dom(c)?),
                            "dimension" => {}
                            other => errors.push(format!("Unknown sheet plain child: {other}")),
                        }
                    }
                }
                "instances" => {
                    for c in child.children() {
                        obj.instances.push(Instance::from_dom(c, errors)?);
                    }
                }
                "busses" => {
                    for c in child.children() {
                        obj.buses.push(Bus {
                            name: c.attr_str("name")?.to_owned(),
                            segments: segments(c, "bus", errors)?,
                        });
                    }
                }
                "nets" => {
                    for c in child.children() {
                        obj.nets.push(Net {
                            name: c.attr_str("name")?.to_owned(),
                            class: c.opt_int("class")?.unwrap_or(0),
                            segments: segments(c, "net", errors)?,
                        });
                    }
                }
                other => errors.push(format!("Unknown sheet child: {other}")),
            }
        }
        Ok(obj)
    }
}

/// An EAGLE schematic (`*.sch`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Schematic {
    /// Description.
    pub description: String,
    /// Grid settings.
    pub grid: Grid,
    /// Embedded libraries.
    pub libraries: Vec<Library>,
    /// Modules.
    pub modules: Vec<Module>,
    /// Parts.
    pub parts: Vec<Part>,
    /// Sheets.
    pub sheets: Vec<Sheet>,
}

impl Schematic {
    /// Parses the content of a `*.sch` file.
    pub fn from_bytes(content: &[u8], errors: &mut Vec<String>) -> Result<Self> {
        let root = DomElement::parse_document(content)?;
        let drawing = root.first_child("drawing")?;
        let mut obj = Self::default();
        if let Some(grid) = drawing.child("grid") {
            obj.grid = Grid::from_dom(grid, &mut Vec::new())?;
        }
        let schematic = drawing.first_child("schematic")?;
        if let Some(d) = schematic.child("description") {
            obj.description = d.text().to_owned();
        }
        for child in schematic.children_of("libraries") {
            obj.libraries.push(Library::from_dom(child, errors)?);
        }
        for child in schematic.children_of("modules") {
            obj.modules.push(Module {
                name: child.attr_str("name")?.to_owned(),
            });
        }
        for child in schematic.children_of("parts") {
            obj.parts.push(Part::from_dom(child, errors)?);
        }
        for child in schematic.children_of("sheets") {
            obj.sheets.push(Sheet::from_dom(child, errors)?);
        }
        Ok(obj)
    }
}
