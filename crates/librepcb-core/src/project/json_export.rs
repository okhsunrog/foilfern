//! Port of libs/librepcb/core/project/projectjsonexport.{h,cpp}.
//!
//! This is upstream's versioned manufacturing summary, not the model's
//! serde wire representation. Lengths are millimeters, colors use "none",
//! and tool counts include repeated holes while diameters are deduplicated.
//! `serde_json` supplies the JSON value model and escaping. The formatter
//! preserves upstream's four-space layout (including empty containers) and
//! shortest-number notation, which the standard pretty formatter cannot
//! reproduce. See Qt's qjsonwriter.cpp and qlocale_tools.cpp::resolveFormat.

use super::board::{Board, ResolvedDevice};
use super::{Error, Project, ProjectView, Result};
use crate::geometry::PadHole;
use crate::types::{Layer, Length, PcbColor, PositiveLength};
use chrono::SecondsFormat;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Default)]
struct Tools {
    diameters: Vec<Length>,
}
impl Tools {
    fn add(&mut self, diameter: PositiveLength) {
        self.diameters.push(*diameter);
    }
    fn json(&self) -> Value {
        json!({"count": self.diameters.len(), "diameters": self.diameters.iter().copied()
            .collect::<BTreeSet<_>>().into_iter().map(length_json).collect::<Vec<_>>()})
    }
}

/// Exports upstream's `librepcb-project` JSON format, major 1, minor 0.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProjectJsonExport;

impl ProjectJsonExport {
    /// Returns project metadata, ordered assembly variants and board summaries.
    pub fn project(project: &Project) -> Result<Value> {
        let metadata = project.metadata();
        Ok(json!({
            "filename": project.file_name(), "uuid": project.uuid().to_string(),
            "name": metadata.name.as_str(), "author": metadata.author,
            "version": metadata.version.as_str(),
            "created": metadata.created.to_rfc3339_opts(SecondsFormat::Secs, true),
            "locales": project.settings().locale_order, "norms": project.settings().norm_order,
            "variants": project.circuit().assembly_variants().iter().map(|v| json!({
                "uuid": v.uuid().to_string(), "name": v.name().as_str(), "description": v.description(),
            })).collect::<Vec<_>>(),
            "boards": project.boards().iter().map(|board| Self::board(board, project.view())).collect::<Result<Vec<_>>>()?,
        }))
    }

    /// Returns a board manufacturing summary, including automatic via drill
    /// diameters and the physical drill span clipped to enabled layers.
    pub fn board(board: &Board, view: ProjectView<'_>) -> Result<Value> {
        let (mut tht, mut blind, mut buried) =
            (Tools::default(), Tools::default(), Tools::default());
        let (mut pth_drills, mut pth_slots) = (Tools::default(), Tools::default());
        let (mut npth_drills, mut npth_slots) = (Tools::default(), Tools::default());
        let mut plated_cutouts = 0;
        let mut copper_widths = BTreeSet::new();
        for segment in board.net_segments().values() {
            for pad in segment.pads().values() {
                add_pad_holes(pad.pad().holes(), &mut pth_drills, &mut pth_slots);
            }
            for via in segment.vias().values() {
                let start = via.start_layer();
                if start.copper_number() > board.settings().inner_layer_count as usize {
                    continue;
                }
                let end = if via.end_layer().is_bottom() {
                    via.end_layer()
                } else {
                    let number = via
                        .end_layer()
                        .copper_number()
                        .min(board.settings().inner_layer_count as usize);
                    let Some(end) = Layer::inner_copper(number) else {
                        continue;
                    };
                    if start.copper_number() >= number {
                        continue;
                    }
                    end
                };
                let automatic = if let Some(net) = segment.net() {
                    let net = view
                        .circuit
                        .net_signal(net)
                        .ok_or(Error::InexistentNetSignal(net.0))?;
                    view.circuit
                        .net_classes()
                        .get(&net.net_class())
                        .ok_or(Error::InexistentNetClass(net.net_class().0))?
                        .default_via_drill()
                } else {
                    None
                };
                let diameter = via
                    .drill_diameter()
                    .or(automatic)
                    .unwrap_or(board.design_rules().default_via_drill_diameter());
                if start.is_top() && end.is_bottom() {
                    tht.add(diameter);
                } else if start.is_top() || end.is_bottom() {
                    blind.add(diameter);
                } else {
                    buried.add(diameter);
                }
            }
            copper_widths.extend(segment.traces().values().map(|trace| *trace.width()));
        }
        for device in board.devices().values() {
            let footprint = ResolvedDevice::resolve(device, view.library)?.footprint;
            for pad in footprint.pads() {
                add_pad_holes(pad.pad().holes(), &mut pth_drills, &mut pth_slots);
            }
            for hole in footprint.holes() {
                if hole.is_slot() {
                    npth_slots.add(hole.diameter());
                } else {
                    npth_drills.add(hole.diameter());
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
                npth_slots.add(hole.diameter());
            } else {
                npth_drills.add(hole.diameter());
            }
        }
        copper_widths.extend(board.planes().values().map(|plane| *plane.min_width()));
        plated_cutouts += board
            .polygons()
            .values()
            .filter(|p| p.layer() == Layer::BOARD_PLATED_CUTOUTS)
            .count();
        let bounding_box = match board.bounding_rect(view.library)? {
            None => Value::Null,
            Some((a, b)) => {
                // i128 differences avoid overflowing on extreme valid coordinates.
                let width = (i128::from(a.x.to_nm()) - i128::from(b.x.to_nm())).abs();
                let height = (i128::from(a.y.to_nm()) - i128::from(b.y.to_nm())).abs();
                json!({"x": length_json(a.x.min(b.x)), "y": length_json(a.y.min(b.y)),
                    "width": mm_json(width as f64 / 1_000_000.0), "height": mm_json(height as f64 / 1_000_000.0)})
            }
        };
        let settings = board.settings();
        Ok(json!({
            "uuid": board.uuid().to_string(), "name": board.name().as_str(), "directory": board.directory_name(),
            "inner_layers": settings.inner_layer_count, "pcb_thickness": length_json(*settings.pcb_thickness),
            "solder_resist": color_json(settings.solder_resist), "silkscreen_top": color_json(settings.silkscreen_color_top()),
            "silkscreen_bottom": color_json(settings.silkscreen_color_bot()), "bounding_box": bounding_box,
            "vias_tht": tht.json(), "vias_blind": blind.json(), "vias_buried": buried.json(),
            "pth_drills": pth_drills.json(), "pth_slots": pth_slots.json(),
            "npth_drills": npth_drills.json(), "npth_slots": npth_slots.json(),
            "plated_cutouts": plated_cutouts, "min_copper_width": copper_widths.first().copied().map(length_json),
        }))
    }

    /// Serializes the versioned document as UTF-8, with a final newline.
    pub fn to_utf8(project: &Project) -> Result<Vec<u8>> {
        let document = json!({"format": {"major": 1, "minor": 0, "type": "librepcb-project"}, "project": Self::project(project)?});
        let mut output = Vec::new();
        write_json(&document, 0, &mut output)?;
        output.push(b'\n');
        Ok(output)
    }
}

fn add_pad_holes<'a>(
    holes: impl IntoIterator<Item = &'a PadHole>,
    drills: &mut Tools,
    slots: &mut Tools,
) {
    for hole in holes {
        if hole.is_slot() {
            slots.add(hole.diameter());
        } else {
            drills.add(hole.diameter());
        }
    }
}
fn color_json(color: Option<PcbColor>) -> &'static str {
    color.map_or("none", PcbColor::id)
}
fn length_json(length: Length) -> Value {
    mm_json(length.to_mm())
}
fn mm_json(value: f64) -> Value {
    // Lengths come from finite i64/i128 nanometers; all integral millimeter
    // values here fit i64 and QJsonValue stores them as integers too.
    if value.fract() == 0.0 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn write_json(value: &Value, depth: usize, output: &mut Vec<u8>) -> Result<()> {
    let indent = |output: &mut Vec<u8>, depth| output.extend(std::iter::repeat_n(b' ', depth * 4));
    match value {
        Value::Array(values) => {
            output.extend_from_slice(b"[\n");
            for (i, value) in values.iter().enumerate() {
                indent(output, depth + 1);
                write_json(value, depth + 1, output)?;
                if i + 1 < values.len() {
                    output.push(b',');
                }
                output.push(b'\n');
            }
            indent(output, depth);
            output.push(b']');
        }
        Value::Object(values) => {
            output.extend_from_slice(b"{\n");
            // Remain deterministic even if a downstream crate enables
            // serde_json's preserve_order feature through feature unification.
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (i, (key, value)) in entries.into_iter().enumerate() {
                indent(output, depth + 1);
                serde_json::to_writer(&mut *output, key)?;
                output.extend_from_slice(b": ");
                write_json(value, depth + 1, output)?;
                if i + 1 < values.len() {
                    output.push(b',');
                }
                output.push(b'\n');
            }
            indent(output, depth);
            output.push(b'}');
        }
        Value::Number(number) if number.is_f64() => output
            .extend_from_slice(shortest_number(number.as_f64().unwrap_or_default()).as_bytes()),
        _ => serde_json::to_writer(output, value)?,
    }
    Ok(())
}

/// Preserve the upstream file's choice between decimal and exponent form.
/// Rust supplies shortest-round-trip digits; this rule changes only their
/// notation. Exponents always include a sign and at least two digits.
fn shortest_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let text = value.abs().to_string();
    let (mantissa, exponent) = text
        .split_once(['e', 'E'])
        .map_or((text.as_str(), 0), |(m, e)| {
            (m, e.parse::<i32>().unwrap_or_default())
        });
    let mut point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let leading = digits.len() - digits.trim_start_matches('0').len();
    point -= leading as i32;
    let digits = digits[leading..].trim_end_matches('0');
    let length = digits.len() as i32;
    let bias = 4 + i32::from(length <= point && length > 1) - i32::from(length == 1 && point <= 0);
    let decimal = if point <= 0 {
        1 - point <= bias
    } else {
        point <= length + bias
    };
    let mut output = if value < 0.0 {
        "-".to_owned()
    } else {
        String::new()
    };
    if decimal {
        if point <= 0 {
            output.push_str("0.");
            output.extend(std::iter::repeat_n('0', (-point) as usize));
            output.push_str(digits);
        } else if point >= length {
            output.push_str(digits);
            output.extend(std::iter::repeat_n('0', (point - length) as usize));
        } else {
            output.push_str(&digits[..point as usize]);
            output.push('.');
            output.push_str(&digits[point as usize..]);
        }
    } else {
        output.push_str(&digits[..1]);
        if length > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        output.push('e');
        let exponent = point - 1;
        output.push(if exponent < 0 { '-' } else { '+' });
        output.push_str(&format!("{:02}", exponent.abs()));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_shortest_number_notation() {
        for (value, expected) in [
            (0.0, "0"),
            (-0.0, "0"),
            (0.000001, "1e-06"),
            (0.00001, "1e-05"),
            (0.0001, "1e-04"),
            (0.001, "0.001"),
            (0.00012, "0.00012"),
            (-0.000001, "-1e-06"),
            (1234.5, "1234.5"),
            (1.6, "1.6"),
            (10_000.0, "10000"),
            (100_000.0, "1e+05"),
            (1_234_500_000.0, "1234500000"),
            (12_345_000_000.0, "1.2345e+10"),
        ] {
            assert_eq!(shortest_number(value), expected, "{value}");
        }
    }

    #[test]
    fn escaping_empty_containers_and_number_formatting() {
        let value = json!({"a": [], "b": {}, "c": "\"\\\n\u{0001}Ж", "d": 0.000001});
        let mut bytes = Vec::new();
        write_json(&value, 0, &mut bytes).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\n    \"a\": [\n    ],\n    \"b\": {\n    },\n    \"c\": \"\\\"\\\\\\n\\u0001Ж\",\n    \"d\": 1e-06\n}"
        );
    }
}
