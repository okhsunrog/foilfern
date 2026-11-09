//! Port of libs/librepcb/kicadimport/kicadtypes.{h,cpp}.
//!
//! The KiCad symbol library (`*.kicad_sym`) and footprint (`*.kicad_mod`)
//! file formats as plain data, parsed from the S-expression tree of
//! `librepcb-core` (in permissive mode). File format documentation:
//! <https://dev-docs.kicad.org/en/file-formats/sexpr-intro/index.html>
//!
//! Differences to upstream: `QPointF`/`QSizeF`/`QVector3D` are replaced by
//! [`PointF`], [`SizeF`] and `[f32; 3]`, `Qt::Alignment` by LibrePCB's
//! [`Alignment`].

use librepcb_core::serialization::{FromSExpression, SExpression};
use librepcb_core::types::{Alignment, HAlign, VAlign};
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_i18n::tr;

use super::error::{Error, Result};

/// A point \[mm\] (upstream `QPointF`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointF {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

impl PointF {
    /// Returns whether both coordinates are zero (`QPointF::isNull()`).
    pub fn is_null(&self) -> bool {
        (self.x == 0.0) && (self.y == 0.0)
    }
}

/// A size \[mm\] (upstream `QSizeF`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SizeF {
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl SizeF {
    /// Returns whether width and height are zero (`QSizeF::isNull()`).
    pub fn is_null(&self) -> bool {
        (self.width == 0.0) && (self.height == 0.0)
    }
}

fn f64_of(node: &SExpression) -> Result<f64> {
    Ok(<f64 as FromSExpression>::from_sexpression(node)?)
}

fn child_f64(node: &SExpression, path: &str) -> Result<f64> {
    Ok(node.child_value::<f64>(path)?)
}

fn opt_f64(node: &SExpression, path: &str) -> Result<Option<f64>> {
    node.child(path).map(f64_of).transpose()
}

fn point(node: &SExpression) -> Result<PointF> {
    Ok(PointF {
        x: child_f64(node, "@0")?,
        y: child_f64(node, "@1")?,
    })
}

fn size(node: &SExpression) -> Result<SizeF> {
    Ok(SizeF {
        width: child_f64(node, "@0")?,
        height: child_f64(node, "@1")?,
    })
}

fn vector3d(node: &SExpression) -> Result<[f32; 3]> {
    Ok([
        node.child_value::<f32>("@0")?,
        node.child_value::<f32>("@1")?,
        node.child_value::<f32>("@2")?,
    ])
}

/// Returns the value of the child at `index` (counting line breaks like
/// upstream `getChild(int)`).
fn raw_value(node: &SExpression, index: usize) -> Result<&str> {
    let child = node
        .children()
        .get(index)
        .ok_or(Error::ChildNotFound(index))?;
    Ok(child.value()?)
}

fn value<'a>(node: &'a SExpression, path: &str) -> Result<&'a str> {
    Ok(node.required_child(path)?.value()?)
}

fn opt_value<'a>(node: &'a SExpression, path: &str) -> Result<Option<&'a str>> {
    node.child(path).map(|c| Ok(c.value()?)).transpose()
}

fn list_children(node: &SExpression) -> impl Iterator<Item = &SExpression> {
    node.children().iter().filter(|c| c.is_list())
}

fn name(node: &SExpression) -> &str {
    node.name().unwrap_or_default()
}

fn has_token(node: &SExpression, token: &str) -> bool {
    node.contains_child(&SExpression::token(token))
}

fn deserialize_bool(node: &SExpression) -> Result<bool> {
    match node.value()? {
        "yes" => Ok(true),
        "no" => Ok(false),
        other => Err(Error::InvalidBool(other.to_owned())),
    }
}

fn deserialize_alignment(node: &SExpression) -> Alignment {
    let h = if has_token(node, "left") {
        HAlign::Left
    } else if has_token(node, "right") {
        HAlign::Right
    } else {
        HAlign::Center
    };
    let v = if has_token(node, "top") {
        VAlign::Top
    } else if has_token(node, "bottom") {
        VAlign::Bottom
    } else {
        VAlign::Center
    };
    Alignment::new(h, v)
}

/// Default alignment (upstream `Qt::AlignCenter`).
const CENTER: Alignment = Alignment::new(HAlign::Center, VAlign::Center);

macro_rules! kicad_enum {
    ($(#[$meta:meta])* $name:ident, $what:literal { $($variant:ident = $($token:literal)|+,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            /// Parse error.
            Unknown,
            $(
                #[doc = concat!($("`", $token, "` "),+)]
                $variant,
            )*
        }

        impl $name {
            /// Parses the value of `node`; unknown values are logged.
            pub fn parse(node: &SExpression, log: &MessageLogger<'_>) -> Result<Self> {
                let v = node.value()?;
                Ok(match v {
                    $($($token)|+ => Self::$variant,)*
                    _ => {
                        log.warning(&format!(concat!("Unknown ", $what, ": {}"), v));
                        Self::Unknown
                    }
                })
            }
        }
    };
}

kicad_enum!(
    /// Edge of a chamfered pad.
    KiCadEdge, "edge" {
        TopLeft = "top_left",
        TopRight = "top_right",
        BottomLeft = "bottom_left",
        BottomRight = "bottom_right",
    }
);

kicad_enum!(
    /// Stroke type of lines.
    KiCadStrokeType, "stroke type" {
        Dash = "dash",
        DashDot = "dash_dot",
        DashDotDot = "dash_dot_dot",
        Dot = "dot",
        Default = "default",
        Solid = "solid",
    }
);

kicad_enum!(
    /// Type of a footprint text.
    KiCadSymbolTextType, "text type" {
        Reference = "reference",
        Value = "value",
        User = "user",
    }
);

kicad_enum!(
    /// Fill type of symbol graphics.
    KiCadSymbolFillType, "symbol fill type" {
        None = "none",
        Outline = "outline",
        Background = "background",
    }
);

kicad_enum!(
    /// Electrical type of a symbol pin.
    KiCadPinType, "pin type" {
        Input = "input",
        Output = "output",
        Bidirectional = "bidirectional",
        TriState = "tri_state",
        Passive = "passive",
        Free = "free",
        Unspecified = "unspecified",
        PowerIn = "power_in",
        PowerOut = "power_out",
        OpenCollector = "open_collector",
        OpenEmitter = "open_emitter",
        NoConnect = "no_connect",
    }
);

kicad_enum!(
    /// Graphical style of a symbol pin.
    KiCadPinStyle, "pin shape" {
        Line = "line",
        Inverted = "inverted",
        Clock = "clock",
        InvertedClock = "inverted_clock",
        InputLow = "input_low",
        ClockLow = "clock_low",
        OutputLow = "output_low",
        EdgeClockHigh = "edge_clock_high",
        NonLogic = "non_logic",
    }
);

kicad_enum!(
    /// Fill type of footprint graphics (just yes/no since KiCad v9).
    KiCadFootprintFillType, "footprint fill type" {
        None = "no" | "none",
        Solid = "yes" | "solid",
    }
);

kicad_enum!(
    /// Type of a footprint pad.
    KiCadPadType, "pad type" {
        ThruHole = "thru_hole",
        Smd = "smd",
        Connect = "connect",
        NpThruHole = "np_thru_hole",
    }
);

kicad_enum!(
    /// Shape of a footprint pad.
    KiCadPadShape, "pad shape" {
        Circle = "circle",
        Rect = "rect",
        Oval = "oval",
        Trapezoid = "trapezoid",
        RoundRect = "roundrect",
        Custom = "custom",
    }
);

kicad_enum!(
    /// Anchor of a custom pad.
    KiCadCustomPadAnchor, "custom pad anchor" {
        Circle = "circle",
        Rect = "rect",
    }
);

kicad_enum!(
    /// Fabrication property of a pad.
    KiCadPadProperty, "pad property" {
        Bga = "pad_prop_bga",
        FiducialGlobal = "pad_prop_fiducial_glob",
        FiducialLocal = "pad_prop_fiducial_loc",
        Testpoint = "pad_prop_testpoint",
        Heatsink = "pad_prop_heatsink",
        Castellated = "pad_prop_castellated",
    }
);

kicad_enum!(
    /// Zone connection mode.
    KiCadZoneConnect, "zone connect" {
        NoConnect = "0",
        ThermalReliefs = "1",
        Solid = "2",
    }
);

/// A KiCad board layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum KiCadLayer {
    /// Parse error.
    Unknown,
    /// `*.Cu`
    AllCopper,
    /// `*.Mask`
    AllSolderMask,
    /// `*.SilkS`
    AllSilkscreen,
    /// `F&B.Cu`
    FrontAndBackCopper,
    FrontAdhesion,
    FrontCopper,
    FrontCourtyard,
    FrontFabrication,
    FrontPaste,
    FrontSilkscreen,
    FrontSolderMask,
    /// `In1.Cu` .. `In30.Cu` (1-based number).
    InnerCopper(u8),
    BackAdhesion,
    BackCopper,
    BackCourtyard,
    BackFabrication,
    BackPaste,
    BackSilkscreen,
    BackSolderMask,
    BoardOutline,
    UserComment,
    UserDrawing,
    /// `User.1` .. `User.9`.
    User(u8),
}

impl KiCadLayer {
    /// Parses a layer name; unknown names are logged.
    pub fn parse(node: &SExpression, log: &MessageLogger<'_>) -> Result<Self> {
        let v = node.value()?;
        let layer = match v {
            "*.Cu" => Self::AllCopper,
            "*.Mask" => Self::AllSolderMask,
            "*.SilkS" => Self::AllSilkscreen,
            "F&B.Cu" => Self::FrontAndBackCopper,
            "F.Adhes" => Self::FrontAdhesion,
            "F.Cu" => Self::FrontCopper,
            "F.CrtYd" => Self::FrontCourtyard,
            "F.Fab" => Self::FrontFabrication,
            "F.Paste" => Self::FrontPaste,
            "F.SilkS" => Self::FrontSilkscreen,
            "F.Mask" => Self::FrontSolderMask,
            "B.Adhes" => Self::BackAdhesion,
            "B.Cu" => Self::BackCopper,
            "B.CrtYd" => Self::BackCourtyard,
            "B.Fab" => Self::BackFabrication,
            "B.Paste" => Self::BackPaste,
            "B.SilkS" => Self::BackSilkscreen,
            "B.Mask" => Self::BackSolderMask,
            "Edge.Cuts" => Self::BoardOutline,
            "Cmts.User" => Self::UserComment,
            "Dwgs.User" => Self::UserDrawing,
            _ => {
                let inner = v
                    .strip_prefix("In")
                    .and_then(|s| s.strip_suffix(".Cu"))
                    .and_then(|s| s.parse::<u8>().ok())
                    .filter(|n| (1..=30).contains(n));
                let user = v
                    .strip_prefix("User.")
                    .and_then(|s| s.parse::<u8>().ok())
                    .filter(|n| (1..=9).contains(n) && (v.len() == 6));
                if let Some(n) = inner {
                    Self::InnerCopper(n)
                } else if let Some(n) = user {
                    Self::User(n)
                } else {
                    log.warning(&format!("Unknown layer: {v}"));
                    Self::Unknown
                }
            }
        };
        Ok(layer)
    }

    /// Returns the index of the layer in upstream's `KiCadLayer` enum (used
    /// in messages).
    pub fn index(self) -> i32 {
        match self {
            Self::Unknown => 0,
            Self::AllCopper => 1,
            Self::AllSolderMask => 2,
            Self::AllSilkscreen => 3,
            Self::FrontAndBackCopper => 4,
            Self::FrontAdhesion => 5,
            Self::FrontCopper => 6,
            Self::FrontCourtyard => 7,
            Self::FrontFabrication => 8,
            Self::FrontPaste => 9,
            Self::FrontSilkscreen => 10,
            Self::FrontSolderMask => 11,
            Self::InnerCopper(n) => 11 + i32::from(n),
            Self::BackAdhesion => 42,
            Self::BackCopper => 43,
            Self::BackCourtyard => 44,
            Self::BackFabrication => 45,
            Self::BackPaste => 46,
            Self::BackSilkscreen => 47,
            Self::BackSolderMask => 48,
            Self::BoardOutline => 49,
            Self::UserComment => 50,
            Self::UserDrawing => 51,
            Self::User(n) => 51 + i32::from(n),
        }
    }
}

/// A property of a symbol or footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadProperty {
    /// Key.
    pub key: String,
    /// Value.
    pub value: String,
    /// Text position.
    pub position: PointF,
    /// Text rotation \[°\].
    pub rotation: f64,
    /// Layer name (may be empty).
    pub layer: String,
    /// Font size.
    pub font_size: SizeF,
    /// Font thickness.
    pub font_thickness: f64,
    /// Text alignment.
    pub alignment: Alignment,
    /// Mirror flag.
    pub mirror: bool,
    /// Unlocked flag (not kept upright).
    pub unlocked: bool,
    /// Hidden flag.
    pub hide: bool,
}

impl KiCadProperty {
    /// Parses a `(property ...)` node.
    pub fn parse(node: &SExpression, _log: &MessageLogger<'_>) -> Result<Self> {
        let mut obj = Self {
            key: raw_value(node, 0)?.to_owned(),
            value: raw_value(node, 1)?.to_owned(),
            position: point(node.required_child("at")?)?,
            rotation: 0.0,
            layer: String::new(),
            font_size: SizeF::default(),
            font_thickness: 0.0,
            alignment: CENTER,
            mirror: false,
            unlocked: false,
            hide: false,
        };
        if let Some(child) = node.child("at/@2") {
            if child.value()? == "unlocked" {
                obj.unlocked = true; // KiCad v6 compatibility.
            } else {
                obj.rotation = f64_of(child)?;
            }
        }
        if let Some(layer) = opt_value(node, "layer/@0")? {
            obj.layer = layer.to_owned();
        }
        if let Some(child) = node.child("effects/font/size") {
            obj.font_size = size(child)?;
        }
        if let Some(v) = opt_f64(node, "effects/font/thickness/@0")? {
            obj.font_thickness = v;
        }
        if let Some(child) = node.child("effects/justify") {
            obj.alignment = deserialize_alignment(child);
            obj.mirror = has_token(child, "mirror");
        }
        if let Some(v) = opt_value(node, "unlocked/@0")? {
            obj.unlocked = v == "yes";
        }
        if let Some(child) = node.child("effects/hide/@0") {
            obj.hide = deserialize_bool(child)?;
        }
        Ok(obj)
    }
}

/// A line of a custom pad shape (`gr_line`).
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadGraphicalLine {
    /// Start point.
    pub start: PointF,
    /// End point.
    pub end: PointF,
    /// Width.
    pub width: f64,
}

/// An arc of a custom pad shape (`gr_arc`).
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadGraphicalArc {
    /// Start point.
    pub start: PointF,
    /// Mid point.
    pub mid: PointF,
    /// End point.
    pub end: PointF,
    /// Width.
    pub width: f64,
}

/// A circle of a custom pad shape (`gr_circle`).
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadGraphicalCircle {
    /// Center.
    pub center: PointF,
    /// A point on the circle.
    pub end: PointF,
    /// Width.
    pub width: f64,
    /// Fill flag.
    pub fill: bool,
}

/// A polygon of a custom pad shape (`gr_poly`).
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadGraphicalPolygon {
    /// Vertices.
    pub coordinates: Vec<PointF>,
    /// Width.
    pub width: f64,
    /// Fill flag.
    pub fill: bool,
}

fn graphical_fill(node: &SExpression) -> Result<bool> {
    Ok(opt_value(node, "fill/@0")?.is_some_and(|v| (v == "yes") || (v == "solid")))
}

fn coordinates(node: &SExpression) -> Result<Vec<PointF>> {
    node.required_child("pts")?
        .children_named("xy")
        .map(point)
        .collect()
}

/// A keepout zone of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadZone {
    /// Layers.
    pub layers: Vec<KiCadLayer>,
    /// No tracks.
    pub keep_out_tracks: bool,
    /// No vias.
    pub keep_out_vias: bool,
    /// No pads.
    pub keep_out_pads: bool,
    /// No copper pour.
    pub keep_out_copper_pour: bool,
    /// No footprints.
    pub keep_out_footprints: bool,
    /// Outline.
    pub polygon: Vec<PointF>,
}

impl KiCadZone {
    /// Parses a `(zone ...)` node.
    pub fn parse(node: &SExpression, log: &MessageLogger<'_>) -> Result<Self> {
        let keep_out = |name: &str| -> Result<bool> {
            let v = value(node, &format!("keepout/{name}/@0"))?;
            Ok(match v {
                "allowed" => false,
                "not_allowed" => true,
                _ => {
                    log.warning(&format!("Unknown keepout value: {v}"));
                    false
                }
            })
        };
        let mut layers = Vec::new();
        if let Some(child) = node.child("layer/@0") {
            layers.push(KiCadLayer::parse(child, log)?);
        } else {
            for child in node
                .required_child("layers")?
                .children()
                .iter()
                .filter(|c| c.is_string())
            {
                layers.push(KiCadLayer::parse(child, log)?);
            }
        }
        Ok(Self {
            layers,
            keep_out_tracks: keep_out("tracks")?,
            keep_out_vias: keep_out("vias")?,
            keep_out_pads: keep_out("pads")?,
            keep_out_copper_pour: keep_out("copperpour")?,
            keep_out_footprints: keep_out("footprints")?,
            polygon: node
                .required_child("polygon/pts")?
                .children_named("xy")
                .map(point)
                .collect::<Result<_>>()?,
        })
    }
}

/// Stroke width (`width` for KiCad v6 or `stroke/width`) and optional
/// stroke type.
fn stroke(node: &SExpression, log: &MessageLogger<'_>) -> Result<(f64, KiCadStrokeType)> {
    let width = match node.child("width/@0") {
        Some(child) => f64_of(child)?, // KiCad v6 compatibility.
        None => child_f64(node, "stroke/width/@0")?,
    };
    let stroke_type = match node.child("stroke/type/@0") {
        Some(child) => KiCadStrokeType::parse(child, log)?,
        None => KiCadStrokeType::Solid,
    };
    Ok((width, stroke_type))
}

/// An arc of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbolArc {
    /// Start point.
    pub start: PointF,
    /// Mid point.
    pub mid: PointF,
    /// End point.
    pub end: PointF,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Fill type.
    pub fill_type: KiCadSymbolFillType,
}

/// A circle of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbolCircle {
    /// Center.
    pub center: PointF,
    /// Radius.
    pub radius: f64,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Fill type.
    pub fill_type: KiCadSymbolFillType,
}

/// A rectangle of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbolRectangle {
    /// First corner.
    pub start: PointF,
    /// Second corner.
    pub end: PointF,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Fill type.
    pub fill_type: KiCadSymbolFillType,
}

/// A polyline of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbolPolyline {
    /// Vertices.
    pub coordinates: Vec<PointF>,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Fill type.
    pub fill_type: KiCadSymbolFillType,
}

/// A text of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbolText {
    /// Text.
    pub text: String,
    /// Position.
    pub position: PointF,
    /// Rotation \[°\].
    pub rotation: f64,
    /// Font size.
    pub font_size: SizeF,
    /// Font thickness.
    pub font_thickness: f64,
    /// Alignment.
    pub alignment: Alignment,
}

/// A pin of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbolPin {
    /// Electrical type.
    pub pin_type: KiCadPinType,
    /// Graphical style.
    pub shape: KiCadPinStyle,
    /// Position.
    pub position: PointF,
    /// Rotation \[°\].
    pub rotation: f64,
    /// Length.
    pub length: f64,
    /// Name.
    pub name: String,
    /// Number (pad name).
    pub number: String,
}

/// Style of a symbol gate (unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KiCadSymbolGateStyle {
    /// Common to all styles.
    Common,
    /// Base style.
    Base,
    /// De Morgan style.
    DeMorgan,
}

/// A gate (unit) of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbolGate {
    /// Name, including index and style suffixes.
    pub name: String,
    /// Index (0 = common to all gates).
    pub index: i32,
    /// Style.
    pub style: KiCadSymbolGateStyle,
    /// Arcs.
    pub arcs: Vec<KiCadSymbolArc>,
    /// Circles.
    pub circles: Vec<KiCadSymbolCircle>,
    /// Rectangles.
    pub rectangles: Vec<KiCadSymbolRectangle>,
    /// Polylines.
    pub polylines: Vec<KiCadSymbolPolyline>,
    /// Texts.
    pub texts: Vec<KiCadSymbolText>,
    /// Pins.
    pub pins: Vec<KiCadSymbolPin>,
}

impl KiCadSymbolGate {
    /// Parses a gate `(symbol "NAME_1_1" ...)` node.
    pub fn parse(node: &SExpression, log: &MessageLogger<'_>) -> Result<Self> {
        let name = raw_value(node, 0)?.to_owned();
        let parts: Vec<&str> = name.split('_').collect();
        let index = parts
            .len()
            .checked_sub(2)
            .and_then(|i| parts[i].parse::<i32>().ok());
        let style = parts.last().and_then(|s| s.parse::<i32>().ok());
        let (Some(index), Some(style), true) = (index, style, parts.len() >= 3) else {
            return Err(Error::InvalidGateName(name));
        };
        let style = match style {
            0 => KiCadSymbolGateStyle::Common,
            1 => KiCadSymbolGateStyle::Base,
            2 => KiCadSymbolGateStyle::DeMorgan,
            other => return Err(Error::UnknownGateStyle(other)),
        };
        let mut obj = Self {
            name,
            index,
            style,
            arcs: Vec::new(),
            circles: Vec::new(),
            rectangles: Vec::new(),
            polylines: Vec::new(),
            texts: Vec::new(),
            pins: Vec::new(),
        };
        for child in list_children(node) {
            match name_of(child) {
                "arc" => {
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.arcs.push(KiCadSymbolArc {
                        start: point(child.required_child("start")?)?,
                        mid: point(child.required_child("mid")?)?,
                        end: point(child.required_child("end")?)?,
                        stroke_width,
                        stroke_type,
                        fill_type: KiCadSymbolFillType::parse(
                            child.required_child("fill/type/@0")?,
                            log,
                        )?,
                    });
                }
                "circle" => {
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.circles.push(KiCadSymbolCircle {
                        center: point(child.required_child("center")?)?,
                        radius: child_f64(child, "radius/@0")?,
                        stroke_width,
                        stroke_type,
                        fill_type: KiCadSymbolFillType::parse(
                            child.required_child("fill/type/@0")?,
                            log,
                        )?,
                    });
                }
                "rectangle" => {
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.rectangles.push(KiCadSymbolRectangle {
                        start: point(child.required_child("start")?)?,
                        end: point(child.required_child("end")?)?,
                        stroke_width,
                        stroke_type,
                        fill_type: KiCadSymbolFillType::parse(
                            child.required_child("fill/type/@0")?,
                            log,
                        )?,
                    });
                }
                "polyline" => {
                    let coordinates = coordinates(child)?;
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.polylines.push(KiCadSymbolPolyline {
                        coordinates,
                        stroke_width,
                        stroke_type,
                        fill_type: KiCadSymbolFillType::parse(
                            child.required_child("fill/type/@0")?,
                            log,
                        )?,
                    });
                }
                "text" => {
                    let mut t = KiCadSymbolText {
                        text: value(child, "@0")?.to_owned(),
                        position: point(child.required_child("at")?)?,
                        rotation: opt_f64(child, "at/@2")?.unwrap_or(0.0),
                        font_size: SizeF::default(),
                        font_thickness: opt_f64(child, "effects/font/thickness/@0")?.unwrap_or(0.0),
                        alignment: CENTER,
                    };
                    if let Some(c) = child.child("effects/font/size") {
                        t.font_size = size(c)?;
                    }
                    if let Some(c) = child.child("effects/justify") {
                        t.alignment = deserialize_alignment(c);
                    }
                    obj.texts.push(t);
                }
                "pin" => {
                    let pin_type = KiCadPinType::parse(raw_child(child, 0)?, log)?;
                    let shape = KiCadPinStyle::parse(raw_child(child, 1)?, log)?;
                    obj.pins.push(KiCadSymbolPin {
                        pin_type,
                        shape,
                        position: point(child.required_child("at")?)?,
                        rotation: opt_f64(child, "at/@2")?.unwrap_or(0.0),
                        length: child_f64(child, "length/@0")?,
                        name: value(child, "name/@0")?.to_owned(),
                        number: value(child, "number/@0")?.to_owned(),
                    });
                }
                other => log.warning(&tr!(
                    "KiCadSymbolGate",
                    "Unsupported symbol gate child: '{0}'",
                    other
                )),
            }
        }
        Ok(obj)
    }
}

fn name_of(node: &SExpression) -> &str {
    name(node)
}

fn raw_child(node: &SExpression, index: usize) -> Result<&SExpression> {
    node.children()
        .get(index)
        .ok_or(Error::ChildNotFound(index))
}

/// A symbol of a symbol library.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbol {
    /// Name.
    pub name: String,
    /// Name of the extended symbol (may be empty).
    pub extends: String,
    /// Offset of pin names.
    pub pin_names_offset: f64,
    /// Whether pin numbers are hidden.
    pub hide_pin_numbers: bool,
    /// Whether pin names are hidden.
    pub hide_pin_names: bool,
    /// Excluded from simulation.
    pub exclude_from_sim: bool,
    /// Included in BOM.
    pub in_bom: bool,
    /// Placed on the board.
    pub on_board: bool,
    /// Properties.
    pub properties: Vec<KiCadProperty>,
    /// Gates (units).
    pub gates: Vec<KiCadSymbolGate>,
}

impl KiCadSymbol {
    /// Parses a `(symbol ...)` node.
    pub fn parse(node: &SExpression, log: &MessageLogger<'_>) -> Result<Self> {
        let mut obj = Self {
            name: value(node, "@0")?.to_owned(),
            extends: String::new(),
            pin_names_offset: 0.508,
            hide_pin_numbers: false,
            hide_pin_names: false,
            exclude_from_sim: false,
            in_bom: true,
            on_board: true,
            properties: Vec::new(),
            gates: Vec::new(),
        };
        for child in list_children(node) {
            match name_of(child) {
                "extends" => obj.extends = value(child, "@0")?.to_owned(),
                "pin_names" => {
                    obj.hide_pin_names = has_token(child, "hide");
                    if let Some(offset) = opt_f64(child, "offset/@0")? {
                        obj.pin_names_offset = offset;
                    }
                }
                "pin_numbers" => obj.hide_pin_numbers = has_token(child, "hide"),
                "exclude_from_sim" => {
                    obj.exclude_from_sim = deserialize_bool(child.required_child("@0")?)?;
                }
                "in_bom" => obj.in_bom = deserialize_bool(child.required_child("@0")?)?,
                "on_board" => obj.on_board = deserialize_bool(child.required_child("@0")?)?,
                "property" => obj.properties.push(KiCadProperty::parse(child, log)?),
                "symbol" => obj.gates.push(KiCadSymbolGate::parse(child, log)?),
                "embedded_fonts" => {} // New in KiCad v9, ignoring for now.
                other => log.warning(&tr!(
                    "KiCadSymbol",
                    "Unsupported symbol child: '{0}'",
                    other
                )),
            }
        }
        Ok(obj)
    }
}

/// A symbol library (`*.kicad_sym`).
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadSymbolLibrary {
    /// File format version.
    pub version: i32,
    /// Generator.
    pub generator: String,
    /// Symbols (symbols failing to parse are logged and skipped).
    pub symbols: Vec<KiCadSymbol>,
}

impl KiCadSymbolLibrary {
    /// Parses the root node of a `*.kicad_sym` file.
    pub fn parse(node: &SExpression, log: &MessageLogger<'_>) -> Result<Self> {
        if name(node) != "kicad_symbol_lib" {
            return Err(Error::NotASymbolLibrary);
        }
        let mut obj = Self {
            version: node.child_value("version/@0")?,
            generator: value(node, "generator/@0")?.to_owned(),
            symbols: Vec::new(),
        };
        for child in node.children_named("symbol") {
            let symbol_name = opt_value(child, "@0").ok().flatten().unwrap_or_default();
            match KiCadSymbol::parse(child, log) {
                Ok(s) => obj.symbols.push(s),
                Err(e) => log.critical(&format!("Failed to parse symbol '{symbol_name}': {e}")),
            }
        }
        Ok(obj)
    }

    /// Parses a `*.kicad_sym` file content.
    pub fn parse_file(
        content: &[u8],
        path: &std::path::Path,
        log: &MessageLogger<'_>,
    ) -> Result<Self> {
        let root = SExpression::parse(
            content,
            Some(path),
            librepcb_core::serialization::Mode::Permissive,
        )?;
        Self::parse(&root, log)
    }
}

/// A line of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprintLine {
    /// Start point.
    pub start: PointF,
    /// End point.
    pub end: PointF,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Layer.
    pub layer: KiCadLayer,
}

/// An arc of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprintArc {
    /// Start point.
    pub start: PointF,
    /// Mid point.
    pub mid: PointF,
    /// End point.
    pub end: PointF,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Layer.
    pub layer: KiCadLayer,
}

/// A circle of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprintCircle {
    /// Center.
    pub center: PointF,
    /// A point on the circle.
    pub end: PointF,
    /// Layer.
    pub layer: KiCadLayer,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Fill type.
    pub fill_type: KiCadFootprintFillType,
}

/// A rectangle of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprintRectangle {
    /// First corner.
    pub start: PointF,
    /// Second corner.
    pub end: PointF,
    /// Layer.
    pub layer: KiCadLayer,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Fill type.
    pub fill_type: KiCadFootprintFillType,
}

/// A polygon of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprintPolygon {
    /// Vertices.
    pub coordinates: Vec<PointF>,
    /// Layer.
    pub layer: KiCadLayer,
    /// Stroke width.
    pub stroke_width: f64,
    /// Stroke type.
    pub stroke_type: KiCadStrokeType,
    /// Fill type.
    pub fill_type: KiCadFootprintFillType,
}

/// A text of a footprint (`fp_text`).
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprintText {
    /// Type.
    pub text_type: KiCadSymbolTextType,
    /// Text.
    pub text: String,
    /// Position.
    pub position: PointF,
    /// Rotation \[°\].
    pub rotation: f64,
    /// Layer.
    pub layer: KiCadLayer,
    /// Font size.
    pub font_size: SizeF,
    /// Font thickness.
    pub font_thickness: f64,
    /// Alignment.
    pub alignment: Alignment,
    /// Mirror flag.
    pub mirror: bool,
    /// Unlocked flag (not kept upright).
    pub unlocked: bool,
}

/// A pad of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprintPad {
    /// Number (name).
    pub number: String,
    /// Type.
    pub pad_type: KiCadPadType,
    /// Shape.
    pub shape: KiCadPadShape,
    /// Position.
    pub position: PointF,
    /// Offset of the shape from the drill.
    pub offset: PointF,
    /// Rotation \[°\].
    pub rotation: f64,
    /// Size.
    pub size: SizeF,
    /// Drill size.
    pub drill: SizeF,
    /// Layers.
    pub layers: Vec<KiCadLayer>,
    /// Fabrication property (`None` if unspecified).
    pub property: Option<KiCadPadProperty>,
    /// Solder mask margin.
    pub solder_mask_margin: f64,
    /// Solder paste margin.
    pub solder_paste_margin: f64,
    /// Solder paste margin ratio.
    pub solder_paste_margin_ratio: f64,
    /// Thermal bridge angle.
    pub thermal_bridge_angle: f64,
    /// Thermal bridge width.
    pub thermal_bridge_width: f64,
    /// Clearance.
    pub clearance: f64,
    /// Remove unused layers flag.
    pub remove_unused_layers: bool,
    /// Corner radius ratio of rounded rectangles.
    pub round_rect_r_ratio: f64,
    /// Trapezoid delta.
    pub rect_delta: SizeF,
    /// Chamfer ratio.
    pub chamfer_ratio: f64,
    /// Chamfered edges.
    pub chamfer_edges: Vec<KiCadEdge>,
    /// Anchor of custom pads (`None` if unspecified).
    pub custom_pad_anchor: Option<KiCadCustomPadAnchor>,
    /// Lines of custom pads.
    pub graphical_lines: Vec<KiCadGraphicalLine>,
    /// Arcs of custom pads.
    pub graphical_arcs: Vec<KiCadGraphicalArc>,
    /// Circles of custom pads.
    pub graphical_circles: Vec<KiCadGraphicalCircle>,
    /// Polygons of custom pads.
    pub graphical_polygons: Vec<KiCadGraphicalPolygon>,
}

impl KiCadFootprintPad {
    /// Parses a `(pad ...)` node.
    pub fn parse(node: &SExpression, log: &MessageLogger<'_>) -> Result<Self> {
        const CTX: &str = "KiCadFootprintPad";
        let mut obj = Self {
            number: value(node, "@0")?.to_owned(),
            pad_type: KiCadPadType::parse(node.required_child("@1")?, log)?,
            shape: KiCadPadShape::parse(node.required_child("@2")?, log)?,
            position: PointF::default(),
            offset: PointF::default(),
            rotation: 0.0,
            size: SizeF::default(),
            drill: SizeF::default(),
            layers: Vec::new(),
            property: None,
            solder_mask_margin: 0.0,
            solder_paste_margin: 0.0,
            solder_paste_margin_ratio: 0.0,
            thermal_bridge_angle: 0.0,
            thermal_bridge_width: 0.0,
            clearance: 0.0,
            remove_unused_layers: false,
            round_rect_r_ratio: 0.0,
            rect_delta: SizeF::default(),
            chamfer_ratio: 0.0,
            chamfer_edges: Vec::new(),
            custom_pad_anchor: None,
            graphical_lines: Vec::new(),
            graphical_arcs: Vec::new(),
            graphical_circles: Vec::new(),
            graphical_polygons: Vec::new(),
        };
        for child in list_children(node) {
            match name_of(child) {
                "at" => {
                    obj.position = point(child)?;
                    if let Some(rot) = child.child("@2") {
                        obj.rotation = f64_of(rot)?;
                    }
                }
                "size" => obj.size = size(child)?,
                "drill" => {
                    let mut c0 = child.child("@0");
                    let mut c1 = child.child("@1");
                    let c2 = child.child("@2");
                    let is_oval =
                        c0.is_some_and(|c| c.is_token() && (c.value().ok() == Some("oval")));
                    if is_oval {
                        c0 = c1;
                        c1 = c2;
                    }
                    if let Some(c) = c0.filter(|c| c.is_token()) {
                        let v = f64_of(c)?;
                        obj.drill = SizeF {
                            width: v,
                            height: v,
                        };
                    }
                    if let Some(c) = c1.filter(|c| is_oval && c.is_token()) {
                        obj.drill.height = f64_of(c)?;
                    }
                    if let Some(offset) = child.child("offset") {
                        obj.offset = point(offset)?;
                    }
                }
                "layers" => {
                    // KiCad v6 seems to had no quotes in some cases, thus we
                    // also need to take tokens into account.
                    let strings = child.children().iter().filter(|c| c.is_string());
                    let tokens = child.children().iter().filter(|c| c.is_token());
                    for layer in strings.chain(tokens) {
                        obj.layers.push(KiCadLayer::parse(layer, log)?);
                    }
                }
                "property" => {
                    obj.property = Some(KiCadPadProperty::parse(child.required_child("@0")?, log)?);
                }
                "solder_mask_margin" => obj.solder_mask_margin = child_f64(child, "@0")?,
                "solder_paste_margin" => obj.solder_paste_margin = child_f64(child, "@0")?,
                "solder_paste_margin_ratio" => {
                    obj.solder_paste_margin_ratio = child_f64(child, "@0")?;
                }
                "thermal_bridge_angle" => obj.thermal_bridge_angle = child_f64(child, "@0")?,
                "thermal_bridge_width" => obj.thermal_bridge_width = child_f64(child, "@0")?,
                "zone_connect" | "die_length" | "keep_end_layers" => {} // Not supported yet.
                "clearance" => obj.clearance = child_f64(child, "@0")?,
                "remove_unused_layers" => {
                    obj.remove_unused_layers = deserialize_bool(child.required_child("@0")?)?;
                }
                "roundrect_rratio" => obj.round_rect_r_ratio = child_f64(child, "@0")?,
                "rect_delta" => obj.rect_delta = size(child)?,
                "chamfer_ratio" => obj.chamfer_ratio = child_f64(child, "@0")?,
                "chamfer" => {
                    for c in child.children().iter().filter(|c| c.is_token()) {
                        obj.chamfer_edges.push(KiCadEdge::parse(c, log)?);
                    }
                }
                "options" => {
                    for option in list_children(child) {
                        match name_of(option) {
                            "clearance" => {
                                let clr = value(option, "@0")?;
                                if clr != "outline" {
                                    log.warning(&tr!(CTX, "Unsupported pad clearance: '{0}'", clr));
                                }
                            }
                            "anchor" => {
                                obj.custom_pad_anchor = Some(KiCadCustomPadAnchor::parse(
                                    option.required_child("@0")?,
                                    log,
                                )?);
                            }
                            other => log.warning(&tr!(CTX, "Unsupported pad option: '{0}'", other)),
                        }
                    }
                }
                "primitives" => {
                    for p in list_children(child) {
                        match name_of(p) {
                            "gr_line" => obj.graphical_lines.push(KiCadGraphicalLine {
                                start: point(p.required_child("start")?)?,
                                end: point(p.required_child("end")?)?,
                                width: child_f64(p, "width/@0")?,
                            }),
                            "gr_arc" => obj.graphical_arcs.push(KiCadGraphicalArc {
                                start: point(p.required_child("start")?)?,
                                mid: point(p.required_child("mid")?)?,
                                end: point(p.required_child("end")?)?,
                                width: child_f64(p, "width/@0")?,
                            }),
                            "gr_circle" => obj.graphical_circles.push(KiCadGraphicalCircle {
                                center: point(p.required_child("center")?)?,
                                end: point(p.required_child("end")?)?,
                                width: child_f64(p, "width/@0")?,
                                fill: graphical_fill(p)?,
                            }),
                            "gr_poly" => obj.graphical_polygons.push(KiCadGraphicalPolygon {
                                coordinates: coordinates(p)?,
                                width: child_f64(p, "width/@0")?,
                                fill: graphical_fill(p)?,
                            }),
                            other => {
                                log.warning(&tr!(CTX, "Unsupported pad primitive: '{0}'", other))
                            }
                        }
                    }
                }
                "uuid" | "tstamp" => {} // Ignored for now.
                other => log.warning(&tr!(CTX, "Unsupported pad child: '{0}'", other)),
            }
        }
        Ok(obj)
    }
}

/// A 3D model reference of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprintModel {
    /// Path (may contain variables like `${KICAD9_3DMODEL_DIR}`).
    pub path: String,
    /// Offset \[mm\].
    pub offset: [f32; 3],
    /// Scale.
    pub scale: [f32; 3],
    /// Rotation \[°\].
    pub rotate: [f32; 3],
}

/// A footprint (`*.kicad_mod`).
#[derive(Debug, Clone, PartialEq)]
pub struct KiCadFootprint {
    /// Name.
    pub name: String,
    /// File format version (-1 if not set).
    pub version: i32,
    /// Generator.
    pub generator: String,
    /// Board side layer.
    pub layer: KiCadLayer,
    /// Description.
    pub description: String,
    /// Tags.
    pub tags: String,
    /// SMD flag.
    pub is_smd: bool,
    /// Through-hole flag.
    pub is_through_hole: bool,
    /// Board-only flag.
    pub board_only: bool,
    /// Excluded from position files.
    pub exclude_from_pos_files: bool,
    /// Excluded from BOM.
    pub exclude_from_bom: bool,
    /// Solder mask margin.
    pub solder_mask_margin: f64,
    /// Solder paste margin.
    pub solder_paste_margin: f64,
    /// Solder paste ratio.
    pub solder_paste_ratio: f64,
    /// Clearance.
    pub clearance: f64,
    /// Zone connection mode.
    pub zone_connect: Option<KiCadZoneConnect>,
    /// Net tie pad groups.
    pub net_tie_pad_groups: Vec<Vec<String>>,
    /// Properties.
    pub properties: Vec<KiCadProperty>,
    /// Lines.
    pub lines: Vec<KiCadFootprintLine>,
    /// Arcs.
    pub arcs: Vec<KiCadFootprintArc>,
    /// Circles.
    pub circles: Vec<KiCadFootprintCircle>,
    /// Rectangles.
    pub rectangles: Vec<KiCadFootprintRectangle>,
    /// Polygons.
    pub polygons: Vec<KiCadFootprintPolygon>,
    /// Texts.
    pub texts: Vec<KiCadFootprintText>,
    /// Pads.
    pub pads: Vec<KiCadFootprintPad>,
    /// Keepout zones.
    pub zones: Vec<KiCadZone>,
    /// 3D models.
    pub models: Vec<KiCadFootprintModel>,
}

fn footprint_fill(node: &SExpression, log: &MessageLogger<'_>) -> Result<KiCadFootprintFillType> {
    match node.child("fill/@0") {
        Some(c) => KiCadFootprintFillType::parse(c, log),
        None => Ok(KiCadFootprintFillType::None),
    }
}

impl KiCadFootprint {
    /// Parses the root node of a `*.kicad_mod` file.
    pub fn parse(node: &SExpression, log: &MessageLogger<'_>) -> Result<Self> {
        if !matches!(name(node), "footprint" | "module") {
            return Err(Error::NotAFootprint);
        }
        let mut obj = Self {
            name: value(node, "@0")?.to_owned(),
            version: -1,
            generator: String::new(),
            layer: KiCadLayer::Unknown,
            description: String::new(),
            tags: String::new(),
            is_smd: false,
            is_through_hole: false,
            board_only: false,
            exclude_from_pos_files: false,
            exclude_from_bom: false,
            solder_mask_margin: 0.0,
            solder_paste_margin: 0.0,
            solder_paste_ratio: 0.0,
            clearance: 0.0,
            zone_connect: None,
            net_tie_pad_groups: Vec::new(),
            properties: Vec::new(),
            lines: Vec::new(),
            arcs: Vec::new(),
            circles: Vec::new(),
            rectangles: Vec::new(),
            polygons: Vec::new(),
            texts: Vec::new(),
            pads: Vec::new(),
            zones: Vec::new(),
            models: Vec::new(),
        };
        for child in list_children(node) {
            match name_of(child) {
                "version" => obj.version = child.child_value("@0")?,
                "generator" => obj.generator = value(child, "@0")?.to_owned(),
                "generator_version" => {}
                "layer" => obj.layer = KiCadLayer::parse(child.required_child("@0")?, log)?,
                "descr" => obj.description = value(child, "@0")?.to_owned(),
                "tags" => obj.tags = value(child, "@0")?.to_owned(),
                "attr" => {
                    obj.is_smd |= has_token(child, "smd");
                    obj.is_through_hole |= has_token(child, "through_hole");
                    obj.board_only |= has_token(child, "board_only");
                    obj.exclude_from_pos_files |= has_token(child, "exclude_from_pos_files");
                    obj.exclude_from_bom |= has_token(child, "exclude_from_bom");
                }
                "solder_mask_margin" => obj.solder_mask_margin = child_f64(child, "@0")?,
                "solder_paste_ratio" => obj.solder_paste_ratio = child_f64(child, "@0")?,
                "solder_paste_margin" => obj.solder_paste_margin = child_f64(child, "@0")?,
                "clearance" => obj.clearance = child_f64(child, "@0")?,
                "zone_connect" => {
                    obj.zone_connect =
                        Some(KiCadZoneConnect::parse(child.required_child("@0")?, log)?);
                }
                "net_tie_pad_groups" => {
                    for grp in child.children().iter().filter(|c| c.is_string()) {
                        obj.net_tie_pad_groups.push(
                            grp.value()?
                                .split(',')
                                .map(str::trim)
                                .filter(|p| !p.is_empty())
                                .map(str::to_owned)
                                .collect(),
                        );
                    }
                }
                "property" => obj.properties.push(KiCadProperty::parse(child, log)?),
                "fp_line" => {
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.lines.push(KiCadFootprintLine {
                        start: point(child.required_child("start")?)?,
                        end: point(child.required_child("end")?)?,
                        stroke_width,
                        stroke_type,
                        layer: KiCadLayer::parse(child.required_child("layer/@0")?, log)?,
                    });
                }
                "fp_arc" => {
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.arcs.push(KiCadFootprintArc {
                        start: point(child.required_child("start")?)?,
                        mid: point(child.required_child("mid")?)?,
                        end: point(child.required_child("end")?)?,
                        stroke_width,
                        stroke_type,
                        layer: KiCadLayer::parse(child.required_child("layer/@0")?, log)?,
                    });
                }
                "fp_circle" => {
                    let center = point(child.required_child("center")?)?;
                    let end = point(child.required_child("end")?)?;
                    let layer = KiCadLayer::parse(child.required_child("layer/@0")?, log)?;
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.circles.push(KiCadFootprintCircle {
                        center,
                        end,
                        layer,
                        stroke_width,
                        stroke_type,
                        fill_type: footprint_fill(child, log)?,
                    });
                }
                "fp_rect" => {
                    let start = point(child.required_child("start")?)?;
                    let end = point(child.required_child("end")?)?;
                    let layer = KiCadLayer::parse(child.required_child("layer/@0")?, log)?;
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.rectangles.push(KiCadFootprintRectangle {
                        start,
                        end,
                        layer,
                        stroke_width,
                        stroke_type,
                        fill_type: footprint_fill(child, log)?,
                    });
                }
                "fp_poly" => {
                    let coordinates = coordinates(child)?;
                    let layer = KiCadLayer::parse(child.required_child("layer/@0")?, log)?;
                    let (stroke_width, stroke_type) = stroke(child, log)?;
                    obj.polygons.push(KiCadFootprintPolygon {
                        coordinates,
                        layer,
                        stroke_width,
                        stroke_type,
                        fill_type: footprint_fill(child, log)?,
                    });
                }
                "fp_text" => {
                    let text_type = KiCadSymbolTextType::parse(child.required_child("@0")?, log)?;
                    let mut t = KiCadFootprintText {
                        text_type,
                        text: value(child, "@1")?.to_owned(),
                        position: point(child.required_child("at")?)?,
                        rotation: 0.0,
                        layer: KiCadLayer::Unknown,
                        font_size: SizeF::default(),
                        font_thickness: 0.0,
                        alignment: CENTER,
                        mirror: false,
                        unlocked: false,
                    };
                    if let Some(c) = child.child("at/@2") {
                        if c.value()? == "unlocked" {
                            t.unlocked = true; // KiCad v6 compatibility.
                        } else {
                            t.rotation = f64_of(c)?;
                        }
                    }
                    t.layer = KiCadLayer::parse(child.required_child("layer/@0")?, log)?;
                    if let Some(c) = child.child("effects/font/size") {
                        t.font_size = size(c)?;
                    }
                    if let Some(v) = opt_f64(child, "effects/font/thickness/@0")? {
                        t.font_thickness = v;
                    }
                    if let Some(c) = child.child("effects/justify") {
                        t.alignment = deserialize_alignment(c);
                        t.mirror = has_token(c, "mirror");
                    }
                    if let Some(v) = opt_value(child, "unlocked/@0")? {
                        t.unlocked = v == "yes";
                    }
                    obj.texts.push(t);
                }
                "zone" => obj.zones.push(KiCadZone::parse(child, log)?),
                "pad" => obj.pads.push(KiCadFootprintPad::parse(child, log)?),
                "group" => {}
                "embedded_fonts" => {} // New in KiCad v9, ignoring for now.
                "model" => {
                    let offset = match child.child("at/xyz") {
                        Some(c) => vector3d(c)?,
                        None => vector3d(child.required_child("offset/xyz")?)?,
                    };
                    obj.models.push(KiCadFootprintModel {
                        path: raw_value(child, 0)?.to_owned(),
                        offset,
                        scale: vector3d(child.required_child("scale/xyz")?)?,
                        rotate: vector3d(child.required_child("rotate/xyz")?)?,
                    });
                }
                other => log.warning(&tr!(
                    "KiCadFootprint",
                    "Unsupported footprint child: '{0}'",
                    other
                )),
            }
        }
        Ok(obj)
    }

    /// Parses a `*.kicad_mod` file content.
    pub fn parse_file(
        content: &[u8],
        path: &std::path::Path,
        log: &MessageLogger<'_>,
    ) -> Result<Self> {
        let root = SExpression::parse(
            content,
            Some(path),
            librepcb_core::serialization::Mode::Permissive,
        )?;
        Self::parse(&root, log)
    }
}
