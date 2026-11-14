//! Strict EasyEDA primitives to the existing KiCad intermediate representation.

use kurbo::{BezPath, PathEl, Point};
use serde_json::Value;
use std::collections::BTreeSet;
use svgtypes::{SimplePathSegment, SimplifyingPathParser};

use super::{Error, quote, text};

const UNIT: f64 = 0.254; // EasyEDA's schematic/PCB unit is 10 mil.
const MAX_POINTS: usize = 100_000;
const TOLERANCE: f64 = 0.001; // Curved artwork is flattened to 1 µm in the intermediate.

type Output = (String, String, BTreeSet<String>, BTreeSet<String>);

fn number(value: &str) -> Result<f64, Error> {
    let value: f64 = value
        .parse()
        .map_err(|_| Error::Source(format!("Invalid numeric coordinate {value:?}.")))?;
    if !value.is_finite() || value.abs() > 1e10 {
        return Err(Error::Source(
            "Non-finite or excessive source geometry.".into(),
        ));
    }
    Ok(value)
}

fn field<'a>(fields: &[&'a str], index: usize) -> Result<&'a str, Error> {
    fields
        .get(index)
        .copied()
        .ok_or_else(|| Error::Source(format!("Missing primitive field {index}.")))
}

fn n(fields: &[&str], index: usize) -> Result<f64, Error> {
    number(field(fields, index)?)
}
fn positive(value: f64) -> Result<f64, Error> {
    if value > 0.0 {
        Ok(value)
    } else {
        Err(Error::Source("Primitive size must be positive.".into()))
    }
}

fn origin(data: &Value) -> Result<Point, Error> {
    let coordinate = |key: &str| -> Result<f64, Error> {
        let value = &data["head"][key];
        if let Some(value) = value.as_str() {
            number(value)
        } else if let Some(value) = value.as_f64().filter(|n| n.is_finite()) {
            Ok(value)
        } else {
            Err(Error::Source(format!("Missing source origin {key}.")))
        }
    };
    Ok(Point::new(coordinate("x")?, coordinate("y")?))
}

fn point(x: f64, y: f64, origin: Point, flip: bool) -> Point {
    Point::new(
        (x - origin.x) * UNIT,
        (y - origin.y) * UNIT * if flip { -1.0 } else { 1.0 },
    )
}

fn points(source: &str, origin: Point, flip: bool) -> Result<Vec<Point>, Error> {
    let values: Vec<_> = source
        .split_whitespace()
        .map(number)
        .collect::<Result<_, _>>()?;
    if values.len() < 4 || values.len() % 2 != 0 || values.len() / 2 > MAX_POINTS {
        return Err(Error::Source("Invalid polygon point sequence.".into()));
    }
    Ok(values
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| point(p[0], p[1], origin, flip))
        .collect())
}

fn svg(source: &str, origin: Point, flip: bool) -> Result<Vec<Vec<Point>>, Error> {
    if source.len() > 1024 * 1024 {
        return Err(Error::Source("Source SVG path is too large.".into()));
    }
    let mut path = BezPath::new();
    let mut count = 0;
    for segment in SimplifyingPathParser::from(source) {
        count += 1;
        if count > MAX_POINTS {
            return Err(Error::Source("Too many SVG segments.".into()));
        }
        let p = |x, y| point(x, y, origin, flip);
        match segment.map_err(|e| Error::Source(e.to_string()))? {
            SimplePathSegment::MoveTo { x, y } => path.move_to(p(x, y)),
            SimplePathSegment::LineTo { x, y } => path.line_to(p(x, y)),
            SimplePathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => path.curve_to(p(x1, y1), p(x2, y2), p(x, y)),
            SimplePathSegment::Quadratic { x1, y1, x, y } => path.quad_to(p(x1, y1), p(x, y)),
            SimplePathSegment::ClosePath => path.close_path(),
        }
    }
    let mut paths: Vec<Vec<Point>> = Vec::new();
    let mut too_many = false;
    let mut count = 0;
    kurbo::flatten(path, TOLERANCE, |segment| {
        count += 1;
        if count > MAX_POINTS {
            too_many = true;
            return;
        }
        match segment {
            PathEl::MoveTo(p) => paths.push(vec![p]),
            PathEl::LineTo(p) => {
                if let Some(path) = paths.last_mut() {
                    path.push(p);
                }
            }
            PathEl::ClosePath => {
                if let Some(path) = paths.last_mut()
                    && let Some(first) = path.first().copied()
                {
                    path.push(first);
                }
            }
            _ => {}
        }
    });
    if too_many || paths.is_empty() {
        return Err(Error::Source(
            "Empty or excessively complex SVG geometry.".into(),
        ));
    }
    Ok(paths)
}

fn pts(points: &[Point]) -> String {
    points
        .iter()
        .map(|p| format!("(xy {} {})", p.x, p.y))
        .collect::<Vec<_>>()
        .join(" ")
}

fn layer(value: &str) -> Result<Option<&'static str>, Error> {
    match value {
        "1" => Ok(Some("F.Cu")),
        "2" => Ok(Some("B.Cu")),
        "3" => Ok(Some("F.SilkS")),
        "4" => Ok(Some("B.SilkS")),
        "5" => Ok(Some("F.Paste")),
        "6" => Ok(Some("B.Paste")),
        "7" => Ok(Some("F.Mask")),
        "8" => Ok(Some("B.Mask")),
        "12" | "13" => Ok(Some("Dwgs.User")),
        "15" => Ok(Some("F.Fab")),
        // EasyEDA's body/lead projection artwork is not fabricated copper.
        "99" | "100" | "101" => Ok(None),
        other => Err(Error::Source(format!("Unsupported source layer {other}."))),
    }
}

fn shapes(data: &Value) -> Result<Vec<&str>, Error> {
    let shapes = data["shape"]
        .as_array()
        .ok_or_else(|| Error::Source("Missing source shapes.".into()))?;
    if shapes.len() > MAX_POINTS {
        return Err(Error::Source("Too many source primitives.".into()));
    }
    shapes
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| Error::Source("A primitive is not a string.".into()))
        })
        .collect()
}

pub(super) fn convert(
    raw: &Value,
    name: &str,
    warnings: &mut Vec<String>,
) -> Result<Output, Error> {
    let symbol_data = &raw["dataStr"];
    let footprint_data = &raw["packageDetail"]["dataStr"];
    let symbol_origin = origin(symbol_data)?;
    let footprint_origin = origin(footprint_data)?;
    let parameters = &symbol_data["head"]["c_para"];
    let prefix = text(parameters, "pre")?.trim_end_matches('?');
    let mut symbol = format!("(symbol {} (in_bom yes) (on_board yes)", quote(name)?);
    let description = format!(
        "Imported from JLCEDA/EasyEDA Official Library. https://lceda.cn/ https://easyeda.com/\n{}",
        footprint_data["head"]["license"]
            .as_str()
            .unwrap_or_default()
    );
    for (key, value) in [
        ("Reference", prefix),
        ("Value", name),
        ("Description", description.as_str()),
    ] {
        symbol.push_str(&format!(
            "(property {} {} (at 0 0 0)(effects(font(size 1.27 1.27))))",
            quote(key)?,
            quote(value)?
        ));
    }
    symbol.push_str(&format!("(symbol {}", quote(&format!("{name}_1_1"))?));
    let mut pin_numbers = BTreeSet::new();
    for shape in shapes(symbol_data)? {
        let fields: Vec<_> = shape.split('~').collect();
        match field(&fields, 0)? {
            "P" => {
                let segments: Vec<_> = shape.split("^^").collect();
                if segments.len() < 4 {
                    return Err(Error::Source("Incomplete source pin.".into()));
                }
                let settings: Vec<_> = segments[0].split('~').collect();
                let name_fields: Vec<_> = segments[3].split('~').collect();
                let path_fields: Vec<_> = segments[2].split('~').collect();
                let number = field(&settings, 3)?;
                if number.is_empty() || !pin_numbers.insert(number.to_owned()) {
                    return Err(Error::Source(format!(
                        "Empty or duplicate source pin number {number:?}."
                    )));
                }
                let anchor = point(n(&settings, 4)?, n(&settings, 5)?, symbol_origin, true);
                let paths = svg(field(&path_fields, 0)?, symbol_origin, true)?;
                let [path] = paths.as_slice() else {
                    return Err(Error::Source("Pin path must be continuous.".into()));
                };
                if path.len() != 2
                    || libm::hypot(path[0].x - anchor.x, path[0].y - anchor.y) > 0.001
                {
                    return Err(Error::Source(
                        "Pin path does not start at its declared connection point.".into(),
                    ));
                }
                let delta = path[1] - path[0];
                let length = positive(libm::hypot(delta.x, delta.y))?;
                let rotation = libm::atan2(delta.y, delta.x).to_degrees().rem_euclid(360.0);
                let pin_type = match field(&settings, 2)? {
                    "0" => "unspecified",
                    "1" => "input",
                    "2" => "output",
                    "3" => "bidirectional",
                    "4" => "power_in",
                    other => {
                        return Err(Error::Source(format!(
                            "Unknown electrical pin type {other}."
                        )));
                    }
                };
                symbol.push_str(&format!("(pin {pin_type} line (at {} {} {rotation})(length {length})(name {} (effects(font(size 1.0 1.0))))(number {} (effects(font(size 1.0 1.0)))))",anchor.x,anchor.y,quote(field(&name_fields,4)?)?,quote(number)?));
            }
            "R" => {
                let p = point(n(&fields, 1)?, n(&fields, 2)?, symbol_origin, true);
                let width = positive(n(&fields, 5)?)? * UNIT;
                let height = positive(n(&fields, 6)?)? * UNIT;
                symbol.push_str(&format!("(rectangle(start {} {})(end {} {})(stroke(width {})(type default))(fill(type none)))",p.x,p.y,p.x+width,p.y-height,n(&fields,8)?*UNIT));
            }
            "E" => {
                let p = point(n(&fields, 1)?, n(&fields, 2)?, symbol_origin, true);
                let rx = positive(n(&fields, 3)?)? * UNIT;
                let ry = positive(n(&fields, 4)?)? * UNIT;
                let stroke = n(&fields, 6)? * UNIT;
                if (rx - ry).abs() < 0.000001 {
                    symbol.push_str(&format!("(circle(center {} {})(radius {rx})(stroke(width {stroke})(type default))(fill(type none)))",p.x,p.y));
                } else {
                    let points: Vec<_> = (0..=128)
                        .map(|i| {
                            let a = f64::from(i) * std::f64::consts::TAU / 128.0;
                            Point::new(p.x + rx * libm::cos(a), p.y + ry * libm::sin(a))
                        })
                        .collect();
                    symbol.push_str(&format!(
                        "(polyline(pts {})(stroke(width {stroke})(type default))(fill(type none)))",
                        pts(&points)
                    ));
                }
            }
            "PL" | "PG" => {
                let mut points = points(field(&fields, 1)?, symbol_origin, true)?;
                if fields[0] == "PG" {
                    points.push(points[0]);
                }
                symbol.push_str(&format!(
                    "(polyline(pts {})(stroke(width {})(type default))(fill(type none)))",
                    pts(&points),
                    n(&fields, 3)? * UNIT
                ));
            }
            "A" => {
                for path in svg(field(&fields, 1)?, symbol_origin, true)? {
                    symbol.push_str(&format!(
                        "(polyline(pts {})(stroke(width {})(type default))(fill(type none)))",
                        pts(&path),
                        n(&fields, 3)? * UNIT
                    ));
                }
            }
            "T" if matches!(field(&fields, 1)?, "N" | "P") => {} // Native reference/value fields above.
            other => {
                return Err(Error::Source(format!(
                    "Unsupported schematic primitive {other}."
                )));
            }
        }
    }
    symbol.push_str("))");
    let mut footprint = format!(
        "(footprint {} (layer F.Cu)(descr {})",
        quote(name)?,
        quote(&description)?
    );
    footprint.push_str("(fp_text reference \"REF**\" (at 0 0)(layer F.SilkS)(effects(font(size 1 1)(thickness 0.15))))");
    let mut pad_numbers = BTreeSet::new();
    let mut omitted_projection = 0;
    let mut has_through_hole = false;
    let mut model_references = 0;
    for shape in shapes(footprint_data)? {
        let fields: Vec<_> = shape.split('~').collect();
        match field(&fields, 0)? {
            "PAD" => {
                let shape = field(&fields, 1)?;
                let p = point(n(&fields, 2)?, n(&fields, 3)?, footprint_origin, false);
                let width = positive(n(&fields, 4)?)? * UNIT;
                let height = positive(n(&fields, 5)?)? * UNIT;
                let radius = n(&fields, 9)?;
                let plated = match fields.get(15).copied().unwrap_or("Y") {
                    "Y" | "" => true,
                    "N" => false,
                    other => return Err(Error::Source(format!("Invalid plating flag {other}."))),
                };
                let pad_number = if radius > 0.0 && !plated {
                    ""
                } else {
                    field(&fields, 8)?
                };
                if !pad_number.is_empty() {
                    pad_numbers.insert(pad_number.to_owned());
                }
                has_through_hole |= radius > 0.0 && !pad_number.is_empty();
                if radius < 0.0 {
                    return Err(Error::Source("Negative drill radius.".into()));
                }
                let rotation = -n(&fields, 11)?;
                if rotation.rem_euclid(90.0).abs() > 0.000001 {
                    return Err(Error::Source(
                        "Non-cardinal pad rotation requires source contour mapping.".into(),
                    ));
                }
                let hole_length = fields
                    .get(13)
                    .filter(|s| !s.is_empty())
                    .map(|s| number(s))
                    .transpose()?
                    .unwrap_or(0.0)
                    * UNIT;
                let (kind, layers) = if radius > 0.0 {
                    (
                        if pad_number.is_empty() {
                            "np_thru_hole"
                        } else {
                            "thru_hole"
                        },
                        "\"*.Cu\" \"*.Mask\"",
                    )
                } else {
                    match field(&fields, 6)? {
                        "1" => ("smd", "F.Cu F.Paste F.Mask"),
                        "2" => ("smd", "B.Cu B.Paste B.Mask"),
                        other => {
                            return Err(Error::Source(format!(
                                "Unsupported SMD pad layer {other}."
                            )));
                        }
                    }
                };
                let drill = if radius > 0.0 {
                    let diameter = 2.0 * radius * UNIT;
                    if hole_length > diameter {
                        format!("(drill oval {diameter} {hole_length})")
                    } else {
                        format!("(drill {diameter})")
                    }
                } else {
                    String::new()
                };
                let shape = match shape {
                    "RECT" => "rect",
                    "ELLIPSE" if (width - height).abs() < 0.000001 => "circle",
                    "OVAL" => "oval",
                    other => return Err(Error::Source(format!("Unsupported pad shape {other}."))),
                };
                let mut expansion = String::new();
                for (index, name) in [(17, "solder_paste_margin"), (18, "solder_mask_margin")] {
                    if let Some(value) = fields.get(index).filter(|value| !value.is_empty()) {
                        expansion.push_str(&format!("({name} {})", number(value)? * UNIT));
                    }
                }
                footprint.push_str(&format!("(pad {} {kind} {shape}(at {} {} {rotation})(size {width} {height})(layers {layers}){drill}{expansion})",quote(pad_number)?,p.x,p.y));
            }
            "HOLE" => {
                let p = point(n(&fields, 1)?, n(&fields, 2)?, footprint_origin, false);
                let diameter = positive(n(&fields, 3)?)? * 2.0 * UNIT;
                footprint.push_str(&format!("(pad \"\" np_thru_hole circle(at {} {})(size {diameter} {diameter})(drill {diameter})(layers \"*.Cu\" \"*.Mask\"))",p.x,p.y));
            }
            "TRACK" => {
                if let Some(layer) = layer(field(&fields, 2)?)? {
                    let points = points(field(&fields, 4)?, footprint_origin, false)?;
                    for pair in points.windows(2) {
                        line(
                            &mut footprint,
                            pair[0],
                            pair[1],
                            positive(n(&fields, 1)?)? * UNIT,
                            layer,
                        );
                    }
                } else {
                    omitted_projection += 1;
                }
            }
            "CIRCLE" => {
                if let Some(layer) = layer(field(&fields, 5)?)? {
                    let p = point(n(&fields, 1)?, n(&fields, 2)?, footprint_origin, false);
                    let radius = positive(n(&fields, 3)?)? * UNIT;
                    footprint.push_str(&format!("(fp_circle(center {} {})(end {} {})(stroke(width {})(type default))(fill none)(layer {layer}))",p.x,p.y,p.x+radius,p.y,positive(n(&fields,4)?)?*UNIT));
                } else {
                    omitted_projection += 1;
                }
            }
            "ARC" => {
                if let Some(layer) = layer(field(&fields, 2)?)? {
                    for path in svg(field(&fields, 4)?, footprint_origin, false)? {
                        for pair in path.windows(2) {
                            line(
                                &mut footprint,
                                pair[0],
                                pair[1],
                                positive(n(&fields, 1)?)? * UNIT,
                                layer,
                            );
                        }
                    }
                } else {
                    omitted_projection += 1;
                }
            }
            "SOLIDREGION" => {
                if let Some(layer) = layer(field(&fields, 1)?)? {
                    if matches!(field(&fields, 4)?, "cutout" | "npth") {
                        return Err(Error::Source(
                            "Non-lead solid-region cutout requires a native mechanical mapping."
                                .into(),
                        ));
                    }
                    for path in svg(field(&fields, 3)?, footprint_origin, false)? {
                        footprint.push_str(&format!("(fp_poly(pts {})(stroke(width 0)(type default))(fill solid)(layer {layer}))",pts(&path)));
                    }
                } else {
                    omitted_projection += 1;
                }
            }
            "SVGNODE" => {
                model_references += 1;
            } // STEP placement handled separately.
            other => {
                return Err(Error::Source(format!(
                    "Unsupported footprint primitive {other}."
                )));
            }
        }
    }
    footprint.push_str(if has_through_hole {
        "(attr through_hole))"
    } else {
        "(attr smd))"
    });
    if model_references > 0 {
        warnings.push(format!("Retained {model_references} source 3D references; no STEP asset has been attached by this conversion stage."));
    }
    if omitted_projection > 0 {
        warnings.push(format!("Retained {omitted_projection} source body/lead projection primitives in provenance; they are not fabricated 2D geometry."));
    }
    Ok((symbol, footprint, pin_numbers, pad_numbers))
}

fn line(output: &mut String, a: Point, b: Point, width: f64, layer: &str) {
    output.push_str(&format!(
        "(fp_line(start {} {})(end {} {})(stroke(width {width})(type default))(layer {layer}))",
        a.x, a.y, b.x, b.y
    ));
}
