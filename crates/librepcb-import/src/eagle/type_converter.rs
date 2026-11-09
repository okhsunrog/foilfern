//! Port of libs/librepcb/eagleimport/eagletypeconverter.{h,cpp}.
//!
//! Conversion of EAGLE types (names, layers, geometries, pins, pads, ...)
//! into LibrePCB types.
//!
//! Differences to upstream:
//! - Element descriptions are converted from HTML by
//!   [`html_to_plain_text()`] instead of `QTextDocument` (see COMPAT.md).
//! - The layer setup is returned as `BTreeMap` (upstream `QHash`).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use librepcb_core::attribute::{Attribute, AttributeKey, AttributeList, AttributeType};
use librepcb_core::geometry::{
    Circle, ComponentSide, Hole, NonEmptyPath, Pad, PadFunction, PadHole, PadHoleList, PadShape,
    Path, Polygon, StrokeText, Text, Vertex, Zone, ZoneLayers, ZoneRules,
};
use librepcb_core::library::cmp::{ComponentPrefix, ComponentSymbolVariantItemSuffix};
use librepcb_core::library::pkg::{FootprintPad, PackagePad};
use librepcb_core::library::sym::SymbolPin;
use librepcb_core::types::{
    Alignment, Angle, BoundedUnsignedRatio, CircuitIdentifier, ElementName, HAlign, Layer, Length,
    LengthUnit, MaskConfig, Point, PositiveLength, Ratio, SimpleString, StrokeTextSpacing,
    UnsignedLength, UnsignedLimitedRatio, UnsignedRatio, VAlign,
};
use librepcb_core::utils::clipper_helpers;
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_core::utils::tangent_path_joiner;
use librepcb_i18n::tr;

use super::error::{Error, Result};
use super::model;
use crate::UuidGenerator;
use crate::html::html_to_plain_text;

const CTX: &str = "EagleTypeConverter";

/// Intermediate geometry used for converting polygon-like EAGLE elements.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    /// EAGLE layer ID.
    pub layer_id: i32,
    /// Line width.
    pub line_width: UnsignedLength,
    /// Whether the area is filled.
    pub filled: bool,
    /// Whether it is a grab area.
    pub grab_area: bool,
    /// The outline.
    pub path: Path,
    /// Center and diameter if the geometry is a circle.
    pub circle: Option<(Point, PositiveLength)>,
}

/// LibrePCB objects representing an EAGLE symbol pin.
#[derive(Debug, Clone)]
pub struct ConvertedPin {
    /// The pin.
    pub pin: SymbolPin,
    /// The inversion dot, if any.
    pub circle: Option<Circle>,
    /// The clock triangle, if any.
    pub polygon: Option<Polygon>,
}

/// Converter of EAGLE types into LibrePCB types (upstream
/// `EagleTypeConverter`).
#[derive(Debug, Clone, Default)]
pub struct EagleTypeConverter {
    uuids: UuidGenerator,
}

/// Parses a `mic`, `mm`, `mil` or `inch` length unit.
fn length_unit(unit: &str) -> Option<LengthUnit> {
    match unit {
        "mic" => Some(LengthUnit::Micrometers),
        "mm" => Some(LengthUnit::Millimeters),
        "mil" => Some(LengthUnit::Mils),
        "inch" => Some(LengthUnit::Inches),
        _ => None,
    }
}

fn grid_unit(unit: model::GridUnit) -> Option<LengthUnit> {
    match unit {
        model::GridUnit::Micrometers => Some(LengthUnit::Micrometers),
        model::GridUnit::Millimeters => Some(LengthUnit::Millimeters),
        model::GridUnit::Mils => Some(LengthUnit::Mils),
        model::GridUnit::Inches => Some(LengthUnit::Inches),
        model::GridUnit::Unknown => None,
    }
}

/// Returns whether the EAGLE layer is a silkscreen name/value/place layer
/// (minimum text size and stroke width are enforced there).
fn is_silkscreen_text_layer(layer_id: i32) -> bool {
    matches!(layer_id, 21 | 22 | 25 | 26 | 27 | 28)
}

impl EagleTypeConverter {
    /// Creates a converter creating UUIDs with `uuids`.
    pub fn new(uuids: UuidGenerator) -> Self {
        Self { uuids }
    }

    fn uuid(&self) -> librepcb_core::types::Uuid {
        self.uuids.generate()
    }

    /// Removes all invalid characters from an EAGLE element name; returns
    /// "Unnamed" if nothing is left.
    pub fn convert_element_name(&self, n: &str) -> ElementName {
        let mut name = ElementName::clean(n);
        if name.is_empty() {
            name = "Unnamed".to_owned(); // No tr() to ensure valid ElementName.
        }
        ElementName::new(name).expect("cleaned name is valid")
    }

    /// Removes HTML tags from an EAGLE element description.
    pub fn convert_element_description(&self, d: &str) -> String {
        let text = html_to_plain_text(&d.replace('\n', "<br/>"));
        text.trim()
            .split('\n')
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Like [`convert_element_name()`](Self::convert_element_name), but also
    /// removes a trailing separation character (`-` or `_`).
    pub fn convert_component_name(&self, n: &str) -> ElementName {
        let mut n = n;
        if (n.chars().count() > 1) && (n.ends_with('-') || n.ends_with('_')) {
            n = &n[..n.len() - 1];
        }
        self.convert_element_name(n)
    }

    /// Concatenates the EAGLE device set name with the device name.
    pub fn convert_device_name(&self, device_set_name: &str, device_name: &str) -> ElementName {
        let add_separator = !device_set_name.ends_with('-')
            && !device_set_name.ends_with('_')
            && !device_name.starts_with('-')
            && !device_name.starts_with('_');
        let mut name = device_set_name.to_owned();
        if add_separator && !device_name.is_empty() {
            name.push('-');
        }
        name.push_str(device_name);
        self.convert_element_name(&name)
    }

    /// Converts a device set prefix.
    pub fn convert_component_prefix(&self, p: &str) -> ComponentPrefix {
        ComponentPrefix::new(ComponentPrefix::clean(p)).expect("cleaned prefix is valid")
    }

    /// Converts a gate name into a symbol variant item suffix.
    pub fn convert_gate_name(&self, n: &str) -> ComponentSymbolVariantItemSuffix {
        ComponentSymbolVariantItemSuffix::new(ComponentSymbolVariantItemSuffix::clean(n))
            .expect("cleaned suffix is valid")
    }

    /// Converts a pin or pad name into a circuit identifier.
    pub fn convert_pin_or_pad_name(&self, n: &str) -> CircuitIdentifier {
        let mut name = self.convert_inversion_syntax(&CircuitIdentifier::clean(n));
        if (name.chars().count() > 2) && name.starts_with("P$") {
            name.replace_range(..2, "");
        }
        if name.is_empty() {
            name = "Unnamed".to_owned();
        }
        CircuitIdentifier::new(name).expect("cleaned identifier is valid")
    }

    /// Converts the inversion syntax (`"!RST!/EN"` → `"!RST/EN"`).
    pub fn convert_inversion_syntax(&self, s: &str) -> String {
        let mut out = String::new();
        let mut input_overlined = false;
        let mut output_overlined = false;
        for c in s.chars() {
            if c == '!' {
                input_overlined = !input_overlined;
                continue;
            }
            if c == '/' {
                output_overlined = false;
            }
            if input_overlined != output_overlined {
                out.push('!');
                output_overlined = input_overlined;
            }
            out.push(c);
        }
        out
    }

    /// Converts an attribute, or logs a warning and returns `None` if its
    /// name is invalid.
    pub fn try_convert_attribute(
        &self,
        a: &model::Attribute,
        log: &MessageLogger<'_>,
    ) -> Option<Attribute> {
        let key = AttributeKey::clean(&a.name);
        if key.is_empty() {
            log.warning(&format!(
                "Skipped attribute '{}' due to invalid name.",
                a.name
            ));
            return None;
        }
        let key = AttributeKey::new(key).ok()?;
        Attribute::new(key, AttributeType::String, a.value.clone(), None).ok()
    }

    /// Converts attributes (skipping invalid ones and duplicate keys) and
    /// appends them to `out`.
    pub fn try_convert_attributes(
        &self,
        input: &[model::Attribute],
        out: &mut AttributeList,
        log: &MessageLogger<'_>,
    ) {
        for eagle_attr in input {
            if let Some(obj) = self.try_convert_attribute(eagle_attr, log)
                && !out.contains_name(obj.key())
            {
                out.push(obj);
            }
        }
    }

    /// Extracts MPN and manufacturer from attributes (removing them) if
    /// `mpn`/`manufacturer` are still empty.
    pub fn try_extract_mpn_and_manufacturer(
        &self,
        attributes: &mut AttributeList,
        mpn: &mut SimpleString,
        manufacturer: &mut SimpleString,
    ) {
        for name in ["MPN", "MANUFACTURER_PART_NUMBER", "PART_NUMBER"] {
            if let Some(a) = attributes.by_name(name, true)
                && mpn.is_empty()
            {
                *mpn = SimpleString::clean(a.value());
                attributes.take_by_name(name);
            }
        }
        for name in ["MANUFACTURER", "MFR", "MF", "VENDOR"] {
            if let Some(a) = attributes.by_name(name, true)
                && manufacturer.is_empty()
            {
                *manufacturer = SimpleString::clean(a.value());
                attributes.take_by_name(name);
            }
        }
    }

    /// Converts a layer ID to a schematic/symbol layer (`None` to discard
    /// the object).
    pub fn try_convert_schematic_layer(&self, id: i32) -> Option<Layer> {
        match id {
            // 90 = modules: not sure what this layer is used for.
            // 91 = nets: in some schematics, this layer seems to be used for
            // things not related to nets at all so let's move them to the
            // documentation layer.
            // 92 = buses: probably the same as for layer 91.
            90..=92 => Some(Layer::SCHEMATIC_DOCUMENTATION),
            93 => Some(Layer::SYMBOL_PIN_NAMES),
            94 => Some(Layer::SYMBOL_OUTLINES),
            95 => Some(Layer::SYMBOL_NAMES),
            96 => Some(Layer::SYMBOL_VALUES),
            97 => Some(Layer::SCHEMATIC_DOCUMENTATION),
            98 => Some(Layer::SCHEMATIC_GUIDE),
            _ => None, // 99 = spice order
        }
    }

    /// Converts a layer ID to a board/footprint layer (`None` to discard
    /// the object).
    pub fn try_convert_board_layer(&self, id: i32) -> Option<Layer> {
        match id {
            1 => Some(Layer::TOP_COPPER),
            2..=15 => Layer::inner_copper((id - 1) as usize),
            16 => Some(Layer::BOT_COPPER),
            // Note: We cannot know whether we need to return board outlines
            // or board cutouts, but for footprints the dimension layer is
            // more likely used for cutouts.
            20 => Some(Layer::BOARD_CUTOUTS),
            21 => Some(Layer::TOP_LEGEND),
            22 => Some(Layer::BOT_LEGEND),
            25 => Some(Layer::TOP_NAMES),
            26 => Some(Layer::BOT_NAMES),
            27 => Some(Layer::TOP_VALUES),
            28 => Some(Layer::BOT_VALUES),
            29 => Some(Layer::TOP_STOP_MASK),
            30 => Some(Layer::BOT_STOP_MASK),
            31 => Some(Layer::TOP_SOLDER_PASTE),
            32 => Some(Layer::BOT_SOLDER_PASTE),
            35 => Some(Layer::TOP_GLUE),
            36 => Some(Layer::BOT_GLUE),
            46 => Some(Layer::BOARD_CUTOUTS),
            47..=50 => Some(Layer::BOARD_DOCUMENTATION),
            51 => Some(Layer::TOP_DOCUMENTATION),
            52 => Some(Layer::BOT_DOCUMENTATION),
            _ => None,
        }
    }

    /// Converts a layer setup string (e.g. `"[2:1+((2*3)+(14*15))+16:15]"`)
    /// into a map moving all inner copper layers to the top (no gaps).
    pub fn convert_layer_setup(&self, s: &str) -> Result<BTreeMap<Layer, Layer>> {
        let tmp: String = s
            .chars()
            .map(|c| if ":*+()[]".contains(c) { ' ' } else { c })
            .collect();
        let mut numbers = BTreeSet::new();
        for number_str in tmp.split(' ').filter(|p| !p.is_empty()) {
            let id: i32 = number_str.parse().unwrap_or(0);
            if !(1..=16).contains(&id) {
                return Err(Error::UnsupportedLayerSetup(s.to_owned()));
            }
            numbers.insert(id);
        }
        let mut result = BTreeMap::new();
        let mut next_inner_layer = 1;
        for id in numbers {
            if id == 1 {
                result.insert(Layer::TOP_COPPER, Layer::TOP_COPPER);
            } else if id == 16 {
                result.insert(Layer::BOT_COPPER, Layer::BOT_COPPER);
            } else {
                let (Some(from), Some(to)) = (
                    Layer::inner_copper((id - 1) as usize),
                    Layer::inner_copper(next_inner_layer),
                ) else {
                    return Err(Error::UnsupportedLayerSetup(s.to_owned()));
                };
                result.insert(from, to);
                next_inner_layer += 1;
            }
        }
        Ok(result)
    }

    /// Converts an alignment.
    pub fn convert_alignment(&self, a: model::Alignment) -> Alignment {
        use model::Alignment as A;
        let (h, v) = match a {
            A::BottomLeft | A::Unknown => (HAlign::Left, VAlign::Bottom),
            A::BottomCenter => (HAlign::Center, VAlign::Bottom),
            A::BottomRight => (HAlign::Right, VAlign::Bottom),
            A::CenterLeft => (HAlign::Left, VAlign::Center),
            A::Center => (HAlign::Center, VAlign::Center),
            A::CenterRight => (HAlign::Right, VAlign::Center),
            A::TopLeft => (HAlign::Left, VAlign::Top),
            A::TopCenter => (HAlign::Center, VAlign::Top),
            A::TopRight => (HAlign::Right, VAlign::Top),
        };
        Alignment::new(h, v)
    }

    /// Converts a length \[mm\].
    pub fn convert_length(&self, l: f64) -> Result<Length> {
        Ok(Length::from_mm(l)?)
    }

    /// Converts a line width \[mm\], applying the rules of special layers
    /// (no line width on the dimension and milling layers).
    pub fn convert_line_width(&self, w: f64, layer_id: i32) -> Result<UnsignedLength> {
        let l = match layer_id {
            20 | 46 => Length::ZERO, // dimension, milling
            _ => self.convert_length(w)?,
        };
        Ok(UnsignedLength::new(l)?)
    }

    /// Converts a design rule parameter to a length.
    pub fn convert_param_to_length(&self, p: &model::Param) -> Result<Length> {
        p.value_as_double_with_unit()
            .and_then(|(value, unit)| length_unit(&unit).map(|u| (value, u)))
            .and_then(|(value, unit)| unit.convert_from_unit(value).ok())
            .ok_or_else(|| Error::InvalidLengthParam(p.value.clone()))
    }

    /// Converts a design rule parameter to an unsigned length.
    pub fn convert_param_to_unsigned_length(&self, p: &model::Param) -> Result<UnsignedLength> {
        Ok(UnsignedLength::new(self.convert_param_to_length(p)?)?)
    }

    /// Converts a design rule parameter to a positive length.
    pub fn convert_param_to_positive_length(&self, p: &model::Param) -> Result<PositiveLength> {
        Ok(PositiveLength::new(self.convert_param_to_length(p)?)?)
    }

    /// Converts a design rule parameter to a ratio.
    pub fn convert_param_to_ratio(&self, p: &model::Param) -> Result<Ratio> {
        p.value_as_double()
            .map(Ratio::from_normalized)
            .ok_or_else(|| Error::InvalidRatioParam(p.value.clone()))
    }

    /// Converts a design rule parameter to an unsigned ratio.
    pub fn convert_param_to_unsigned_ratio(&self, p: &model::Param) -> Result<UnsignedRatio> {
        Ok(UnsignedRatio::new(self.convert_param_to_ratio(p)?)?)
    }

    /// Converts a point \[mm\].
    pub fn convert_point(&self, p: model::Point) -> Result<Point> {
        Ok(Point::from_mm(p.x, p.y)?)
    }

    /// Converts an angle \[°\].
    pub fn convert_angle(&self, a: f64) -> Result<Angle> {
        Ok(Angle::from_deg(a)?)
    }

    /// Converts grid settings; unknown/unsupported settings keep the given
    /// values.
    pub fn convert_grid(
        &self,
        g: &model::Grid,
        interval: &mut PositiveLength,
        unit: &mut LengthUnit,
    ) {
        let dist_unit = grid_unit(g.unit_distance).unwrap_or(LengthUnit::Millimeters);
        if let Ok(value) = dist_unit.convert_from_unit(g.distance)
            && g.distance > 0.0
            && let Ok(value) = PositiveLength::new(value)
        {
            *interval = value;
        }
        if let Some(u) = grid_unit(g.unit) {
            *unit = u;
        }
    }

    /// Converts a vertex.
    pub fn convert_vertex(&self, v: &model::Vertex) -> Result<Vertex> {
        Ok(Vertex::new(
            self.convert_point(v.position)?,
            self.convert_angle(v.curve)?,
        ))
    }

    /// Converts vertices into a path (closed if `close`).
    pub fn convert_vertices(&self, v: &[model::Vertex], close: bool) -> Result<Path> {
        let mut path = Path::new(
            v.iter()
                .map(|vertex| self.convert_vertex(vertex))
                .collect::<Result<_>>()?,
        );
        if close {
            path.close();
        }
        Ok(path)
    }

    /// Converts wires and joins them into polygons.
    pub fn convert_and_join_wires(
        &self,
        wires: &[model::Wire],
        is_grab_area_if_closed: bool,
        log: &MessageLogger<'_>,
    ) -> Vec<Geometry> {
        let mut polygons = Vec::new();

        let mut joinable_wires: BTreeMap<(i32, UnsignedLength), Vec<&model::Wire>> =
            BTreeMap::new();
        for wire in wires {
            let result = (|| -> Result<()> {
                let width = self.convert_line_width(wire.width, wire.layer)?;
                if (*width > Length::ZERO) && (wire.wire_cap == model::WireCap::Flat) {
                    // Convert wires with flat cap to filled polygons. This is
                    // especially important for the MOUNT-HOLE
                    // symbol/footprints, see
                    // https://librepcb.discourse.group/t/import-of-an-eagle-sch-brd-glitch/967
                    let path = Path::flat_cap_line(
                        self.convert_point(wire.p1)?,
                        self.convert_point(wire.p2)?,
                        self.convert_angle(wire.curve)?,
                        PositiveLength::new(*width)?,
                    )
                    .ok_or(Error::UnsupportedFlatCapGeometry)?;
                    polygons.push(Geometry {
                        layer_id: wire.layer,
                        line_width: UnsignedLength::ZERO, // now filled
                        filled: true,
                        grab_area: is_grab_area_if_closed,
                        path,
                        circle: None,
                    });
                } else {
                    // For any other wire, prepare them to be joined.
                    joinable_wires
                        .entry((wire.layer, width))
                        .or_default()
                        .push(wire);
                }
                Ok(())
            })();
            if let Err(e) = result {
                log.warning(&format!("Failed to convert wires: {e}"));
            }
        }

        let mut timed_out = false;
        for ((layer_id, width), wires) in &joinable_wires {
            let result = (|| -> Result<()> {
                let mut paths = Vec::new();
                for wire in wires {
                    paths.push(Path::line(
                        self.convert_point(wire.p1)?,
                        self.convert_point(wire.p2)?,
                        self.convert_angle(wire.curve)?,
                    ));
                    if wire.wire_style != model::WireStyle::Continuous {
                        log.warning(&tr!(
                            CTX,
                            "Dashed/dotted line is not supported, converting to continuous."
                        ));
                    }
                }
                let joined = tangent_path_joiner::join(paths, Some(Duration::from_millis(5000)));
                timed_out |= joined.timed_out;
                for p in joined.paths {
                    polygons.push(Geometry {
                        layer_id: *layer_id,
                        line_width: *width,
                        filled: false,
                        grab_area: is_grab_area_if_closed && p.is_closed(),
                        path: p,
                        circle: None,
                    });
                }
                Ok(())
            })();
            if let Err(e) = result {
                log.warning(&format!("Failed to convert wires: {e}"));
            }
        }
        if timed_out {
            log.info(
                "Aborted joining tangent line segments to polygons due to timeout, keeping them \
                 separate.",
            );
        }
        polygons
    }

    /// Converts a rectangle (filled).
    pub fn convert_rectangle(&self, r: &model::Rectangle, is_grab_area: bool) -> Result<Geometry> {
        let p1 = self.convert_point(r.p1)?;
        let p2 = self.convert_point(r.p2)?;
        let center = (p1 + p2) / 2;
        let rotation = self.convert_angle(r.rotation.angle)?;
        Ok(Geometry {
            layer_id: r.layer,
            line_width: UnsignedLength::ZERO,
            filled: true,
            grab_area: is_grab_area,
            path: Path::rect(p1, p2).rotated(rotation, center),
            circle: None,
        })
    }

    /// Converts a polygon (always closed and filled).
    pub fn convert_polygon(&self, p: &model::Polygon, is_grab_area: bool) -> Result<Geometry> {
        Ok(Geometry {
            layer_id: p.layer,
            line_width: self.convert_line_width(p.width, p.layer)?,
            filled: true, // EAGLE polygons are always filled
            grab_area: is_grab_area,
            path: self.convert_vertices(&p.vertices, true)?,
            circle: None,
        })
    }

    /// Converts a circle (filled if its width is zero).
    pub fn convert_circle(&self, c: &model::Circle, is_grab_area: bool) -> Result<Geometry> {
        let filled = c.width == 0.0; // EAGLE fills zero-width circles!
        let line_width = self.convert_line_width(c.width, c.layer)?;
        let pos = self.convert_point(c.position)?;
        let diameter = PositiveLength::new(self.convert_length(c.radius)? * 2)?;
        Ok(Geometry {
            layer_id: c.layer,
            line_width,
            filled,
            grab_area: is_grab_area,
            path: Path::circle(diameter).translated(pos),
            circle: Some((pos, diameter)),
        })
    }

    /// Converts a hole.
    pub fn convert_hole(&self, h: &model::Hole) -> Result<Hole> {
        Ok(Hole::new(
            self.uuid(),
            PositiveLength::new(self.convert_length(h.diameter)?)?,
            NonEmptyPath::from_point(self.convert_point(h.position)?),
            MaskConfig::Automatic,
        ))
    }

    /// Converts a frame (the inner rectangle).
    pub fn convert_frame(&self, f: &model::Frame) -> Result<Geometry> {
        let width = Length::new(3_810_000);
        let p1 = self.convert_point(f.p1)?;
        let p2 = self.convert_point(f.p2)?;
        let p1_abs = Point::new(p1.x.min(p2.x) + width, p1.y.min(p2.y) + width);
        let p2_abs = Point::new(p1.x.max(p2.x) - width, p1.y.max(p2.y) - width);
        Ok(Geometry {
            layer_id: f.layer,
            line_width: UnsignedLength::new(Length::new(200_000))?,
            filled: false,
            grab_area: false,
            path: Path::rect(p1_abs, p2_abs),
            circle: None,
        })
    }

    /// Converts a text value (`">NAME"` → `"{{NAME}}"`).
    pub fn convert_text_value(&self, v: &str) -> String {
        match v {
            ">DRAWING_NAME" => "{{PROJECT}}".to_owned(),
            ">LAST_DATE_TIME" | ">PLOT_DATE_TIME" => "{{DATE}} {{TIME}}".to_owned(),
            ">SHEET" => "{{PAGE}}/{{PAGES}}".to_owned(),
            _ => match v.strip_prefix('>') {
                Some(key) => format!("{{{{{}}}}}", key.to_uppercase()),
                None => v.to_owned(),
            },
        }
    }

    /// Converts the size of a schematic text.
    pub fn convert_schematic_text_size(&self, s: f64) -> Result<PositiveLength> {
        Ok(PositiveLength::new(Length::from_mm(s * 2.5 / 1.778)?)?)
    }

    /// Converts a schematic/symbol text, or returns `None` if its layer is
    /// not supported. Texts are locked depending on their layer, unless
    /// `allow_locked` is `false`.
    pub fn try_convert_schematic_text(
        &self,
        t: &model::Text,
        allow_locked: bool,
    ) -> Result<Option<Text>> {
        let Some(layer) = self.try_convert_schematic_layer(t.layer) else {
            return Ok(None);
        };
        let mirror = t.rotation.mirror;
        let rotation = self.convert_angle(t.rotation.angle)?;
        let alignment = self.convert_alignment(t.alignment);
        let locked =
            allow_locked && (layer != Layer::SYMBOL_NAMES) && (layer != Layer::SYMBOL_VALUES);
        Ok(Some(Text::new(
            self.uuid(),
            layer,
            self.convert_text_value(&t.value),
            self.convert_point(t.position)?,
            if mirror { -rotation } else { rotation },
            self.convert_schematic_text_size(t.size)?,
            if mirror {
                alignment.mirrored_h()
            } else {
                alignment
            },
            locked,
        )))
    }

    /// Converts a schematic/symbol attribute text, or returns `None` if its
    /// layer is not supported.
    pub fn try_convert_schematic_attribute(&self, t: &model::Attribute) -> Result<Option<Text>> {
        let Some(layer) = self.try_convert_schematic_layer(t.layer) else {
            return Ok(None);
        };
        let mirror = t.rotation.mirror;
        let rotation = self.convert_angle(t.rotation.angle)?;
        let alignment = self.convert_alignment(t.alignment);
        Ok(Some(Text::new(
            self.uuid(),
            layer,
            self.convert_text_value(&format!(">{}", t.name)),
            self.convert_point(t.position)?,
            if mirror { -rotation } else { rotation },
            self.convert_schematic_text_size(t.size)?,
            if mirror {
                alignment.mirrored_h()
            } else {
                alignment
            },
            t.constant, // Not verified yet if this is good.
        )))
    }

    /// Converts the size of a board text.
    pub fn convert_board_text_size(&self, layer_id: i32, size: f64) -> Result<PositiveLength> {
        let mut new_size = Length::from_mm(size * 0.85)?;
        // Avoid too small texts on silkscreen layers. Do not touch texts on
        // functional layers like copper to avoid possible unintended effects.
        if is_silkscreen_text_layer(layer_id) {
            new_size = new_size.max(Length::new(800_000)); // min. 0.8mm
        }
        Ok(PositiveLength::new(new_size)?)
    }

    /// Converts the stroke width of a board text (`ratio` in percent, 0
    /// means the default of 15%).
    pub fn convert_board_text_stroke_width(
        &self,
        layer_id: i32,
        size: f64,
        ratio: i32,
    ) -> Result<UnsignedLength> {
        let ratio = if ratio == 0 { 15 } else { ratio };
        let mut width = Length::from_mm((size * f64::from(ratio)) / 100.0)?;
        // Avoid too thin texts on silkscreen layers. Do not touch texts on
        // functional layers like copper to avoid possible unintended effects.
        if is_silkscreen_text_layer(layer_id) {
            width = width.max(Length::new(150_000)); // min. 150um
        }
        Ok(UnsignedLength::new(width)?)
    }

    /// Converts a board/footprint text, or returns `None` if its layer is
    /// not supported.
    pub fn try_convert_board_text(
        &self,
        t: &model::Text,
        allow_locked: bool,
    ) -> Result<Option<StrokeText>> {
        let Some(layer) = self.try_convert_board_layer(t.layer) else {
            return Ok(None);
        };
        let locked = allow_locked
            && ![
                Layer::TOP_NAMES,
                Layer::TOP_VALUES,
                Layer::BOT_NAMES,
                Layer::BOT_VALUES,
            ]
            .contains(&layer);
        self.board_text(layer, &t.value, t, locked).map(Some)
    }

    /// Converts a board/footprint attribute text, or returns `None` if its
    /// layer is not supported.
    pub fn try_convert_board_attribute(&self, t: &model::Attribute) -> Result<Option<StrokeText>> {
        let Some(layer) = self.try_convert_board_layer(t.layer) else {
            return Ok(None);
        };
        let text = model::Text {
            layer: t.layer,
            font: t.font,
            size: t.size,
            ratio: t.ratio,
            position: t.position,
            rotation: t.rotation,
            alignment: t.alignment,
            value: String::new(),
        };
        // Locked: not verified yet if the "constant" flag is good.
        self.board_text(layer, &format!(">{}", t.name), &text, t.constant)
            .map(Some)
    }

    fn board_text(
        &self,
        layer: Layer,
        value: &str,
        t: &model::Text,
        locked: bool,
    ) -> Result<StrokeText> {
        let mirror = t.rotation.mirror;
        let rotation = self.convert_angle(t.rotation.angle)?;
        Ok(StrokeText::new(
            self.uuid(),
            layer,
            self.convert_text_value(value),
            self.convert_point(t.position)?,
            if mirror { -rotation } else { rotation },
            self.convert_board_text_size(t.layer, t.size)?,
            self.convert_board_text_stroke_width(t.layer, t.size, t.ratio)?,
            StrokeTextSpacing::default(),
            StrokeTextSpacing::default(),
            self.convert_alignment(t.alignment),
            mirror,
            !t.rotation.spin,
            locked,
        ))
    }

    /// Converts a symbol pin (with inversion dot and clock triangle).
    pub fn convert_symbol_pin(&self, p: &model::Pin) -> Result<ConvertedPin> {
        use model::PinFunction as F;
        let is_dot = matches!(p.function, F::Dot | F::DotClock);
        let is_clock = matches!(p.function, F::Clock | F::DotClock);
        let dot_diameter = Length::new(if is_dot { 1_700_000 } else { 0 });
        let total_length = UnsignedLength::new(self.convert_length(p.length_mm())?)?;
        let pin = SymbolPin::new(
            self.uuid(),
            self.convert_pin_or_pad_name(&p.name),
            self.convert_point(p.position)?,
            UnsignedLength::new((*total_length - dot_diameter).max(Length::ZERO))?,
            self.convert_angle(p.rotation.angle)?,
            Point::new(*total_length + Length::new(2_540_000), Length::ZERO),
            Angle::DEG0,
            SymbolPin::default_name_height(),
            Alignment::new(HAlign::Left, VAlign::Center),
        );
        let line_width = UnsignedLength::new(Length::new(158_750))?;
        let circle = if is_dot {
            Some(Circle::new(
                self.uuid(),
                Layer::SYMBOL_OUTLINES,
                line_width,
                false,
                false,
                Point::new(*total_length - dot_diameter / 2, Length::ZERO)
                    .rotated(pin.rotation(), Point::ORIGIN)
                    + pin.position(),
                PositiveLength::new(dot_diameter)?,
            ))
        } else {
            None
        };
        let polygon = if is_clock {
            let dy = Length::new(900_000);
            let dx = Length::new(1_900_000);
            let path = Path::new(vec![
                Vertex::at(Point::new(*total_length, dy)),
                Vertex::at(Point::new(*total_length + dx, Length::ZERO)),
                Vertex::at(Point::new(*total_length, -dy)),
            ])
            .rotated(pin.rotation(), Point::ORIGIN)
            .translated(pin.position());
            Some(Polygon::new(
                self.uuid(),
                Layer::SYMBOL_OUTLINES,
                line_width,
                false,
                false,
                path,
            ))
        } else {
            None
        };
        Ok(ConvertedPin {
            pin,
            circle,
            polygon,
        })
    }

    /// Converts a THT pad. `auto_annular_width` defines the pad size if it
    /// is set to "auto" in EAGLE (see
    /// [`default_auto_tht_annular_width()`](Self::default_auto_tht_annular_width)).
    pub fn convert_tht_pad(
        &self,
        p: &model::ThtPad,
        auto_annular_width: &BoundedUnsignedRatio,
    ) -> Result<(PackagePad, FootprintPad)> {
        let uuid = self.uuid();
        let drill_diameter = PositiveLength::new(self.convert_length(p.drill_diameter)?)?;
        let mut size = self.convert_length(p.outer_diameter)?;
        if size <= Length::ZERO {
            // If the pad size is set to "auto", it will be zero.
            let annular = auto_annular_width.calc_value(*drill_diameter);
            size = *drill_diameter + *annular * 2;
        }
        let mut width = PositiveLength::new(size)?;
        let height = PositiveLength::new(size)?;
        let mut radius = UnsignedLimitedRatio::new(Ratio::from_percent(0))?;
        let mut custom_shape_outline = Path::default();
        let full = UnsignedLimitedRatio::new(Ratio::from_percent(100))?;
        let shape = match p.shape {
            model::PadShape::Square => PadShape::RoundedRect,
            model::PadShape::Octagon => PadShape::RoundedOctagon,
            model::PadShape::Round => {
                radius = full;
                PadShape::RoundedRect
            }
            model::PadShape::Long => {
                radius = full;
                width = PositiveLength::new(size * 2)?;
                PadShape::RoundedRect
            }
            model::PadShape::Offset => {
                radius = full;
                width = PositiveLength::new(size * 2)?;
                custom_shape_outline =
                    Path::obround(width, height).translated(Point::new(size / 2, Length::ZERO));
                PadShape::Custom
            }
            model::PadShape::Unknown => {
                return Err(Error::UnknownPadShape(format!("{:?}", p.shape)));
            }
        };
        let pad = Pad::new(
            uuid,
            self.convert_point(p.position)?,
            self.convert_angle(p.rotation.angle)?,
            shape,
            width,
            height,
            radius,
            custom_shape_outline,
            if p.stop {
                MaskConfig::Automatic
            } else {
                MaskConfig::Off
            },
            MaskConfig::Off,
            UnsignedLength::ZERO,
            ComponentSide::Top,
            PadFunction::Unspecified,
            PadHoleList::from(vec![PadHole::new(
                self.uuid(),
                drill_diameter,
                NonEmptyPath::from_point(Point::ORIGIN),
            )]),
        );
        Ok((
            PackagePad::new(uuid, self.convert_pin_or_pad_name(&p.name)),
            FootprintPad::new(pad, Some(uuid)),
        ))
    }

    /// Converts an SMT pad.
    pub fn convert_smt_pad(&self, p: &model::SmtPad) -> Result<(PackagePad, FootprintPad)> {
        let uuid = self.uuid();
        let side = match self.try_convert_board_layer(p.layer) {
            Some(Layer::TOP_COPPER) => ComponentSide::Top,
            Some(Layer::BOT_COPPER) => ComponentSide::Bottom,
            _ => return Err(Error::InvalidPadLayer(p.layer)),
        };
        let pad = Pad::new(
            uuid,
            self.convert_point(p.position)?,
            self.convert_angle(p.rotation.angle)?,
            PadShape::RoundedRect,
            PositiveLength::new(self.convert_length(p.width)?)?,
            PositiveLength::new(self.convert_length(p.height)?)?,
            UnsignedLimitedRatio::new(Ratio::from_percent(p.roundness))?,
            Path::default(),
            if p.stop {
                MaskConfig::Automatic
            } else {
                MaskConfig::Off
            },
            if p.cream {
                MaskConfig::Automatic
            } else {
                MaskConfig::Off
            },
            UnsignedLength::ZERO,
            side,
            PadFunction::Unspecified,
            PadHoleList::new(),
        );
        Ok((
            PackagePad::new(uuid, self.convert_pin_or_pad_name(&p.name)),
            FootprintPad::new(pad, Some(uuid)),
        ))
    }

    /// Converts a geometry to a schematic circle, if it is a circle on a
    /// schematic layer.
    pub fn try_convert_to_schematic_circle(&self, g: &Geometry) -> Option<Circle> {
        let (center, diameter) = g.circle?;
        let layer = self.try_convert_schematic_layer(g.layer_id)?;
        Some(Circle::new(
            self.uuid(),
            layer,
            g.line_width,
            g.filled,
            g.grab_area,
            center,
            diameter,
        ))
    }

    /// Converts a geometry to a schematic polygon, if it is on a schematic
    /// layer.
    pub fn try_convert_to_schematic_polygon(&self, g: &Geometry) -> Option<Polygon> {
        let layer = self.try_convert_schematic_layer(g.layer_id)?;
        Some(Polygon::new(
            self.uuid(),
            layer,
            g.line_width,
            g.filled,
            g.grab_area,
            g.path.clone(),
        ))
    }

    /// Converts the outline of a keepout zone: zones have no line width, so
    /// the outline is offset by half the line width.
    pub fn convert_board_zone_outline(
        &self,
        outline: &Path,
        line_width: Length,
    ) -> Result<Vec<Path>> {
        let max_arc_tolerance = PositiveLength::new(Length::new(10_000))?;
        if (line_width / 2) > *max_arc_tolerance {
            let mut paths = vec![clipper_helpers::path_to_clipper(outline, max_arc_tolerance)];
            clipper_helpers::offset(
                &mut paths,
                line_width / 2,
                max_arc_tolerance,
                clipper::JoinType::Round,
            )?;
            Ok(clipper_helpers::paths_from_clipper(&paths))
        } else {
            Ok(vec![outline.clone()])
        }
    }

    /// Converts a geometry to board keepout zones, if it is on a keepout or
    /// restrict layer.
    pub fn try_convert_to_board_zones(&self, g: &Geometry) -> Result<Vec<Zone>> {
        let copper = ZoneRules::NO_COPPER | ZoneRules::NO_PLANES;
        let (layers, rules) = match g.layer_id {
            39 => (ZoneLayers::TOP, ZoneRules::NO_DEVICES), // tKeepout
            40 => (ZoneLayers::BOTTOM, ZoneRules::NO_DEVICES), // bKeepout
            41 => (ZoneLayers::TOP, copper),                // tRestrict
            42 => (ZoneLayers::BOTTOM, copper),             // bRestrict
            43 => (ZoneLayers::INNER, copper),              // vRestrict
            _ => return Ok(Vec::new()),
        };
        Ok(self
            .convert_board_zone_outline(&g.path, *g.line_width)?
            .into_iter()
            .map(|outline| Zone::new(self.uuid(), layers, rules, outline))
            .collect())
    }

    /// Converts a geometry to a board circle, if it is a circle on a board
    /// layer.
    pub fn try_convert_to_board_circle(&self, g: &Geometry) -> Option<Circle> {
        let (center, diameter) = g.circle?;
        let layer = self.try_convert_board_layer(g.layer_id)?;
        Some(Circle::new(
            self.uuid(),
            layer,
            g.line_width,
            g.filled,
            g.grab_area,
            center,
            diameter,
        ))
    }

    /// Converts a geometry to a board polygon, if it is on a board layer.
    pub fn try_convert_to_board_polygon(&self, g: &Geometry) -> Option<Polygon> {
        let layer = self.try_convert_board_layer(g.layer_id)?;
        Some(Polygon::new(
            self.uuid(),
            layer,
            g.line_width,
            g.filled,
            g.grab_area,
            g.path.clone(),
        ))
    }

    /// Returns the EAGLE layer name of a layer ID (or `fallback`).
    pub fn layer_name(id: i32, fallback: &str) -> String {
        let name = match id {
            1 => "tCu",
            2..=15 => return format!("Route{id}"),
            16 => "bCu",
            17 => "Pads",
            18 => "Vias",
            19 => "Unrouted",
            20 => "Dimension",
            21 => "tPlace",
            22 => "bPlace",
            23 => "tOrigins",
            24 => "bOrigins",
            25 => "tNames",
            26 => "bNames",
            27 => "tValues",
            28 => "bValues",
            29 => "tStop",
            30 => "bStop",
            31 => "tCream",
            32 => "bCream",
            33 => "tFinish",
            34 => "bFinish",
            35 => "tGlue",
            36 => "bGlue",
            37 => "tTest",
            38 => "bTest",
            39 => "tKeepout",
            40 => "bKeepout",
            41 => "tRestrict",
            42 => "bRestrict",
            43 => "vRestrict",
            44 => "Drills",
            45 => "Holes",
            46 => "Milling",
            47 => "Measures",
            48 => "Document",
            49 => "ReferenceLC",
            50 => "ReferenceLS",
            51 => "tDocu",
            52 => "bDocu",
            90 => "Modules",
            91 => "Nets",
            92 => "Buses",
            93 => "Pins",
            94 => "Symbols",
            95 => "Names",
            96 => "Values",
            97 => "Info",
            98 => "Guide",
            99 => "Spice Order",
            _ => fallback,
        };
        name.to_owned()
    }

    /// Returns the annular width of THT pads with "auto" size used by the
    /// EAGLE footprint editor: 25% of the drill diameter, bounded to
    /// 10..20mils (0.254..0.508mm).
    pub fn default_auto_tht_annular_width() -> BoundedUnsignedRatio {
        BoundedUnsignedRatio::new(
            UnsignedRatio::new(Ratio::from_percent(25)).expect("25% is unsigned"),
            UnsignedLength::new(Length::new(254_000)).expect("constant is unsigned"),
            UnsignedLength::new(Length::new(508_000)).expect("constant is unsigned"),
        )
        .expect("min <= max")
    }
}
