//! Port of libs/librepcb/core/project/projectjsonexport.{h,cpp}.
//!
//! Exports general information about a project and its boards (layer
//! count, colors, drills, bounding box, ...) as JSON, e.g. for PCB
//! fabrication services. The values are built as [`serde_json::Value`]s;
//! [`to_utf8()`] writes them byte-identical to upstream's
//! `QJsonDocument::toJson(Indented)`: keys sorted (`QJsonObject` is a sorted
//! map), four spaces indentation, empty arrays as `[` newline `]`, and
//! floating point numbers formatted like `QByteArray::number(d, 'g',
//! QLocale::FloatingPointShortest)` (shortest round-trip digits, exponent
//! form only where it is shorter). This formatting is ported by hand
//! because no crate reproduces Qt's number format.
//!
//! Differences to upstream: the functions take the [`Project`] as context
//! where upstream objects have back-pointers (e.g. [`board_to_json()`]).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use super::Project;
use super::board::{Board, BoardDevice};
use super::circuit::AssemblyVariant;
use crate::geometry::Path;
use crate::library::pkg::Footprint;
use crate::types::{Layer, Length, PcbColor, Point};
use crate::utils::painter_path;

/// A bounding box given by two opposite corners, or `None` if empty
/// (upstream `ProjectJsonExport::BoundingBox`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BoundingBox(pub Option<(Point, Point)>);

/// The diameters of drills of one kind (upstream
/// `ProjectJsonExport::ToolList`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolList {
    /// The diameter of each drill (with duplicates).
    pub diameters: Vec<Length>,
}

/// Returns a JSON array of strings.
pub fn string_list_to_json<S: AsRef<str>>(list: &[S]) -> Value {
    Value::Array(
        list.iter()
            .map(|s| Value::String(s.as_ref().to_owned()))
            .collect(),
    )
}

/// Returns a length in millimeters.
pub fn length_to_json(length: Length) -> Value {
    f64_to_json(length.to_mm())
}

/// Returns a length in millimeters, or `null`.
pub fn optional_length_to_json(length: Option<Length>) -> Value {
    length.map_or(Value::Null, length_to_json)
}

/// Returns a sorted array of lengths in millimeters.
pub fn length_set_to_json(lengths: &BTreeSet<Length>) -> Value {
    Value::Array(lengths.iter().map(|l| length_to_json(*l)).collect())
}

/// Returns the color identifier, or `"none"` (not `null`, to distinguish
/// it from an unknown color).
pub fn pcb_color_to_json(color: Option<PcbColor>) -> Value {
    Value::String(color.map_or("none", |c| c.id()).to_owned())
}

/// Returns an assembly variant.
pub fn assembly_variant_to_json(av: &AssemblyVariant) -> Value {
    json!({
        "uuid": av.uuid().to_string(),
        "name": av.name().to_string(),
        "description": av.description(),
    })
}

/// Returns the position (lower left corner) and size of a bounding box, or
/// `null`.
pub fn bounding_box_to_json(bbox: &BoundingBox) -> Value {
    match bbox.0 {
        Some((p1, p2)) => {
            let size = p2 - p1;
            json!({
                "x": length_to_json(p1.x.min(p2.x)),
                "y": length_to_json(p1.y.min(p2.y)),
                "width": length_to_json(size.x.abs()),
                "height": length_to_json(size.y.abs()),
            })
        }
        None => Value::Null,
    }
}

/// Returns the number of drills and their (distinct) diameters.
pub fn tool_list_to_json(tools: &ToolList) -> Value {
    json!({
        "count": tools.diameters.len(),
        "diameters": length_set_to_json(&tools.diameters.iter().copied().collect()),
    })
}

/// Returns the information about a board of `project`.
pub fn board_to_json(project: &Project, board: &Board) -> Value {
    let settings = board.settings();
    let mut tht_vias = ToolList::default();
    let mut blind_vias = ToolList::default();
    let mut buried_vias = ToolList::default();
    let mut pth_drills = ToolList::default();
    let mut pth_slots = ToolList::default();
    let mut npth_drills = ToolList::default();
    let mut npth_slots = ToolList::default();
    let mut plated_cutouts: usize = 0;
    let mut copper_widths: BTreeSet<Length> = BTreeSet::new();
    let mut add_pad_holes = |pad: &crate::geometry::Pad| {
        for hole in pad.holes().iter() {
            if hole.is_slot() {
                pth_slots.diameters.push(*hole.diameter());
            } else {
                pth_drills.diameters.push(*hole.diameter());
            }
        }
    };
    for segment in board.net_segments().values() {
        for pad in segment.pads().values() {
            add_pad_holes(pad.pad());
        }
        for via in segment.vias().values() {
            let properties = board.via_properties(via, segment.net(), &project.circuit);
            if let Some((start, end)) = properties.drill_layer_span {
                let drill = *properties.drill_diameter;
                if start.is_top() && end.is_bottom() {
                    tht_vias.diameters.push(drill);
                } else if start.is_top() || end.is_bottom() {
                    blind_vias.diameters.push(drill);
                } else {
                    buried_vias.diameters.push(drill);
                }
            }
        }
        for trace in segment.traces().values() {
            copper_widths.insert(*trace.width());
        }
    }
    for device in board.devices().values() {
        if let Ok(pads) = device.pads(&project.library, &project.circuit) {
            for pad in pads {
                add_pad_holes(pad.properties());
            }
        }
        let Some(footprint) = lib_footprint(project, device) else {
            continue;
        };
        for hole in footprint.holes().iter() {
            if hole.is_slot() {
                npth_slots.diameters.push(*hole.diameter());
            } else {
                npth_drills.diameters.push(*hole.diameter());
            }
        }
        plated_cutouts += footprint
            .polygons()
            .iter()
            .filter(|p| p.layer() == Layer::BOARD_PLATED_CUTOUTS)
            .count();
        plated_cutouts += footprint
            .circles()
            .iter()
            .filter(|c| c.layer() == Layer::BOARD_PLATED_CUTOUTS)
            .count();
    }
    for hole in board.holes().values() {
        if hole.path().vertices().len() > 1 {
            npth_slots.diameters.push(*hole.diameter());
        } else {
            npth_drills.diameters.push(*hole.diameter());
        }
    }
    for plane in board.planes().values() {
        copper_widths.insert(*plane.min_width());
    }
    plated_cutouts += board
        .polygons()
        .values()
        .filter(|p| p.layer() == Layer::BOARD_PLATED_CUTOUTS)
        .count();
    let min_copper_width = copper_widths.first().copied();

    json!({
        "uuid": board.uuid().to_string(),
        "name": board.name().to_string(),
        "directory": board.directory_name(),
        "inner_layers": settings.inner_layer_count,
        "pcb_thickness": length_to_json(*settings.pcb_thickness),
        "solder_resist": pcb_color_to_json(settings.solder_resist),
        "silkscreen_top": pcb_color_to_json(settings.silkscreen_color_top()),
        "silkscreen_bottom": pcb_color_to_json(settings.silkscreen_color_bot()),
        "bounding_box": bounding_box_to_json(&board_bounding_box(project, board)),
        "vias_tht": tool_list_to_json(&tht_vias),
        "vias_blind": tool_list_to_json(&blind_vias),
        "vias_buried": tool_list_to_json(&buried_vias),
        "pth_drills": tool_list_to_json(&pth_drills),
        "pth_slots": tool_list_to_json(&pth_slots),
        "npth_drills": tool_list_to_json(&npth_drills),
        "npth_slots": tool_list_to_json(&npth_slots),
        "plated_cutouts": plated_cutouts,
        "min_copper_width": optional_length_to_json(min_copper_width),
    })
}

/// Returns the information about a project and its boards.
pub fn project_to_json(project: &Project) -> Value {
    let metadata = project.metadata();
    let settings = project.settings();
    json!({
        "filename": project.file_name(),
        "uuid": project.uuid().to_string(),
        "name": metadata.name.to_string(),
        "author": metadata.author,
        "version": metadata.version.to_string(),
        "created": metadata.created.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "locales": string_list_to_json(&settings.locale_order),
        "norms": string_list_to_json(&settings.norm_order),
        "variants": project
            .circuit()
            .assembly_variants()
            .iter()
            .map(assembly_variant_to_json)
            .collect::<Vec<_>>(),
        "boards": project
            .boards()
            .iter()
            .map(|b| board_to_json(project, b))
            .collect::<Vec<_>>(),
    })
}

/// Returns the complete JSON file content of a project (upstream
/// `toUtf8()`).
pub fn to_utf8(project: &Project) -> Vec<u8> {
    let root = json!({
        "format": {
            "major": 1, // Only increment (when needed) for new major releases!!!
            "minor": 0, // Increment on every backwards-compatible format addition.
            "type": "librepcb-project",
        },
        "project": project_to_json(project),
    });
    to_indented_json(&root).into_bytes()
}

/// Formats a JSON object or array like `QJsonDocument::toJson(Indented)`
/// (including the trailing newline). Other values are formatted like
/// object members.
pub fn to_indented_json(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    if value.is_object() || value.is_array() {
        out.push('\n');
    }
    out
}

// --- Board helpers ---

/// Upstream `Board::calculateBoundingRect()`: the bounding rectangle of the
/// board outlines (board polygons and footprint polygons/circles), computed
/// on the `QPainterPath` like upstream.
fn board_bounding_box(project: &Project, board: &Board) -> BoundingBox {
    let mut outlines: Vec<Path> = board
        .polygons()
        .values()
        .filter(|p| p.layer() == Layer::BOARD_OUTLINES && !p.path().vertices().is_empty())
        .map(|p| p.path().clone())
        .collect();
    for device in board.devices().values() {
        let Some(footprint) = lib_footprint(project, device) else {
            continue;
        };
        let transform = device.transform();
        for polygon in footprint.polygons().iter() {
            if polygon.layer() == Layer::BOARD_OUTLINES && !polygon.path().vertices().is_empty() {
                outlines.push(transform.map(polygon.path()));
            }
        }
        for circle in footprint.circles().iter() {
            if circle.layer() == Layer::BOARD_OUTLINES {
                outlines.push(
                    transform.map(&Path::circle(circle.diameter()).translated(circle.center())),
                );
            }
        }
    }
    if outlines.is_empty() {
        return BoundingBox(None);
    }
    let rect = painter_path::bounding_rect_px(&outlines);
    // The rectangle is built from coordinates of valid points, so the
    // conversion back cannot overflow.
    let bottom_left = Point::from_px(rect.left(), rect.bottom()).unwrap_or_default();
    let top_right = Point::from_px(rect.right(), rect.top()).unwrap_or_default();
    BoundingBox(Some((bottom_left, top_right)))
}

/// Returns the library footprint of a device.
fn lib_footprint<'a>(project: &'a Project, device: &BoardDevice) -> Option<&'a Footprint> {
    let lib_device = project.library.device(&device.lib_device())?;
    let package = project.library.package(&lib_device.package_uuid())?;
    package.footprints().by_uuid(&device.lib_footprint())
}

// --- Qt compatible JSON formatting ---

fn f64_to_json(value: f64) -> Value {
    // Lengths in millimeters are always finite.
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn write_value(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let _ = write!(out, "{i}");
            } else if let Some(u) = n.as_u64() {
                let _ = write!(out, "{u}");
            } else {
                out.push_str(&format_double(n.as_f64().unwrap_or_default()));
            }
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                push_indent(out, indent + 1);
                write_value(out, item, indent + 1);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            push_indent(out, indent);
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("{\n");
            write_object_content(out, map, indent + 1);
            push_indent(out, indent);
            out.push('}');
        }
    }
}

fn write_object_content(out: &mut String, map: &Map<String, Value>, indent: usize) {
    // `QJsonObject` is sorted by key (`serde_json::Map` might preserve the
    // insertion order if its `preserve_order` feature is enabled).
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (i, (key, value)) in entries.iter().enumerate() {
        push_indent(out, indent);
        write_string(out, key);
        out.push_str(": ");
        write_value(out, value, indent);
        out.push_str(if i + 1 < entries.len() { ",\n" } else { "\n" });
    }
}

fn push_indent(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', 4 * indent));
}

/// Writes a JSON string like Qt: `"` and `\` escaped, control characters
/// as `\b`, `\f`, `\n`, `\r`, `\t` or `\u00xx`, everything else verbatim
/// (same as `serde_json`).
fn write_string(out: &mut String, s: &str) {
    // Serializing a string cannot fail.
    out.push_str(&serde_json::to_string(s).unwrap_or_default());
}

/// Formats a finite double like `QByteArray::number(d, 'g',
/// QLocale::FloatingPointShortest)` (Qt 6 `qdtoAscii()`).
fn format_double(d: f64) -> String {
    if !d.is_finite() {
        return "null".to_owned();
    }
    if d == 0.0 {
        return "0".to_owned(); // Qt never returns "-0".
    }
    // Shortest round-trip digits and exponent, e.g. "1.25e-3".
    let sci = format!("{:e}", d.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let decpt = exponent.parse::<i32>().unwrap_or(0) + 1;
    let length = digits.len() as i32;

    // Qt's `resolveFormat()`: use the shorter representation.
    let mut bias = 2 + 2;
    if length <= decpt && length > 1 {
        bias += 1;
    } else if length == 1 && decpt <= 0 {
        bias -= 1;
    }
    let use_decimal = if decpt <= 0 {
        1 - decpt <= bias
    } else if decpt <= length {
        true
    } else {
        decpt <= length + bias
    };

    let mut result = String::new();
    if d < 0.0 {
        result.push('-');
    }
    if use_decimal {
        if decpt <= 0 {
            result.push_str("0.");
            result.extend(std::iter::repeat_n('0', (-decpt) as usize));
            result.push_str(&digits);
        } else if decpt >= length {
            result.push_str(&digits);
            result.extend(std::iter::repeat_n('0', (decpt - length) as usize));
        } else {
            let (int, frac) = digits.split_at(decpt as usize);
            result.push_str(int);
            result.push('.');
            result.push_str(frac);
        }
    } else {
        let (first, rest) = digits.split_at(1);
        result.push_str(first);
        if !rest.is_empty() {
            result.push('.');
            result.push_str(rest);
        }
        let exp = decpt - 1;
        let _ = write!(
            result,
            "e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_double_like_qt() {
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (1.5, "1.5"),
            (-5.5, "-5.5"),
            (0.1, "0.1"),
            (0.3, "0.3"),
            (100.0, "100"),
            (1234.5678, "1234.5678"),
            (0.001, "0.001"),
            (0.0001, "1e-04"),
            (0.00001, "1e-05"),
            (0.000001, "1e-06"),
            (0.0000015, "1.5e-06"),
            (0.00015, "0.00015"),
            (10000.0, "10000"),
            (100000.0, "1e+05"),
            (1000000.0, "1e+06"),
            (1200000.0, "1200000"),
            (12000000.0, "1.2e+07"),
            (0.2 + 0.1, "0.30000000000000004"),
        ];
        for (value, expected) in cases {
            assert_eq!(format_double(value), expected, "{value}");
        }
    }

    #[test]
    fn indented_json_like_qt() {
        let value = json!({"b": [], "a": {"x": 1, "y": [0.5, "s\"\n"]}, "c": {}});
        assert_eq!(
            to_indented_json(&value),
            "{\n    \"a\": {\n        \"x\": 1,\n        \"y\": [\n            0.5,\n            \
             \"s\\\"\\n\"\n        ]\n    },\n    \"b\": [\n    ],\n    \"c\": {\n    }\n}\n"
        );
    }
}
