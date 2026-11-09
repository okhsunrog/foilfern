//! Port of parseagle/library.{h,cpp}, parseagle/symbol/*,
//! parseagle/package/* and parseagle/deviceset/* (libs/parseagle).

use super::super::error::{Error, Result};
use super::common::{Attribute, Circle, Frame, Point, Polygon, Rectangle, Rotation, Text, Wire};
use super::dom::DomElement;
use super::enums::{GateAddLevel, PadShape, PinDirection, PinFunction, PinLength, PinVisibility};

fn point(e: &DomElement) -> Result<Point> {
    Ok(Point {
        x: e.attr_double("x")?,
        y: e.attr_double("y")?,
    })
}

fn rotation(e: &DomElement) -> Rotation {
    e.opt_str("rot").map(Rotation::parse).unwrap_or_default()
}

/// A symbol pin (`<pin>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    /// Name.
    pub name: String,
    /// Position.
    pub position: Point,
    /// Visibility of pin/pad names.
    pub visibility: PinVisibility,
    /// Length.
    pub length: PinLength,
    /// Electrical direction.
    pub direction: PinDirection,
    /// Graphical function.
    pub function: PinFunction,
    /// Swap level.
    pub swap_level: i32,
    /// Rotation.
    pub rotation: Rotation,
}

impl Pin {
    /// Parses a `<pin>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            position: point(e)?,
            visibility: e
                .opt_str("visible")
                .map_or(PinVisibility::Both, |s| PinVisibility::parse(s, errors)),
            length: e
                .opt_str("length")
                .map_or(PinLength::Long, |s| PinLength::parse(s, errors)),
            direction: e
                .opt_str("direction")
                .map_or(PinDirection::Io, |s| PinDirection::parse(s, errors)),
            function: e
                .opt_str("function")
                .map_or(PinFunction::None, |s| PinFunction::parse(s, errors)),
            swap_level: e.opt_int("swaplevel")?.unwrap_or(0),
            rotation: rotation(e),
        })
    }

    /// Returns the pin length in millimeters.
    pub fn length_mm(&self) -> f64 {
        match self.length {
            PinLength::Short => 2.54,
            PinLength::Middle => 5.08,
            PinLength::Long => 7.62,
            PinLength::Point | PinLength::Unknown => 0.0,
        }
    }
}

/// A symbol (`<symbol>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Symbol {
    /// Name.
    pub name: String,
    /// Description (may contain HTML).
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
    /// Pins.
    pub pins: Vec<Pin>,
    /// Frames.
    pub frames: Vec<Frame>,
}

impl Symbol {
    /// Parses a `<symbol>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut obj = Self {
            name: e.attr_str("name")?.to_owned(),
            ..Self::default()
        };
        for child in e.children() {
            match child.tag_name() {
                "description" => obj.description = child.text().to_owned(),
                "wire" => obj.wires.push(Wire::from_dom(child, errors)?),
                "rectangle" => obj.rectangles.push(Rectangle::from_dom(child)?),
                "circle" => obj.circles.push(Circle::from_dom(child)?),
                "polygon" => obj.polygons.push(Polygon::from_dom(child, errors)?),
                "text" => obj.texts.push(Text::from_dom(child, errors)?),
                "pin" => obj.pins.push(Pin::from_dom(child, errors)?),
                "frame" => obj.frames.push(Frame::from_dom(child)?),
                "dimension" => {}
                other => errors.push(format!("Unknown symbol child: {other}")),
            }
        }
        Ok(obj)
    }
}

/// A non-plated hole (`<hole>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Hole {
    /// Position.
    pub position: Point,
    /// Drill diameter \[mm\].
    pub diameter: f64,
}

impl Hole {
    /// Parses a `<hole>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            position: point(e)?,
            diameter: e.attr_double("drill")?,
        })
    }
}

/// An SMT pad (`<smd>`).
#[derive(Debug, Clone, PartialEq)]
pub struct SmtPad {
    /// Name.
    pub name: String,
    /// Layer ID.
    pub layer: i32,
    /// Position.
    pub position: Point,
    /// Rotation.
    pub rotation: Rotation,
    /// Width \[mm\].
    pub width: f64,
    /// Height \[mm\].
    pub height: f64,
    /// Roundness \[%\].
    pub roundness: i32,
    /// Stop mask flag.
    pub stop: bool,
    /// Solder paste flag.
    pub cream: bool,
}

impl SmtPad {
    /// Parses an `<smd>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            layer: e.attr_int("layer")?,
            position: point(e)?,
            rotation: rotation(e),
            width: e.attr_double("dx")?,
            height: e.attr_double("dy")?,
            roundness: e.opt_int("roundness")?.unwrap_or(0),
            stop: e.opt_bool("stop")?.unwrap_or(true),
            cream: e.opt_bool("cream")?.unwrap_or(true),
        })
    }
}

/// A THT pad (`<pad>`).
#[derive(Debug, Clone, PartialEq)]
pub struct ThtPad {
    /// Name.
    pub name: String,
    /// Position.
    pub position: Point,
    /// Drill diameter \[mm\].
    pub drill_diameter: f64,
    /// Outer diameter \[mm\] (0 = auto).
    pub outer_diameter: f64,
    /// Shape.
    pub shape: PadShape,
    /// Rotation.
    pub rotation: Rotation,
    /// Stop mask flag.
    pub stop: bool,
}

impl ThtPad {
    /// Parses a `<pad>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            position: point(e)?,
            drill_diameter: e.attr_double("drill")?,
            outer_diameter: e.opt_double("diameter")?.unwrap_or(0.0),
            shape: e
                .opt_str("shape")
                .map_or(PadShape::Round, |s| PadShape::parse(s, errors)),
            rotation: rotation(e),
            stop: e.opt_bool("stop")?.unwrap_or(true),
        })
    }
}

/// A package (`<package>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Package {
    /// Name.
    pub name: String,
    /// Description (may contain HTML).
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
    /// Holes.
    pub holes: Vec<Hole>,
    /// THT pads.
    pub tht_pads: Vec<ThtPad>,
    /// SMT pads.
    pub smt_pads: Vec<SmtPad>,
}

impl Package {
    /// Parses a `<package>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut obj = Self {
            name: e.attr_str("name")?.to_owned(),
            ..Self::default()
        };
        for child in e.children() {
            match child.tag_name() {
                "description" => obj.description = child.text().to_owned(),
                "wire" => obj.wires.push(Wire::from_dom(child, errors)?),
                "rectangle" => obj.rectangles.push(Rectangle::from_dom(child)?),
                "circle" => obj.circles.push(Circle::from_dom(child)?),
                "polygon" => obj.polygons.push(Polygon::from_dom(child, errors)?),
                "text" => obj.texts.push(Text::from_dom(child, errors)?),
                "hole" => obj.holes.push(Hole::from_dom(child)?),
                "pad" => obj.tht_pads.push(ThtPad::from_dom(child, errors)?),
                "smd" => obj.smt_pads.push(SmtPad::from_dom(child)?),
                "dimension" => {}
                other => errors.push(format!("Unknown package child: {other}")),
            }
        }
        Ok(obj)
    }
}

/// A gate of a device set (`<gate>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    /// Name.
    pub name: String,
    /// Name of the symbol.
    pub symbol: String,
    /// Position.
    pub position: Point,
    /// Add level.
    pub add_level: GateAddLevel,
}

impl Gate {
    /// Parses a `<gate>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            symbol: e.attr_str("symbol")?.to_owned(),
            position: point(e)?,
            add_level: e
                .opt_str("addlevel")
                .map_or(GateAddLevel::Next, |s| GateAddLevel::parse(s, errors)),
        })
    }
}

/// A connection between a gate pin and pads (`<connect>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    /// Gate name.
    pub gate: String,
    /// Pin name.
    pub pin: String,
    /// Pad names.
    pub pads: Vec<String>,
}

impl Connection {
    /// Parses a `<connect>` element.
    pub fn from_dom(e: &DomElement) -> Result<Self> {
        Ok(Self {
            gate: e.attr_str("gate")?.to_owned(),
            pin: e.attr_str("pin")?.to_owned(),
            pads: e.attr_str("pad")?.split(' ').map(str::to_owned).collect(),
        })
    }
}

/// A technology of a device (`<technology>`).
#[derive(Debug, Clone, PartialEq)]
pub struct Technology {
    /// Name.
    pub name: String,
    /// Attributes.
    pub attributes: Vec<Attribute>,
}

impl Technology {
    /// Parses a `<technology>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut attributes = Vec::new();
        for child in e.children() {
            if child.tag_name() == "attribute" {
                attributes.push(Attribute::from_dom(child, errors)?);
            } else {
                errors.push(format!("Unknown technology child: {}", child.tag_name()));
            }
        }
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            attributes,
        })
    }
}

/// A device of a device set (`<device>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Device {
    /// Name (may be empty).
    pub name: String,
    /// Package name (empty for devices without package).
    pub package: String,
    /// Pin-pad connections.
    pub connections: Vec<Connection>,
    /// Technologies.
    pub technologies: Vec<Technology>,
}

impl Device {
    /// Parses a `<device>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            name: e.opt_str("name").unwrap_or_default().to_owned(),
            package: e.opt_str("package").unwrap_or_default().to_owned(),
            connections: e
                .children_of("connects")
                .iter()
                .map(Connection::from_dom)
                .collect::<Result<_>>()?,
            technologies: e
                .children_of("technologies")
                .iter()
                .map(|c| Technology::from_dom(c, errors))
                .collect::<Result<_>>()?,
        })
    }
}

/// A device set (`<deviceset>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceSet {
    /// Name.
    pub name: String,
    /// Description (may contain HTML).
    pub description: String,
    /// Designator prefix.
    pub prefix: String,
    /// Whether the value is user-defined.
    pub user_value: bool,
    /// Gates.
    pub gates: Vec<Gate>,
    /// Devices.
    pub devices: Vec<Device>,
}

impl DeviceSet {
    /// Parses a `<deviceset>` element.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        Ok(Self {
            name: e.attr_str("name")?.to_owned(),
            description: e
                .child("description")
                .map(|c| c.text().to_owned())
                .unwrap_or_default(),
            prefix: e.opt_str("prefix").unwrap_or_default().to_owned(),
            user_value: e.opt_bool("uservalue")?.unwrap_or(false),
            gates: e
                .children_of("gates")
                .iter()
                .map(|c| Gate::from_dom(c, errors))
                .collect::<Result<_>>()?,
            devices: e
                .children_of("devices")
                .iter()
                .map(|c| Device::from_dom(c, errors))
                .collect::<Result<_>>()?,
        })
    }
}

/// A library (`*.lbr` file or a library embedded in a schematic/board).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Library {
    /// Name (only for embedded libraries).
    pub embedded_name: String,
    /// URN (only for embedded libraries).
    pub embedded_urn: String,
    /// Description.
    pub description: String,
    /// Symbols.
    pub symbols: Vec<Symbol>,
    /// Packages.
    pub packages: Vec<Package>,
    /// Device sets.
    pub device_sets: Vec<DeviceSet>,
}

impl Library {
    /// Parses the content of a `*.lbr` file.
    pub fn from_bytes(content: &[u8], errors: &mut Vec<String>) -> Result<Self> {
        let root = DomElement::parse_document(content)?;
        let library = root.first_child("drawing")?.first_child("library")?;
        Self::from_dom(library, errors)
    }

    /// Parses a `*.lbr` file.
    pub fn open(path: &std::path::Path, errors: &mut Vec<String>) -> Result<Self> {
        if !path.exists() {
            return Err(Error::FileNotFound(path.display().to_string()));
        }
        let content = std::fs::read(path).map_err(|e| Error::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        Self::from_bytes(&content, errors)
    }

    /// Parses a `<library>` element. Elements which fail to parse are
    /// skipped and reported in `errors`.
    pub fn from_dom(e: &DomElement, errors: &mut Vec<String>) -> Result<Self> {
        let mut obj = Self {
            embedded_name: e.opt_str("name").unwrap_or_default().to_owned(),
            embedded_urn: e.opt_str("urn").unwrap_or_default().to_owned(),
            description: e
                .child("description")
                .map(|c| c.text().to_owned())
                .unwrap_or_default(),
            ..Self::default()
        };
        for child in e.children_of("symbols") {
            match Symbol::from_dom(child, errors) {
                Ok(o) => obj.symbols.push(o),
                Err(err) => errors.push(format!("Failed to parse symbol: {err}")),
            }
        }
        for child in e.children_of("packages") {
            match Package::from_dom(child, errors) {
                Ok(o) => obj.packages.push(o),
                Err(err) => errors.push(format!("Failed to parse package: {err}")),
            }
        }
        for child in e.children_of("devicesets") {
            match DeviceSet::from_dom(child, errors) {
                Ok(o) => obj.device_sets.push(o),
                Err(err) => errors.push(format!("Failed to parse deviceset: {err}")),
            }
        }
        Ok(obj)
    }
}
