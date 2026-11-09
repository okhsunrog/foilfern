//! Port of libs/librepcb/kicadimport/kicadtypeconverter.{h,cpp}.
//!
//! Conversion of KiCad types (see [`types`](super::types)) into LibrePCB
//! types.
//!
//! Differences to upstream:
//! - A converter object holds the [`UuidGenerator`] (upstream: static
//!   methods using `Uuid::createRandom()`).
//! - Lines are grouped by layer in the order of the LibrePCB layer list
//!   (upstream: by the memory address of the layer objects), see COMPAT.md.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{
    Circle, ComponentSide, Hole, NonEmptyPath, Pad, PadFunction, PadHole, PadHoleList, PadShape,
    Path, Polygon, StrokeText, Text, Vertex, Zone, ZoneLayers, ZoneRules,
};
use librepcb_core::library::pkg::FootprintPad;
use librepcb_core::library::sym::SymbolPin;
use librepcb_core::library::{Resource, ResourceList};
use librepcb_core::types::{
    Angle, CircuitIdentifier, ElementName, Layer, Length, MaskConfig, Point, PositiveLength, Ratio,
    StrokeTextSpacing, UnsignedLength, UnsignedLimitedRatio,
};
use librepcb_core::utils::clipper_helpers;
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_core::utils::toolbox;

use super::error::{Error, Result};
use super::types::*;
use crate::UuidGenerator;

/// A line or arc of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Layer.
    pub layer: Layer,
    /// Width.
    pub width: UnsignedLength,
    /// Start point.
    pub start: Point,
    /// End point.
    pub end: Point,
    /// Arc angle.
    pub angle: Angle,
}

/// Lines with the same layer and width, to be joined.
#[derive(Debug, Clone, PartialEq)]
pub struct LineGroup {
    /// Layer.
    pub layer: Layer,
    /// Width.
    pub width: UnsignedLength,
    /// Paths of the lines.
    pub paths: Vec<Path>,
}

/// LibrePCB objects replacing a KiCad pad.
#[derive(Debug, Clone, Default)]
pub struct PadReplacements {
    /// Footprint pad (not connected to a package pad yet).
    pub fpt_pad: Option<FootprintPad>,
    /// Non-plated hole.
    pub hole: Option<Hole>,
    /// Polygons (pads without copper, e.g. solder paste apertures).
    pub polygons: Vec<Polygon>,
}

/// Converter of KiCad types into LibrePCB types.
#[derive(Debug, Clone, Default)]
pub struct KiCadTypeConverter {
    uuids: UuidGenerator,
}

fn max_arc_tolerance() -> PositiveLength {
    PositiveLength::new(Length::new(5000)).expect("constant is positive")
}

fn mm(v: f64) -> Result<Length> {
    Ok(Length::from_mm(v)?)
}

fn deg(v: f64) -> Result<Angle> {
    Ok(Angle::from_deg(v)?)
}

/// Fixes "keep upright": texts rotated by (90°..270°] are turned by 180°.
fn keep_upright(rotation: Angle) -> Angle {
    let r = rotation.mapped_to_0_360deg();
    if (r > Angle::DEG90) && (r <= Angle::DEG270) {
        rotation + Angle::DEG180
    } else {
        rotation
    }
}

impl KiCadTypeConverter {
    /// Creates a converter creating UUIDs with `uuids`.
    pub fn new(uuids: UuidGenerator) -> Self {
        Self { uuids }
    }

    fn uuid(&self) -> librepcb_core::types::Uuid {
        self.uuids.generate()
    }

    /// Converts an element name (removing invalid characters).
    pub fn convert_element_name(s: &str) -> Result<ElementName> {
        Ok(ElementName::new(ElementName::clean(s))?)
    }

    /// Builds the element description from the `description` property and
    /// the source reference.
    pub fn convert_element_description(
        lib_basename: &str,
        elem_name: &str,
        props: &[KiCadProperty],
    ) -> String {
        let mut s = Self::find_property(props, "description")
            .map(|p| p.value.trim().to_owned())
            .unwrap_or_default();
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(&format!(
            "Imported from KiCad ({lib_basename}:{elem_name})."
        ));
        s
    }

    /// Builds the keywords from the common keywords and the `ki_keywords`
    /// property.
    pub fn convert_element_keywords(common: &str, props: &[KiCadProperty]) -> String {
        let mut ret = common.to_owned();
        if let Some(prop) = Self::find_property(props, "ki_keywords") {
            for keyword in prop
                .value
                .split(' ')
                .map(str::trim)
                .filter(|k| !k.is_empty())
            {
                ret.push(',');
                ret.push_str(keyword);
            }
        }
        ret
    }

    /// Converts the `datasheet` property into a resource.
    pub fn convert_resources(props: &[KiCadProperty]) -> ResourceList {
        let mut ret = ResourceList::new();
        if let Some(prop) = Self::find_property(props, "datasheet") {
            let url = prop.value.trim();
            if !url.is_empty() && (prop.value.chars().count() > 1) {
                // Ignore "~".
                ret.push(Resource::new(
                    ElementName::new("Datasheet").expect("valid name"),
                    "application/pdf",
                    url,
                ));
            }
        }
        ret
    }

    /// Converts a symbol point \[mm\].
    pub fn convert_symbol_point(p: PointF) -> Result<Point> {
        Ok(Point::from_mm(p.x, p.y)?)
    }

    /// Converts a footprint point \[mm\] (Y axis inverted).
    pub fn convert_footprint_point(p: PointF) -> Result<Point> {
        Ok(Point::from_mm(p.x, -p.y)?)
    }

    /// Converts an arc given by 3 points into its angle (rounded to 45°
    /// multiples if close to them).
    pub fn convert_arc(start: Point, mid: Point, end: Point) -> Angle {
        let mut angle = toolbox::arc_angle_from_3_points(start, mid, end);
        if (angle.mapped_to_0_360deg() % Angle::DEG45) < Angle::new(100_000) {
            angle = angle.rounded(Angle::DEG45);
        }
        angle
    }

    /// Converts a symbol stroke width (0 = KiCad's default of 6 mils).
    pub fn convert_symbol_stroke_width(width: f64) -> Result<UnsignedLength> {
        if width <= 0.0 {
            Ok(UnsignedLength::new(Length::from_mil(6.0)?)?) // Default KiCad width.
        } else {
            Ok(UnsignedLength::new(mm(width)?)?)
        }
    }

    /// Converts a symbol text height.
    pub fn convert_symbol_text_height(height: f64) -> Result<PositiveLength> {
        Ok(PositiveLength::new(mm(height * 2.5 / 1.27)?)?)
    }

    /// Converts a footprint stroke width (0 on board edge and area layers).
    pub fn convert_footprint_stroke_width(width: f64, layer: Layer) -> Result<UnsignedLength> {
        if layer.is_board_edge() || layer.polygons_represent_areas() {
            Ok(UnsignedLength::ZERO)
        } else {
            Ok(UnsignedLength::new(mm(width)?)?)
        }
    }

    /// Converts a symbol arc.
    pub fn convert_symbol_arc(&self, a: &KiCadSymbolArc) -> Result<Polygon> {
        let fill = a.fill_type == KiCadSymbolFillType::Outline;
        let grab_area = a.fill_type == KiCadSymbolFillType::Background;
        let pa = Self::convert_symbol_point(a.start)?;
        let pm = Self::convert_symbol_point(a.mid)?;
        let pb = Self::convert_symbol_point(a.end)?;
        let mut path = Path::line(pa, pb, Self::convert_arc(pa, pm, pb));
        if fill {
            path.close(); // KiCad fills even if not closed, but LibrePCB doesn't.
        }
        Ok(Polygon::new(
            self.uuid(),
            Layer::SYMBOL_OUTLINES,
            Self::convert_symbol_stroke_width(a.stroke_width)?,
            fill,
            grab_area,
            path,
        ))
    }

    /// Converts a symbol circle.
    pub fn convert_symbol_circle(&self, c: &KiCadSymbolCircle) -> Result<Circle> {
        Ok(Circle::new(
            self.uuid(),
            Layer::SYMBOL_OUTLINES,
            Self::convert_symbol_stroke_width(c.stroke_width)?,
            c.fill_type == KiCadSymbolFillType::Outline,
            c.fill_type == KiCadSymbolFillType::Background,
            Self::convert_symbol_point(c.center)?,
            PositiveLength::new(mm(c.radius)? * 2)?,
        ))
    }

    /// Converts a symbol rectangle.
    pub fn convert_symbol_rectangle(&self, r: &KiCadSymbolRectangle) -> Result<Polygon> {
        Ok(Polygon::new(
            self.uuid(),
            Layer::SYMBOL_OUTLINES,
            Self::convert_symbol_stroke_width(r.stroke_width)?,
            r.fill_type == KiCadSymbolFillType::Outline,
            r.fill_type == KiCadSymbolFillType::Background,
            Path::rect(
                Self::convert_symbol_point(r.start)?,
                Self::convert_symbol_point(r.end)?,
            ),
        ))
    }

    /// Converts a symbol polyline.
    pub fn convert_symbol_polyline(&self, p: &KiCadSymbolPolyline) -> Result<Polygon> {
        let fill = p.fill_type == KiCadSymbolFillType::Outline;
        let mut path = Path::new(
            p.coordinates
                .iter()
                .map(|c| Ok(Vertex::at(Self::convert_symbol_point(*c)?)))
                .collect::<Result<_>>()?,
        );
        if path.vertices().len() < 2 {
            return Err(Error::TooFewVertices);
        }
        if fill {
            path.close(); // KiCad fills even if not closed, but LibrePCB doesn't.
        }
        Ok(Polygon::new(
            self.uuid(),
            Layer::SYMBOL_OUTLINES,
            Self::convert_symbol_stroke_width(p.stroke_width)?,
            fill,
            p.fill_type == KiCadSymbolFillType::Background,
            path,
        ))
    }

    /// Converts a symbol text (locked, it's probably pure graphics).
    pub fn convert_symbol_text(&self, t: &KiCadSymbolText) -> Result<Text> {
        Ok(Text::new(
            self.uuid(),
            Layer::SYMBOL_OUTLINES,
            t.text.clone(),
            // Note: Upstream converts the position like footprint points.
            Self::convert_footprint_point(t.position)?,
            keep_upright(deg(t.rotation)?),
            Self::convert_symbol_text_height(t.font_size.height)?,
            t.alignment,
            true,
        ))
    }

    /// Converts the `Reference`/`Value` property of a symbol into a text,
    /// or returns `None` for other or hidden properties.
    pub fn convert_symbol_property_to_text(&self, p: &KiCadProperty) -> Result<Option<Text>> {
        let key = p.key.to_lowercase();
        let (layer, text) = if (key == "reference") && !p.hide {
            (Layer::SYMBOL_NAMES, "{{NAME}}")
        } else if (key == "value") && !p.hide {
            (Layer::SYMBOL_VALUES, "{{VALUE}}")
        } else {
            return Ok(None);
        };
        // Skip default-generated properties which are not displayed in KiCad.
        if p.value.is_empty() && p.position.is_null() {
            return Ok(None);
        }
        let mut rotation = deg(p.rotation)?;
        if !p.unlocked {
            rotation = keep_upright(rotation);
        }
        Ok(Some(Text::new(
            self.uuid(),
            layer,
            text,
            Self::convert_symbol_point(p.position)?,
            rotation,
            Self::convert_symbol_text_height(p.font_size.height)?,
            p.alignment,
            false,
        )))
    }

    /// Determines the LibrePCB pin names of symbol pins: pins with identical
    /// name and position are merged (their numbers joined, the merged pins
    /// get an empty name), names are made unique. Returns (name, numbers)
    /// for each pin.
    pub fn convert_symbol_pin_names(pins: &[KiCadSymbolPin]) -> Vec<(String, Vec<String>)> {
        // Merge pins with identical name and position into a single pin.
        struct MergedPin {
            name: String,
            numbers: Vec<String>,
            position: PointF,
        }
        let mut merged: Vec<MergedPin> = Vec::new();
        for pin in pins {
            let numbers: Vec<String> = if pin.number.is_empty() {
                Vec::new()
            } else {
                vec![pin.number.clone()]
            };
            if let Some(existing) = merged
                .iter_mut()
                .find(|m| (m.name == pin.name) && (m.position == pin.position))
            {
                // There's already a pin at the same position -> just append
                // the pin number.
                existing.numbers.extend(numbers);
                merged.push(MergedPin {
                    name: String::new(),
                    numbers: Vec::new(),
                    position: PointF::default(),
                }); // Ignore pin.
            } else {
                merged.push(MergedPin {
                    name: pin.name.clone(),
                    numbers,
                    position: pin.position,
                });
            }
        }

        // Determine new names.
        let mut used_pin_names = BTreeSet::new();
        let names: Vec<String> = merged
            .iter()
            .map(|pin| {
                if pin.name.is_empty() {
                    String::new()
                } else {
                    let name = Self::convert_symbol_pin_name(&pin.name, &pin.numbers).into_string();
                    used_pin_names.insert(name.clone());
                    name
                }
            })
            .collect();

        // Make names unique.
        let mut ret = Vec::new();
        for (i, pin) in merged.into_iter().enumerate() {
            let mut name = names[i].clone();
            if !name.is_empty() {
                if names.iter().filter(|n| **n == name).count() > 1 {
                    let mut number = 1;
                    loop {
                        name = format!("{}_{}", names[i], number);
                        number += 1;
                        if !used_pin_names.contains(&name) {
                            break;
                        }
                    }
                }
                used_pin_names.insert(name.clone());
            }
            ret.push((name, pin.numbers));
        }
        ret
    }

    /// Converts a pin name (the numbers if the name is empty or `~`).
    pub fn convert_symbol_pin_name(name: &str, numbers: &[String]) -> CircuitIdentifier {
        if name.is_empty() || (name == "~") {
            Self::convert_circuit_identifier(&numbers.join(","))
        } else {
            Self::convert_circuit_identifier(name)
        }
    }

    /// Converts a text with KiCad overline markup (`~{RST}`) into a circuit
    /// identifier with LibrePCB markup (`!RST`).
    pub fn convert_circuit_identifier(text: &str) -> CircuitIdentifier {
        // Parse KiCad markup.
        let chars: Vec<char> = text.chars().collect();
        let mut parsed: Vec<(char, bool)> = Vec::new();
        let mut inverted = false;
        let mut i = 0;
        while i < chars.len() {
            if (chars[i] == '~') && (chars.get(i + 1) == Some(&'{')) {
                inverted = true;
                i += 1;
            } else if (chars[i] == '}') && inverted {
                inverted = false;
            } else {
                parsed.push((chars[i], inverted));
            }
            i += 1;
        }

        // Convert to LibrePCB markup.
        let mut out = String::new();
        let mut inverted = false;
        for (ch, invert) in parsed {
            if !inverted && invert {
                // Switch on inversion.
                if ch == '/' {
                    out.push('!');
                }
                out.push('!');
                inverted = true;
            } else if inverted && !invert {
                // Switch off inversion.
                if ch != '/' {
                    out.push('!');
                }
                inverted = false;
            } else if inverted && (ch == '/') {
                // Keep inversion on even though a slash follows.
                out.push('!');
            }
            out.push(ch);
        }

        // Remove invalid characters.
        let mut out = CircuitIdentifier::clean(&out);
        if out.is_empty() {
            out = "UNNAMED".to_owned();
        }
        CircuitIdentifier::new(out).expect("cleaned identifier is valid")
    }

    /// Converts a symbol pin with the given (converted) name.
    pub fn convert_symbol_pin(
        &self,
        p: &KiCadSymbolPin,
        name: &str,
        pin_names_offset: f64,
    ) -> Result<SymbolPin> {
        let length = UnsignedLength::new(mm(p.length)?)?;
        Ok(SymbolPin::new(
            self.uuid(),
            CircuitIdentifier::new(name)?,
            Self::convert_symbol_point(p.position)?,
            length,
            deg(p.rotation)?,
            Point::new(*length + mm(pin_names_offset)?, Length::ZERO),
            Angle::DEG0,
            SymbolPin::default_name_height(),
            SymbolPin::default_name_alignment(),
        ))
    }

    /// Converts the layer of a footprint geometry.
    pub fn convert_footprint_geometry_layer(l: KiCadLayer) -> Result<Layer> {
        Ok(match l {
            KiCadLayer::FrontAdhesion => Layer::TOP_GLUE,
            KiCadLayer::FrontCopper => Layer::TOP_COPPER,
            KiCadLayer::FrontCourtyard => Layer::TOP_COURTYARD,
            KiCadLayer::FrontFabrication => Layer::TOP_DOCUMENTATION,
            KiCadLayer::FrontPaste => Layer::TOP_SOLDER_PASTE,
            KiCadLayer::FrontSilkscreen => Layer::TOP_LEGEND,
            KiCadLayer::FrontSolderMask => Layer::TOP_STOP_MASK,
            KiCadLayer::BackAdhesion => Layer::BOT_GLUE,
            KiCadLayer::BackCopper => Layer::BOT_COPPER,
            KiCadLayer::BackCourtyard => Layer::BOT_COURTYARD,
            KiCadLayer::BackFabrication => Layer::BOT_DOCUMENTATION,
            KiCadLayer::BackPaste => Layer::BOT_SOLDER_PASTE,
            KiCadLayer::BackSilkscreen => Layer::BOT_LEGEND,
            KiCadLayer::BackSolderMask => Layer::BOT_STOP_MASK,
            KiCadLayer::BoardOutline => Layer::BOARD_OUTLINES,
            KiCadLayer::UserComment => Layer::BOARD_COMMENTS,
            KiCadLayer::UserDrawing => Layer::BOARD_DOCUMENTATION,
            other => return Err(Error::UnsupportedGeometryLayer(other.index().to_string())),
        })
    }

    /// Converts a footprint line.
    pub fn convert_footprint_line(l: &KiCadFootprintLine) -> Result<Line> {
        let layer = Self::convert_footprint_geometry_layer(l.layer)?;
        Ok(Line {
            layer,
            width: Self::convert_footprint_stroke_width(l.stroke_width, layer)?,
            start: Self::convert_footprint_point(l.start)?,
            end: Self::convert_footprint_point(l.end)?,
            angle: Angle::DEG0,
        })
    }

    /// Converts a footprint arc.
    pub fn convert_footprint_arc(a: &KiCadFootprintArc) -> Result<Line> {
        let pa = Self::convert_footprint_point(a.start)?;
        let pm = Self::convert_footprint_point(a.mid)?;
        let pb = Self::convert_footprint_point(a.end)?;
        let layer = Self::convert_footprint_geometry_layer(a.layer)?;
        Ok(Line {
            layer,
            width: Self::convert_footprint_stroke_width(a.stroke_width, layer)?,
            start: pa,
            end: pb,
            angle: Self::convert_arc(pa, pm, pb),
        })
    }

    /// Groups lines by layer and width.
    pub fn group_lines_by_layer_and_width(lines: &[Line]) -> Vec<LineGroup> {
        let mut map: BTreeMap<(Layer, UnsignedLength), Vec<&Line>> = BTreeMap::new();
        for line in lines {
            map.entry((line.layer, line.width)).or_default().push(line);
        }
        map.into_iter()
            .map(|((layer, width), lines)| LineGroup {
                layer,
                width,
                paths: lines
                    .iter()
                    .map(|l| Path::line(l.start, l.end, l.angle))
                    .collect(),
            })
            .collect()
    }

    /// Converts a footprint circle (circles thinner than their line width
    /// become filled).
    pub fn convert_footprint_circle(&self, c: &KiCadFootprintCircle) -> Result<Circle> {
        let center = Self::convert_footprint_point(c.center)?;
        let end = Self::convert_footprint_point(c.end)?;
        let mut diameter = *(end - center).length() * 2;
        let layer = Self::convert_footprint_geometry_layer(c.layer)?;
        let mut line_width = Self::convert_footprint_stroke_width(c.stroke_width, layer)?;
        let mut fill = c.fill_type == KiCadFootprintFillType::Solid;
        if diameter <= *line_width {
            diameter += *line_width;
            line_width = UnsignedLength::ZERO;
            fill = true;
        }
        Ok(Circle::new(
            self.uuid(),
            layer,
            line_width,
            fill,
            false,
            center,
            PositiveLength::new(diameter)?,
        ))
    }

    /// Converts a footprint rectangle.
    pub fn convert_footprint_rectangle(&self, r: &KiCadFootprintRectangle) -> Result<Polygon> {
        let layer = Self::convert_footprint_geometry_layer(r.layer)?;
        Ok(Polygon::new(
            self.uuid(),
            layer,
            Self::convert_footprint_stroke_width(r.stroke_width, layer)?,
            r.fill_type == KiCadFootprintFillType::Solid,
            false,
            Path::rect(
                Self::convert_footprint_point(r.start)?,
                Self::convert_footprint_point(r.end)?,
            ),
        ))
    }

    /// Converts a footprint polygon (always closed).
    pub fn convert_footprint_polygon(&self, p: &KiCadFootprintPolygon) -> Result<Polygon> {
        let layer = Self::convert_footprint_geometry_layer(p.layer)?;
        let mut path = Path::new(
            p.coordinates
                .iter()
                .map(|c| Ok(Vertex::at(Self::convert_footprint_point(*c)?)))
                .collect::<Result<_>>()?,
        );
        if path.vertices().len() < 2 {
            return Err(Error::TooFewVertices);
        }
        path.close(); // KiCad polygons are always closed.
        Ok(Polygon::new(
            self.uuid(),
            layer,
            Self::convert_footprint_stroke_width(p.stroke_width, layer)?,
            p.fill_type == KiCadFootprintFillType::Solid,
            false,
            path,
        ))
    }

    /// Converts a footprint keepout zone (`None` if it has no supported
    /// layers or rules).
    pub fn convert_footprint_zone(
        &self,
        z: &KiCadZone,
        log: &MessageLogger<'_>,
    ) -> Result<Option<Zone>> {
        let has = |l: KiCadLayer| z.layers.contains(&l);
        let mut layers = ZoneLayers::empty();
        layers.set(
            ZoneLayers::TOP,
            has(KiCadLayer::FrontCopper)
                || has(KiCadLayer::FrontAndBackCopper)
                || has(KiCadLayer::AllCopper),
        );
        layers.set(
            ZoneLayers::INNER,
            has(KiCadLayer::InnerCopper(1)) || has(KiCadLayer::AllCopper),
        );
        layers.set(
            ZoneLayers::BOTTOM,
            has(KiCadLayer::BackCopper)
                || has(KiCadLayer::FrontAndBackCopper)
                || has(KiCadLayer::AllCopper),
        );
        if layers.is_empty() {
            log.warning("Zone without any layers, will be discarded.");
            return Ok(None);
        }

        let mut rules = ZoneRules::empty();
        rules.set(
            ZoneRules::NO_COPPER,
            z.keep_out_tracks || z.keep_out_vias || z.keep_out_pads,
        );
        rules.set(ZoneRules::NO_PLANES, z.keep_out_copper_pour);
        rules.set(ZoneRules::NO_DEVICES, z.keep_out_footprints);
        if !z.keep_out_tracks && (z.keep_out_vias || z.keep_out_pads) {
            log.warning("Via- or pad-keepout in zone is not supported, will be ignored.");
        }
        if rules.is_empty() {
            log.warning("Zone without any rules, will be discarded.");
            return Ok(None);
        }

        let mut outline = Path::new(
            z.polygon
                .iter()
                .map(|p| Ok(Vertex::at(Self::convert_footprint_point(*p)?)))
                .collect::<Result<_>>()?,
        );
        outline.open();
        outline.clean();
        if outline.vertices().len() < 3 {
            log.warning("Invalid zone outline, will be discarded.");
            return Ok(None);
        }
        Ok(Some(Zone::new(self.uuid(), layers, rules, outline)))
    }

    /// Converts a footprint text (locked, it's probably pure graphics), or
    /// `None` for the reference on the documentation layer.
    pub fn convert_footprint_text(&self, t: &KiCadFootprintText) -> Result<Option<StrokeText>> {
        // Discard value on documentation layer as we don't use that.
        if t.text == "${REFERENCE}" {
            return Ok(None);
        }
        let mut rotation = deg(t.rotation)?;
        if !t.unlocked {
            rotation = keep_upright(rotation);
        }
        let layer = Self::convert_footprint_geometry_layer(t.layer)?;
        Ok(Some(StrokeText::new(
            self.uuid(),
            layer,
            t.text.clone(),
            Self::convert_footprint_point(t.position)?,
            rotation,
            PositiveLength::new(mm(t.font_size.height)?)?,
            UnsignedLength::new(mm(t.font_thickness)?)?,
            StrokeTextSpacing::default(),
            StrokeTextSpacing::default(),
            t.alignment,
            t.mirror,
            !t.unlocked,
            true,
        )))
    }

    /// Converts the `Reference`/`Value` property of a footprint into a
    /// stroke text, or `None` for other or hidden properties.
    pub fn convert_footprint_property_to_text(
        &self,
        p: &KiCadProperty,
    ) -> Result<Option<StrokeText>> {
        let key = p.key.to_lowercase();
        let (layer, text) = if (key == "reference") && (p.value == "REF**") && !p.hide {
            (Layer::TOP_NAMES, "{{NAME}}")
        } else if (key == "value") && !p.hide {
            (Layer::TOP_VALUES, "{{VALUE}}")
        } else {
            return Ok(None);
        };
        // Skip default-generated properties which are not displayed in KiCad.
        if p.value.is_empty() && p.position.is_null() {
            return Ok(None);
        }
        let mut rotation = deg(p.rotation)?;
        if !p.unlocked {
            rotation = keep_upright(rotation);
        }
        Ok(Some(StrokeText::new(
            self.uuid(),
            layer,
            text,
            Self::convert_footprint_point(p.position)?,
            rotation,
            PositiveLength::new(mm(p.font_size.height)?)?,
            UnsignedLength::new(mm(p.font_thickness)?)?,
            StrokeTextSpacing::default(),
            StrokeTextSpacing::default(),
            p.alignment,
            p.mirror,
            !p.unlocked,
            false,
        )))
    }

    /// Converts a pad into a footprint pad, a hole and/or polygons, using
    /// the footprint-wide margins/clearance where the pad doesn't specify
    /// them.
    #[allow(clippy::too_many_arguments)]
    pub fn convert_pad(
        &self,
        p: &KiCadFootprintPad,
        fpt_solder_mask_margin: f64,
        fpt_solder_paste_margin: f64,
        fpt_solder_paste_ratio: f64,
        fpt_clearance: f64,
        log: &MessageLogger<'_>,
    ) -> Result<PadReplacements> {
        // Convert layers.
        let layer_map = |l: KiCadLayer| -> Option<Layer> {
            Some(match l {
                KiCadLayer::FrontCopper => Layer::TOP_COPPER,
                KiCadLayer::FrontAdhesion => Layer::TOP_GLUE,
                KiCadLayer::FrontPaste => Layer::TOP_SOLDER_PASTE,
                KiCadLayer::FrontSilkscreen => Layer::TOP_LEGEND,
                KiCadLayer::FrontSolderMask => Layer::TOP_STOP_MASK,
                KiCadLayer::BackCopper => Layer::BOT_COPPER,
                KiCadLayer::BackAdhesion => Layer::BOT_GLUE,
                KiCadLayer::BackPaste => Layer::BOT_SOLDER_PASTE,
                KiCadLayer::BackSilkscreen => Layer::BOT_LEGEND,
                KiCadLayer::BackSolderMask => Layer::BOT_STOP_MASK,
                _ => return None,
            })
        };
        let mut layers: BTreeSet<Layer> = BTreeSet::new();
        let mut handled: BTreeSet<Layer> = BTreeSet::new();
        for &ki_layer in &p.layers {
            if let Some(layer) = layer_map(ki_layer) {
                layers.insert(layer);
            } else if matches!(
                ki_layer,
                KiCadLayer::AllCopper | KiCadLayer::FrontAndBackCopper
            ) {
                layers.insert(Layer::TOP_COPPER);
                layers.insert(Layer::BOT_COPPER);
            } else if ki_layer == KiCadLayer::AllSolderMask {
                layers.insert(Layer::TOP_STOP_MASK);
                layers.insert(Layer::BOT_STOP_MASK);
            } else if matches!(
                ki_layer,
                KiCadLayer::AllSilkscreen
                    | KiCadLayer::FrontSilkscreen
                    | KiCadLayer::BackSilkscreen
            ) {
                // Note: Unreachable for F/B.SilkS (mapped above), like upstream.
                log.warning("Silkscreen enabled on pad, don't know what to do with it.");
            } else {
                log.warning(&format!(
                    "Unsupported layer {} enabled on pad.",
                    ki_layer.index()
                ));
            }
        }
        let has = |l: Layer| layers.contains(&l);

        // Detect & convert pad shape.
        let width = PositiveLength::new(mm(p.size.width)?)?;
        let height = PositiveLength::new(mm(p.size.height)?)?;
        let min_size = (*width).min(*height);
        let ratio = |percent| UnsignedLimitedRatio::new(Ratio::from_percent(percent));
        let mut radius = ratio(0)?;
        let mut shape = PadShape::RoundedRect;
        let mut custom_shape_outline = Path::default();
        let actual_shape_outline;
        if matches!(p.shape, KiCadPadShape::Circle | KiCadPadShape::Oval) {
            // Circle or obround.
            radius = ratio(100)?;
            actual_shape_outline = Path::obround(width, height);
        } else if (p.shape == KiCadPadShape::Rect)
            || ((p.shape == KiCadPadShape::RoundRect) && (p.round_rect_r_ratio == 0.0))
            || ((p.shape == KiCadPadShape::Trapezoid) && p.rect_delta.is_null())
        {
            let chamfer_size = min_size.scaled(p.chamfer_ratio);
            if (chamfer_size > Length::ZERO) && !p.chamfer_edges.is_empty() {
                // Chamfered rect.
                let edge = |e| p.chamfer_edges.contains(&e);
                shape = PadShape::Custom;
                custom_shape_outline = Path::chamfered_rect(
                    width,
                    height,
                    UnsignedLength::new(chamfer_size)?,
                    edge(KiCadEdge::TopLeft),
                    edge(KiCadEdge::TopRight),
                    edge(KiCadEdge::BottomLeft),
                    edge(KiCadEdge::BottomRight),
                );
                actual_shape_outline = custom_shape_outline.clone();
            } else {
                // Plain rect.
                actual_shape_outline = Path::centered_rect(width, height, UnsignedLength::ZERO);
            }
        } else if p.shape == KiCadPadShape::RoundRect {
            // Rounded rect.
            radius = UnsignedLimitedRatio::new(Ratio::from_normalized(
                (p.round_rect_r_ratio * 2.0).clamp(0.0, 1.0),
            ))?;
            actual_shape_outline = Path::centered_rect(
                width,
                height,
                UnsignedLength::new(min_size.scaled(p.round_rect_r_ratio))?,
            );
            if (p.chamfer_ratio > 0.0) && !p.chamfer_edges.is_empty() {
                log.warning("Pads with mixed rounded and chamfered edges are not supported yet.");
            }
        } else if p.shape == KiCadPadShape::Trapezoid {
            // Trapezoidal.
            shape = PadShape::Custom;
            custom_shape_outline = Path::trapezoid(
                width,
                height,
                -mm(p.rect_delta.height)?,
                -mm(p.rect_delta.width)?,
            );
            actual_shape_outline = custom_shape_outline.clone();
        } else if p.shape == KiCadPadShape::Custom {
            let (s, r, custom, actual) = self.convert_custom_pad_shape(p, width, height, log)?;
            shape = s;
            radius = r;
            custom_shape_outline = custom;
            actual_shape_outline = actual;
        } else {
            log.critical(&format!(
                "Unsupported pad shape {}, using circular shape instead.",
                pad_shape_index(p.shape)
            ));
            radius = ratio(100)?;
            actual_shape_outline = Path::obround(width, height);
        }
        let mut actual_shape_outline = actual_shape_outline;
        custom_shape_outline.open(); // Considered as closed by LibrePCB.
        custom_shape_outline.clean();
        actual_shape_outline.close(); // Must be closed for polygons.
        actual_shape_outline.clean();
        let rotation = deg(p.rotation)?;
        let actual_shape_outline = actual_shape_outline
            .rotated(rotation, Point::ORIGIN)
            .translated(Self::convert_footprint_point(p.position)?);
        if actual_shape_outline.vertices().len() < 2 {
            log.critical("Pad shape detection failed.");
        }

        // Convert drill.
        let mut drill: Option<(PositiveLength, Path)> = None;
        let drill_width = mm(p.drill.width.max(0.0))?;
        let drill_height = mm(p.drill.height.max(0.0))?;
        let is_tht_type = matches!(
            p.pad_type,
            KiCadPadType::ThruHole | KiCadPadType::NpThruHole
        );
        if (drill_width > Length::ZERO) && (drill_height > Length::ZERO) {
            let diameter = PositiveLength::new(drill_width.min(drill_height))?;
            let path = if drill_width != drill_height {
                let dx = drill_width - *diameter;
                let dy = drill_height - *diameter;
                Path::line(
                    Point::new(-dx / 2, dy / 2),
                    Point::new(dx / 2, -dy / 2),
                    Angle::DEG0,
                )
            } else {
                Path::new(vec![Vertex::at(Point::ORIGIN)])
            };
            drill = Some((diameter, path));
        } else if is_tht_type {
            return Err(Error::MissingDrill);
        }
        if ((drill_width > Length::ZERO) || (drill_height > Length::ZERO)) && !is_tht_type {
            log.warning("SMD pad has a drill diameter specified, it will be ignored.");
            drill = None;
        }

        // Determine stop mask config.
        let mut has_stop_mask = false;
        if drill.is_some() && (has(Layer::TOP_STOP_MASK) || has(Layer::BOT_STOP_MASK)) {
            has_stop_mask = true;
            if !has(Layer::TOP_STOP_MASK) || !has(Layer::BOT_STOP_MASK) {
                log.warning("THT pad with stop mask only on one side is not supported.");
            }
        } else if has(Layer::TOP_COPPER) && !has(Layer::BOT_COPPER) && has(Layer::TOP_STOP_MASK) {
            has_stop_mask = true;
            if has(Layer::BOT_STOP_MASK) {
                log.warning("SMD pad with stop mask on both sides is not supported.");
            }
        } else if has(Layer::BOT_COPPER) && !has(Layer::TOP_COPPER) && has(Layer::BOT_STOP_MASK) {
            has_stop_mask = true;
            if has(Layer::TOP_STOP_MASK) {
                log.warning("SMD pad with stop mask on both sides is not supported.");
            }
        }
        let solder_mask_margin = if p.solder_mask_margin != 0.0 {
            p.solder_mask_margin
        } else {
            fpt_solder_mask_margin
        };
        let stop_mask_config = if !has_stop_mask {
            MaskConfig::Off
        } else if solder_mask_margin != 0.0 {
            MaskConfig::Manual(mm(solder_mask_margin)?)
        } else {
            MaskConfig::Automatic
        };

        // Determine copper clearance.
        let copper_clearance = UnsignedLength::new(mm(p.clearance.max(fpt_clearance))?)?;

        // Handle pad.
        let mut result = PadReplacements::default();
        if (p.pad_type != KiCadPadType::NpThruHole)
            && (has(Layer::TOP_COPPER) || has(Layer::BOT_COPPER))
        {
            // Determine pad type/side.
            let mut holes = PadHoleList::new();
            let cmp_side;
            if let (KiCadPadType::ThruHole, Some((diameter, path)), true, true) = (
                p.pad_type,
                &drill,
                has(Layer::TOP_COPPER),
                has(Layer::BOT_COPPER),
            ) {
                // It's a THT pad.
                cmp_side = ComponentSide::Top;
                holes.push(PadHole::new(
                    self.uuid(),
                    *diameter,
                    NonEmptyPath::new(path.translated(-Self::convert_footprint_point(p.offset)?))?,
                ));
                handled.extend([
                    Layer::TOP_COPPER,
                    Layer::BOT_COPPER,
                    Layer::TOP_STOP_MASK,
                    Layer::BOT_STOP_MASK,
                    Layer::TOP_SOLDER_PASTE,
                    Layer::BOT_SOLDER_PASTE,
                ]);
            } else if has(Layer::TOP_COPPER) && !has(Layer::BOT_COPPER) {
                // It's a top side pad.
                cmp_side = ComponentSide::Top;
                handled.extend([
                    Layer::TOP_COPPER,
                    Layer::TOP_STOP_MASK,
                    Layer::TOP_SOLDER_PASTE,
                ]);
            } else if has(Layer::BOT_COPPER) && !has(Layer::TOP_COPPER) {
                // It's a bottom side pad.
                cmp_side = ComponentSide::Bottom;
                handled.extend([
                    Layer::BOT_COPPER,
                    Layer::BOT_STOP_MASK,
                    Layer::BOT_SOLDER_PASTE,
                ]);
            } else {
                return Err(Error::UnsupportedPadConfiguration);
            }

            // Determine solder paste config.
            let mut has_solder_paste = false;
            if !holes.is_empty() && (has(Layer::TOP_SOLDER_PASTE) || has(Layer::BOT_SOLDER_PASTE)) {
                has_solder_paste = true;
                if !has(Layer::TOP_SOLDER_PASTE) || !has(Layer::BOT_SOLDER_PASTE) {
                    log.warning("THT pad with solder paste only on one side is not supported.");
                }
            } else if (cmp_side == ComponentSide::Top) && has(Layer::TOP_SOLDER_PASTE) {
                has_solder_paste = true;
                if has(Layer::BOT_SOLDER_PASTE) {
                    log.warning("SMD pad with solder paste on both sides is not supported.");
                }
            } else if (cmp_side == ComponentSide::Bottom) && has(Layer::BOT_SOLDER_PASTE) {
                has_solder_paste = true;
                if has(Layer::TOP_SOLDER_PASTE) {
                    log.warning("SMD pad with solder paste on both sides is not supported.");
                }
            }
            let solder_paste_margin = if p.solder_paste_margin != 0.0 {
                p.solder_paste_margin
            } else {
                fpt_solder_paste_margin
            };
            let solder_paste_ratio = if p.solder_paste_margin_ratio != 0.0 {
                p.solder_paste_margin_ratio
            } else {
                fpt_solder_paste_ratio
            };
            let solder_paste_config = if !has_solder_paste {
                MaskConfig::Off
            } else if (solder_paste_margin != 0.0) || (solder_paste_ratio != 0.0) {
                MaskConfig::Manual(-mm(solder_paste_margin)? - min_size.scaled(solder_paste_ratio))
            } else {
                MaskConfig::Automatic
            };

            // Determine pad function.
            let function = match p.property {
                Some(KiCadPadProperty::Bga) => PadFunction::BgaPad,
                Some(KiCadPadProperty::FiducialGlobal) => PadFunction::GlobalFiducial,
                Some(KiCadPadProperty::FiducialLocal) => PadFunction::LocalFiducial,
                Some(KiCadPadProperty::Testpoint) => PadFunction::TestPad,
                Some(KiCadPadProperty::Heatsink) => PadFunction::ThermalPad,
                _ if p.pad_type == KiCadPadType::Connect => PadFunction::EdgeConnectorPad,
                _ => PadFunction::Unspecified,
            };

            // Determine positioning.
            let position = Self::convert_footprint_point(p.position)?
                + Self::convert_footprint_point(p.offset)?.rotated(rotation, Point::ORIGIN);

            result.fpt_pad = Some(FootprintPad::new(
                Pad::new(
                    self.uuid(),
                    position,
                    rotation,
                    shape,
                    width,
                    height,
                    radius,
                    custom_shape_outline,
                    stop_mask_config,
                    solder_paste_config,
                    copper_clearance,
                    cmp_side,
                    function,
                    holes,
                ),
                None,
            ));
        }

        // Handle NPTH.
        if p.pad_type == KiCadPadType::NpThruHole
            && let Some((diameter, path)) = &drill
        {
            handled.extend([Layer::TOP_STOP_MASK, Layer::BOT_STOP_MASK]);
            result.hole = Some(Hole::new(
                self.uuid(),
                *diameter,
                NonEmptyPath::new(
                    path.rotated(rotation, Point::ORIGIN)
                        .translated(Self::convert_footprint_point(p.position)?),
                )?,
                stop_mask_config,
            ));
            let copper = has(Layer::TOP_COPPER) || has(Layer::BOT_COPPER);
            if ((*width > drill_width) || (*height > drill_height)) && copper {
                log.critical("NPTH with copper on top and/or bottom side is not supported.");
            }
            if (*copper_clearance > Length::ZERO) && copper {
                log.critical("Copper clearance on NPTH is not supported and will be ignored.");
            }
            handled.extend([Layer::TOP_COPPER, Layer::BOT_COPPER]);
        }

        // Handle polygon. No idea why they call it a pad when meaning polygon.
        if (p.pad_type == KiCadPadType::Smd)
            && result.fpt_pad.is_none()
            && !actual_shape_outline.vertices().is_empty()
        {
            // It's only a polygon, who knows why they call it pad.
            for &ki_layer in &p.layers {
                if let Some(layer) = layer_map(ki_layer) {
                    result.polygons.push(Polygon::new(
                        self.uuid(),
                        layer,
                        UnsignedLength::ZERO,
                        true,
                        false,
                        actual_shape_outline.clone(),
                    ));
                    handled.insert(layer);
                } else {
                    log.critical(&format!(
                        "SMD aperture with unsupported layer {}.",
                        p.layers.first().map_or(0, |l| l.index())
                    ));
                }
            }
        }

        // Fail on unsupported pad.
        if result.fpt_pad.is_none() && result.hole.is_none() && result.polygons.is_empty() {
            return Err(Error::PadNotConverted(p.number.clone()));
        }

        // Warn about unhandled layers.
        for layer in layers.difference(&handled) {
            log.critical(&format!(
                "Don't know what to do with layer '{}' on pad.",
                layer.name_tr()
            ));
        }

        Ok(result)
    }

    /// Converts the primitives of a custom pad into (shape, radius, custom
    /// outline, actual outline).
    fn convert_custom_pad_shape(
        &self,
        p: &KiCadFootprintPad,
        width: PositiveLength,
        height: PositiveLength,
        log: &MessageLogger<'_>,
    ) -> Result<(PadShape, UnsignedLimitedRatio, Path, Path)> {
        use clipper::{JoinType, PolyFillType};
        let tol = max_arc_tolerance();
        let mut paths = clipper_helpers::ClipperPaths::new();
        let add_to_paths = |paths: &mut clipper_helpers::ClipperPaths,
                            path: &Path,
                            width: Length|
         -> Result<()> {
            let mut tmp = vec![clipper_helpers::path_to_clipper(path, tol)];
            if (width / 2) > Length::ZERO {
                clipper_helpers::offset(&mut tmp, width / 2, tol, JoinType::Round)?;
            }
            clipper_helpers::unite_with(paths, &tmp, PolyFillType::EvenOdd, PolyFillType::NonZero)?;
            Ok(())
        };
        for line in &p.graphical_lines {
            let start = Self::convert_footprint_point(line.start)?;
            let end = Self::convert_footprint_point(line.end)?;
            add_to_paths(
                &mut paths,
                &Path::obround_line(start, end, PositiveLength::new(mm(line.width)?)?),
                Length::ZERO,
            )?;
        }
        for arc in &p.graphical_arcs {
            let start = Self::convert_footprint_point(arc.start)?;
            let mid = Self::convert_footprint_point(arc.mid)?;
            let end = Self::convert_footprint_point(arc.end)?;
            let angle = Self::convert_arc(start, mid, end);
            add_to_paths(
                &mut paths,
                &Path::arc_obround(start, end, angle, PositiveLength::new(mm(arc.width)?)?),
                Length::ZERO,
            )?;
        }
        for circle in &p.graphical_circles {
            let center = Self::convert_footprint_point(circle.center)?;
            let end = Self::convert_footprint_point(circle.end)?;
            let diameter = PositiveLength::new(*(end - center).length() * 2)?;
            let w = mm(circle.width)?;
            let outer = *diameter + w;
            let hole = *diameter - w;
            if (outer > Length::ZERO) && (circle.fill || (hole <= Length::ZERO)) {
                add_to_paths(
                    &mut paths,
                    &Path::circle(PositiveLength::new(outer)?).translated(center),
                    Length::ZERO,
                )?;
            } else if (outer > hole) && (hole > Length::ZERO) {
                add_to_paths(
                    &mut paths,
                    &Path::donut(PositiveLength::new(outer)?, PositiveLength::new(hole)?)
                        .translated(center),
                    Length::ZERO,
                )?;
            } else {
                log.warning("Strange circle in custom pad shape ignored.");
            }
        }
        for polygon in &p.graphical_polygons {
            let outline = Path::new(
                polygon
                    .coordinates
                    .iter()
                    .map(|c| Ok(Vertex::at(Self::convert_footprint_point(*c)?)))
                    .collect::<Result<_>>()?,
            );
            add_to_paths(&mut paths, &outline, mm(polygon.width)?)?;
        }
        // Add pad anchor.
        let (radius, actual, anchor) = if p.custom_pad_anchor == Some(KiCadCustomPadAnchor::Rect) {
            (
                UnsignedLimitedRatio::new(Ratio::from_percent(0))?,
                Path::centered_rect(width, height, UnsignedLength::ZERO),
                Path::centered_rect(width, height, UnsignedLength::ZERO),
            )
        } else {
            if p.custom_pad_anchor != Some(KiCadCustomPadAnchor::Circle) {
                log.critical(&format!(
                    "Invalid custom pad anchor {}, using circular shape.",
                    match p.custom_pad_anchor {
                        Some(KiCadCustomPadAnchor::Unknown) => 0,
                        None => 1,
                        Some(KiCadCustomPadAnchor::Circle) => 2,
                        Some(KiCadCustomPadAnchor::Rect) => 3,
                    }
                ));
            }
            (
                UnsignedLimitedRatio::new(Ratio::from_percent(100))?,
                Path::obround(width, height),
                Path::obround(width, height),
            )
        };
        if !paths.is_empty() {
            add_to_paths(&mut paths, &anchor, Length::ZERO)?;
        }
        let tree = clipper_helpers::unite_with_to_tree(
            &paths,
            &[],
            PolyFillType::EvenOdd,
            PolyFillType::EvenOdd,
        )?;
        let paths = clipper_helpers::flatten_tree(tree.root())?;
        if let Some(first) = paths.first() {
            if paths.len() > 1 {
                log.critical(
                    "Custom pad shape consists of multiple separated primitives, considering \
                     only one of them.",
                );
            }
            let custom = clipper_helpers::path_from_clipper(first);
            Ok((PadShape::Custom, radius, custom.clone(), custom))
        } else {
            log.critical("Custom pad shape does not have a custom shape set.");
            Ok((PadShape::RoundedRect, radius, Path::default(), actual))
        }
    }

    /// Returns the first property with the given key (case insensitive).
    pub fn find_property<'a>(props: &'a [KiCadProperty], key: &str) -> Option<&'a KiCadProperty> {
        let key = key.to_lowercase();
        props.iter().find(|p| p.key.to_lowercase() == key)
    }
}

/// Index of the pad shape in upstream's enum (for messages).
fn pad_shape_index(shape: KiCadPadShape) -> i32 {
    match shape {
        KiCadPadShape::Unknown => 0,
        KiCadPadShape::Circle => 1,
        KiCadPadShape::Rect => 2,
        KiCadPadShape::Oval => 3,
        KiCadPadShape::Trapezoid => 4,
        KiCadPadShape::RoundRect => 5,
        KiCadPadShape::Custom => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(name: &str, number: &str, x: f64) -> KiCadSymbolPin {
        KiCadSymbolPin {
            pin_type: KiCadPinType::Passive,
            shape: KiCadPinStyle::Line,
            position: PointF { x, y: 0.0 },
            rotation: 0.0,
            length: 2.54,
            name: name.to_owned(),
            number: number.to_owned(),
        }
    }

    #[test]
    fn test_convert_circuit_identifier() {
        let c = KiCadTypeConverter::convert_circuit_identifier;
        assert_eq!("!RST", c("~{RST}").as_str());
        assert_eq!("A/!B", c("A/~{B}").as_str());
        assert_eq!("!A!/B", c("~{A/B}").as_str());
        assert_eq!("UNNAMED", c("").as_str());
    }

    #[test]
    fn test_convert_symbol_pin_names() {
        let names = KiCadTypeConverter::convert_symbol_pin_names(&[
            pin("GND", "1", 0.0),
            pin("GND", "2", 0.0),
            pin("IO", "3", 1.0),
            pin("IO", "4", 2.0),
            pin("~", "5", 3.0),
        ]);
        assert_eq!(
            vec![
                ("GND".to_owned(), vec!["1".to_owned(), "2".to_owned()]),
                (String::new(), vec![]),
                ("IO_1".to_owned(), vec!["3".to_owned()]),
                ("IO_2".to_owned(), vec!["4".to_owned()]),
                ("5".to_owned(), vec!["5".to_owned()]),
            ],
            names
        );
    }

    #[test]
    fn test_convert_arc() {
        let p = |x, y| Point::from_mm(x, y).unwrap();
        let angle = KiCadTypeConverter::convert_arc(p(0.0, 0.0), p(1.0, 1.0), p(2.0, 0.0));
        assert_eq!(Angle::DEG180, angle.abs());
    }
}
