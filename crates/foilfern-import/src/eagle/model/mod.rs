//! Port of libs/parseagle: the EAGLE XML file format (libraries `*.lbr`,
//! schematics `*.sch` and boards `*.brd`) as plain data.
//!
//! Upstream parses with `QXmlStreamReader`; here the XML is read with
//! [`roxmltree`] into an owned [`DomElement`] tree first.

mod board;
mod common;
mod dom;
mod enums;
mod library;
mod schematic;

pub use board::{Board, ContactRef, DesignRules, Element, Param, Signal, Via};
pub use common::{
    Attribute, Circle, Frame, Grid, Point, Polygon, Rectangle, Rotation, Text, Vertex, Wire,
};
pub use dom::DomElement;
pub use enums::{
    Alignment, AttributeDisplay, Font, GateAddLevel, GridStyle, GridUnit, PadShape, PinDirection,
    PinFunction, PinLength, PinVisibility, PolygonPour, ViaShape, WireCap, WireStyle,
};
pub use library::{
    Connection, Device, DeviceSet, Gate, Hole, Library, Package, Pin, SmtPad, Symbol, Technology,
    ThtPad,
};
pub use schematic::{Bus, Instance, Label, Module, Net, Part, PinRef, Schematic, Segment, Sheet};
