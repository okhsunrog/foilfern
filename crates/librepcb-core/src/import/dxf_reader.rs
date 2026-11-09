//! Port of libs/librepcb/core/import/dxfreader.{h,cpp}: reads points,
//! lines, arcs, circles and polylines of a DXF file (with the `dxf` crate
//! instead of dxflib).
//!
//! Differences to upstream: an empty file is read as a drawing without
//! entities like upstream, but other files the `dxf` crate rejects are
//! reported as errors even where dxflib silently reads nothing.

use std::io::Cursor;

use dxf::entities::EntityType;
use dxf::enums::Units;
use librepcb_i18n::tr;

use crate::fileio::FilePath;
use crate::geometry::Path;
use crate::types::{Angle, Length, Point, PositiveLength};

/// Errors of the DXF reader.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file cannot be read.
    #[error("{0}")]
    Read(String),
    /// The DXF content is invalid.
    #[error("{0}")]
    Parse(String),
    /// A coordinate is out of range.
    #[error("Value out of range in DXF file: {0}")]
    Range(String),
}

/// A circle of a DXF file (upstream `DxfReader::Circle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DxfCircle {
    /// The center.
    pub position: Point,
    /// The diameter.
    pub diameter: PositiveLength,
}

/// The objects of a DXF file (upstream `DxfReader`).
///
/// Units: the `$INSUNITS` of the file are applied (unspecified: millimeters)
/// and then [`scale_factor`](Self::set_scale_factor).
#[derive(Debug, Clone, PartialEq)]
pub struct DxfReader {
    scale_factor: f64,
    points: Vec<Point>,
    circles: Vec<DxfCircle>,
    polygons: Vec<Path>,
}

impl Default for DxfReader {
    fn default() -> Self {
        Self::new()
    }
}

impl DxfReader {
    /// A reader with scale factor 1.
    pub fn new() -> Self {
        Self {
            scale_factor: 1.0,
            points: Vec::new(),
            circles: Vec::new(),
            polygons: Vec::new(),
        }
    }

    /// Sets the factor all coordinates are multiplied with.
    pub fn set_scale_factor(&mut self, factor: f64) {
        self.scale_factor = factor;
    }

    /// The points.
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// The circles.
    pub fn circles(&self) -> &[DxfCircle] {
        &self.circles
    }

    /// The lines, arcs and polylines as paths.
    pub fn polygons(&self) -> &[Path] {
        &self.polygons
    }

    /// Reads a DXF file (upstream `parse()`).
    pub fn parse_file(&mut self, file: &FilePath) -> Result<(), Error> {
        let content = std::fs::read(file.as_path()).map_err(|e| {
            Error::Read(format!(
                "{} {}",
                tr!("DxfReader", "File does not exist or is not readable."),
                e
            ))
        })?;
        self.parse(&content).map_err(|e| match e {
            Error::Parse(msg) => Error::Parse(tr!(
                "DxfReader",
                "Failed to read DXF file \"{0}\": {1}",
                file.to_native(),
                msg
            )),
            other => other,
        })
    }

    /// Reads DXF content.
    pub fn parse(&mut self, content: &[u8]) -> Result<(), Error> {
        if content.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(());
        }
        let drawing = dxf::Drawing::load(&mut Cursor::new(content))
            .map_err(|e| Error::Parse(e.to_string()))?;
        let scale = units_to_mm(drawing.header.default_drawing_units) * self.scale_factor;
        let length = |v: f64| -> Result<Length, Error> {
            Length::from_mm(v * scale).map_err(|e| Error::Range(e.to_string()))
        };
        let point =
            |x: f64, y: f64| -> Result<Point, Error> { Ok(Point::new(length(x)?, length(y)?)) };
        let angle = |deg: f64| -> Result<Angle, Error> {
            Angle::from_deg(deg).map_err(|e| Error::Range(e.to_string()))
        };
        // Round to 0.001° to avoid odd numbers like 179.999999°.
        let bulge_to_angle = |bulge: f64| -> Result<Angle, Error> {
            Ok(Angle::from_rad(libm::atan(bulge) * 4.0)
                .map_err(|e| Error::Range(e.to_string()))?
                .rounded(Angle::new(1000)))
        };
        for entity in drawing.entities() {
            match &entity.specific {
                EntityType::ModelPoint(p) => {
                    self.points.push(point(p.location.x, p.location.y)?);
                }
                EntityType::Line(l) => {
                    self.polygons.push(Path::line(
                        point(l.p1.x, l.p1.y)?,
                        point(l.p2.x, l.p2.y)?,
                        Angle::DEG0,
                    ));
                }
                EntityType::Arc(a) => {
                    let center = point(a.center.x, a.center.y)?;
                    let radius = length(a.radius)?;
                    let angle1 = angle(a.start_angle)?;
                    let angle2 = angle(a.end_angle)?;
                    let p1 =
                        center + Point::new(radius, Length::ZERO).rotated(angle1, Point::ORIGIN);
                    let p2 =
                        center + Point::new(radius, Length::ZERO).rotated(angle2, Point::ORIGIN);
                    let mut delta = angle2 - angle1;
                    if delta < Angle::DEG0 {
                        delta = delta.inverted();
                    }
                    self.polygons.push(Path::line(p1, p2, delta));
                }
                EntityType::Circle(c) => {
                    let diameter = length(c.radius * 2.0)?;
                    match PositiveLength::new(diameter) {
                        Ok(diameter) => self.circles.push(DxfCircle {
                            position: point(c.center.x, c.center.y)?,
                            diameter,
                        }),
                        Err(_) => log::warn!(
                            "Circle in DXF file ignored due to invalid radius: {}",
                            c.radius
                        ),
                    }
                }
                EntityType::Ellipse(_) => {
                    log::warn!("Ellipse in DXF file ignored since it is not supported yet.");
                }
                EntityType::LwPolyline(p) => {
                    let mut path = Path::default();
                    for v in &p.vertices {
                        path.add_vertex(point(v.x, v.y)?, bulge_to_angle(v.bulge)?);
                    }
                    self.add_polyline(path, p.is_closed());
                }
                EntityType::Polyline(p) => {
                    let mut path = Path::default();
                    for v in p.vertices() {
                        path.add_vertex(
                            point(v.location.x, v.location.y)?,
                            bulge_to_angle(v.bulge)?,
                        );
                    }
                    self.add_polyline(path, p.is_closed());
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Upstream `endSequence()`.
    fn add_polyline(&mut self, mut path: Path, closed: bool) {
        if path.vertices().len() >= 2 {
            if closed && path.vertices().len() >= 3 {
                path.close();
            }
            self.polygons.push(path);
        }
    }
}

/// Factor to millimeters of the `$INSUNITS` (upstream
/// `setVariableInt()`; unknown units are millimeters).
fn units_to_mm(units: Units) -> f64 {
    match units {
        Units::Inches => 25.4,
        Units::Feet => 304.8,
        Units::Centimeters => 10.0,
        Units::Meters => 1000.0,
        Units::Microinches => 2.54e-5,
        Units::Mils => 0.0254,
        Units::Yards => 914.4,
        Units::Angstroms => 1.0e-7,
        Units::Nanometers => 1.0e-6,
        Units::Microns => 1.0e-3,
        Units::Decimeters => 100.0,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Vertex;

    fn parse(reader: &mut DxfReader, content: &str) {
        reader.parse(content.as_bytes()).expect("valid DXF");
    }

    fn pt(x: i64, y: i64) -> Point {
        Point::new(Length::new(x), Length::new(y))
    }

    fn header(units: u32) -> String {
        format!("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n{units}\n0\nENDSEC\n")
    }

    fn entities(content: &str) -> String {
        format!("0\nSECTION\n2\nENTITIES\n{content}0\nENDSEC\n0\nEOF\n")
    }

    fn counts(r: &DxfReader) -> (usize, usize, usize) {
        (r.points().len(), r.polygons().len(), r.circles().len())
    }

    #[test]
    fn inexistent_file() {
        let fp = FilePath::new(std::env::temp_dir().join("librepcb-rs-inexistent.dxf"))
            .expect("absolute path");
        assert!(matches!(
            DxfReader::new().parse_file(&fp),
            Err(Error::Read(_))
        ));
    }

    #[test]
    fn empty_file() {
        let mut r = DxfReader::new();
        parse(&mut r, "");
        assert_eq!(counts(&r), (0, 0, 0));
    }

    #[test]
    fn point_no_unit_is_millimeters() {
        let mut r = DxfReader::new();
        parse(&mut r, &entities("0\nPOINT\n10\n-4.0\n20\n-5.0\n"));
        assert_eq!(counts(&r), (1, 0, 0));
        assert_eq!(r.points()[0], pt(-4_000_000, -5_000_000));
    }

    #[test]
    fn point_unspecified_unit_is_millimeters() {
        let mut r = DxfReader::new();
        let content = format!(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n0\n0\nENDSEC\n{}",
            entities("0\nPOINT\n10\n-4.0\n20\n-5.0\n")
        );
        parse(&mut r, &content);
        assert_eq!(counts(&r), (1, 0, 0));
        assert_eq!(r.points()[0], pt(-4_000_000, -5_000_000));
    }

    #[test]
    fn point_millimeters() {
        let mut r = DxfReader::new();
        r.set_scale_factor(2.0);
        let content = header(4) + &entities("0\nPOINT\n10\n-4.0\n20\n-5.0\n");
        parse(&mut r, &content);
        assert_eq!(counts(&r), (1, 0, 0));
        assert_eq!(r.points()[0], pt(-8_000_000, -10_000_000));
    }

    #[test]
    fn point_inches() {
        let mut r = DxfReader::new();
        let content = header(1) + &entities("0\nPOINT\n10\n-4.0\n20\n-5.0\n");
        parse(&mut r, &content);
        assert_eq!(counts(&r), (1, 0, 0));
        assert_eq!(r.points()[0], pt(-101_600_000, -127_000_000));
    }

    #[test]
    fn circle() {
        let mut r = DxfReader::new();
        r.set_scale_factor(2.0);
        let content = header(13) + &entities("0\nCIRCLE\n10\n4.0\n20\n5.0\n40\n8.0\n");
        parse(&mut r, &content);
        assert_eq!(counts(&r), (0, 0, 1));
        assert_eq!(
            r.circles()[0],
            DxfCircle {
                position: pt(8000, 10000),
                diameter: PositiveLength::new(Length::new(32000)).unwrap(),
            }
        );
    }

    #[test]
    fn line() {
        let mut r = DxfReader::new();
        r.set_scale_factor(2.0);
        let content = header(13) + &entities("0\nLINE\n10\n4.0\n20\n5.0\n11\n8.0\n21\n10.0\n");
        parse(&mut r, &content);
        assert_eq!(counts(&r), (0, 1, 0));
        let expected = Path::new(vec![
            Vertex::new(pt(8000, 10000), Angle::DEG0),
            Vertex::new(pt(16000, 20000), Angle::DEG0),
        ]);
        assert_eq!(r.polygons()[0], expected);
    }

    fn arc(scale: f64, start: &str, end: &str) -> Path {
        let mut r = DxfReader::new();
        r.set_scale_factor(scale);
        let content = header(13)
            + &entities(&format!(
                "0\nARC\n10\n4.0\n20\n5.0\n40\n2.0\n50\n{start}\n51\n{end}\n"
            ));
        parse(&mut r, &content);
        assert_eq!(counts(&r), (0, 1, 0));
        r.polygons()[0].clone()
    }

    #[test]
    fn arcs() {
        assert_eq!(
            arc(2.0, "90.0", "180.0"),
            Path::new(vec![
                Vertex::new(pt(8000, 14000), Angle::DEG90),
                Vertex::new(pt(4000, 10000), Angle::DEG0),
            ])
        );
        assert_eq!(
            arc(1.0, "180.0", "90.0"),
            Path::new(vec![
                Vertex::new(pt(2000, 5000), Angle::DEG270),
                Vertex::new(pt(4000, 7000), Angle::DEG0),
            ])
        );
        assert_eq!(
            arc(1.0, "-90.0", "90.0"),
            Path::new(vec![
                Vertex::new(pt(4000, 3000), Angle::DEG180),
                Vertex::new(pt(4000, 7000), Angle::DEG0),
            ])
        );
        assert_eq!(
            arc(1.0, "90.0", "-90.0"),
            Path::new(vec![
                Vertex::new(pt(4000, 7000), Angle::DEG180),
                Vertex::new(pt(4000, 3000), Angle::DEG0),
            ])
        );
    }

    #[test]
    fn lw_polyline_bulge_ccw() {
        let mut r = DxfReader::new();
        r.set_scale_factor(2.0);
        let content = header(13)
            + &entities(
                "0\nLWPOLYLINE\n90\n2\n70\n0\n10\n4.0\n20\n5.0\n42\n1.0\n10\n6.0\n20\n5.0\n",
            );
        parse(&mut r, &content);
        assert_eq!(counts(&r), (0, 1, 0));
        assert_eq!(
            r.polygons()[0],
            Path::new(vec![
                Vertex::new(pt(8000, 10000), Angle::DEG180),
                Vertex::new(pt(12000, 10000), Angle::DEG0),
            ])
        );
    }

    #[test]
    fn polyline_r12() {
        let mut r = DxfReader::new();
        let content = header(13)
            + &entities(
                "0\nPOLYLINE\n70\n0\n0\nVERTEX\n10\n4.0\n20\n5.0\n0\nVERTEX\n10\n4.0\n20\n7.0\n\
                 42\n1.0\n0\nVERTEX\n10\n6.0\n20\n7.0\n0\nVERTEX\n10\n6.0\n20\n5.0\n0\nSEQEND\n",
            );
        parse(&mut r, &content);
        assert_eq!(counts(&r), (0, 1, 0));
        assert_eq!(
            r.polygons()[0],
            Path::new(vec![
                Vertex::new(pt(4000, 5000), Angle::DEG0),
                Vertex::new(pt(4000, 7000), Angle::DEG180),
                Vertex::new(pt(6000, 7000), Angle::DEG0),
                Vertex::new(pt(6000, 5000), Angle::DEG0),
            ])
        );
    }

    #[test]
    fn lw_polyline() {
        let mut r = DxfReader::new();
        let content = header(13)
            + &entities(
                "0\nLWPOLYLINE\n90\n4\n70\n0\n10\n4.0\n20\n5.0\n10\n4.0\n20\n7.0\n42\n1.0\n\
                 10\n6.0\n20\n7.0\n10\n6.0\n20\n5.0\n",
            );
        parse(&mut r, &content);
        assert_eq!(counts(&r), (0, 1, 0));
        assert_eq!(
            r.polygons()[0],
            Path::new(vec![
                Vertex::new(pt(4000, 5000), Angle::DEG0),
                Vertex::new(pt(4000, 7000), Angle::DEG180),
                Vertex::new(pt(6000, 7000), Angle::DEG0),
                Vertex::new(pt(6000, 5000), Angle::DEG0),
            ])
        );
    }

    #[test]
    fn lw_polyline_bulge_cw() {
        let mut r = DxfReader::new();
        let content = header(13)
            + &entities(
                "0\nLWPOLYLINE\n90\n2\n70\n0\n10\n4.0\n20\n5.0\n42\n-1.0\n10\n6.0\n20\n5.0\n",
            );
        parse(&mut r, &content);
        assert_eq!(
            r.polygons()[0],
            Path::new(vec![
                Vertex::new(pt(4000, 5000), -Angle::DEG180),
                Vertex::new(pt(6000, 5000), Angle::DEG0),
            ])
        );
    }

    #[test]
    fn lw_polyline_closed() {
        let mut r = DxfReader::new();
        let content = header(13)
            + &entities(
                "0\nLWPOLYLINE\n90\n3\n70\n1\n10\n4.0\n20\n5.0\n10\n4.0\n20\n7.0\n10\n6.0\n20\n7.0\n",
            );
        parse(&mut r, &content);
        assert_eq!(
            r.polygons()[0],
            Path::new(vec![
                Vertex::new(pt(4000, 5000), Angle::DEG0),
                Vertex::new(pt(4000, 7000), Angle::DEG0),
                Vertex::new(pt(6000, 7000), Angle::DEG0),
                Vertex::new(pt(4000, 5000), Angle::DEG0),
            ])
        );
    }
}
