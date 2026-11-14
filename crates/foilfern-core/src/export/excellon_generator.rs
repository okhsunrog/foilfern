//! Port of libs/librepcb/core/export/excellongenerator.{h,cpp}.
//!
//! Differences to upstream:
//! - The application version and creation date are passed in
//!   ([`GerberFileInfo`]).
//! - The overloaded `drill()` is split into [`ExcellonGenerator::drill()`]
//!   (single hole) and [`ExcellonGenerator::drill_path()`] (slot).
//! - [`ExcellonGenerator::generate()`] returns the file content instead of
//!   storing it.

use std::collections::BTreeMap;

use super::{ApertureFunction, Error, GerberAttribute, GerberFileInfo, Result};
use crate::fileio::{FilePath, file_utils};
use crate::geometry::NonEmptyPath;
use crate::types::{Angle, Length, Point, PositiveLength};
use crate::utils::toolbox;

/// Plating of the holes in an Excellon file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Plating {
    /// Only plated holes.
    Yes,
    /// Only non-plated holes.
    No,
    /// Plated and non-plated holes in the same file.
    Mixed,
}

/// A tool: diameter, plated, function. The ordering defines the tool
/// numbers (like the `QMultiMap` keys upstream).
type Tool = (Length, bool, ApertureFunction);

/// Generates an Excellon drill file (XNC format, metric).
#[derive(Debug, Clone)]
pub struct ExcellonGenerator {
    plating: Plating,
    file_attributes: Vec<GerberAttribute>,
    use_g85_slots: bool,
    /// Drills per tool, in insertion order.
    drills: BTreeMap<Tool, Vec<NonEmptyPath>>,
}

impl ExcellonGenerator {
    /// Creates a generator for a file with the given metadata, plating and
    /// layer span (1 = top).
    pub fn new(info: &GerberFileInfo, plating: Plating, from_layer: i32, to_layer: i32) -> Self {
        let mut file_attributes = info.file_attributes();
        file_attributes.push(match plating {
            Plating::Yes => {
                GerberAttribute::file_function_plated_through_hole(from_layer, to_layer)
            }
            Plating::No => {
                GerberAttribute::file_function_non_plated_through_hole(from_layer, to_layer)
            }
            Plating::Mixed => GerberAttribute::file_function_mixed_plating(from_layer, to_layer),
        });
        Self {
            plating,
            file_attributes,
            use_g85_slots: false,
            drills: BTreeMap::new(),
        }
    }

    /// Sets whether slots are exported with the G85 command instead of as
    /// routes (default: `false`). G85 only supports straight slots.
    pub fn set_use_g85_slots(&mut self, use_g85_slots: bool) {
        self.use_g85_slots = use_g85_slots;
    }

    /// Adds a round hole.
    pub fn drill(
        &mut self,
        pos: Point,
        dia: PositiveLength,
        plated: bool,
        function: ApertureFunction,
    ) {
        self.drill_path(NonEmptyPath::from_point(pos), dia, plated, function);
    }

    /// Adds a slot (or a round hole if the path has only one vertex).
    pub fn drill_path(
        &mut self,
        path: NonEmptyPath,
        dia: PositiveLength,
        plated: bool,
        function: ApertureFunction,
    ) {
        self.drills
            .entry((*dia, plated, function))
            .or_default()
            .push(path);
    }

    /// Generates the complete file content.
    ///
    /// Fails if G85 slots are enabled and there are curved slots.
    pub fn generate(&self) -> Result<String> {
        let mut out = String::new();

        // Header.
        out.push_str("M48\n"); // Beginning of Part Program Header
        for attribute in &self.file_attributes {
            out.push_str(&attribute.to_excellon_string());
        }
        out.push_str("FMAT,2\n"); // Use Format 2 commands
        out.push_str("METRIC,TZ\n"); // Metric Format, Trailing Zeros Mode

        // Tool list.
        for (number, &(dia, plated, function)) in (1..).zip(self.drills.keys()) {
            let attribute = if self.plating == Plating::Mixed {
                GerberAttribute::aperture_function_mixed_plating_drill(plated, function)
            } else {
                GerberAttribute::aperture_function(function)
            };
            out.push_str(&attribute.to_excellon_string());
            out.push_str(&format!("T{number}C{}\n", dia.to_mm_string()));
        }

        out.push_str("%\n"); // Beginning of Pattern
        out.push_str("G90\n"); // Absolute Mode
        out.push_str("G05\n"); // Drill Mode
        out.push_str("M71\n"); // Metric Measuring Mode

        // Drills. Like upstream's `QMultiMap::values()`, the most recently
        // added drill of each tool comes first.
        for (number, paths) in (1..).zip(self.drills.values()) {
            out.push_str(&format!("T{number}\n")); // Select Tool
            for path in paths.iter().rev() {
                self.print_path(&mut out, path)?;
            }
        }

        // Footer.
        out.push_str("T0\n");
        out.push_str("M30\n"); // End of Program Rewind
        Ok(out)
    }

    /// Generates the file and writes it.
    pub fn save_to_file(&self, filepath: &FilePath) -> Result<()> {
        // The content is plain ASCII: all non-ASCII characters are removed
        // from attribute values (upstream writes Latin-1).
        file_utils::write_file(filepath, self.generate()?.as_bytes())?;
        Ok(())
    }

    fn print_path(&self, out: &mut String, path: &NonEmptyPath) -> Result<()> {
        let vertices = path.vertices();
        if let [single] = vertices {
            out.push_str(&format!("X{}Y{}\n", mm(single.pos.x), mm(single.pos.y)));
        } else if self.use_g85_slots {
            for pair in vertices.windows(2) {
                let (v0, v1) = (pair[0], pair[1]);
                if v0.angle != Angle::DEG0 {
                    return Err(Error::CurvedSlotG85);
                }
                out.push_str(&format!(
                    "X{}Y{}G85X{}Y{}\n",
                    mm(v0.pos.x),
                    mm(v0.pos.y),
                    mm(v1.pos.x),
                    mm(v1.pos.y)
                ));
            }
        } else {
            print_rout(out, path);
        }
        Ok(())
    }
}

fn print_rout(out: &mut String, path: &NonEmptyPath) {
    let first = path.first().pos;
    out.push_str(&format!("G00X{}Y{}\n", mm(first.x), mm(first.y)));
    out.push_str("M15\n"); // Z Axis Route Position
    for pair in path.vertices().windows(2) {
        let (v0, v1) = (pair[0], pair[1]);
        if v0.angle == Angle::DEG0 {
            out.push_str(&format!("G01X{}Y{}\n", mm(v1.pos.x), mm(v1.pos.y)));
        } else if v0.angle.abs() > Angle::DEG180 {
            // Split arc as recommended in the XNC format specification from
            // Ucamco.
            if let Some(center) = toolbox::arc_center(v0.pos, v1.pos, v0.angle) {
                let half_angle = v0.angle / 2;
                let middle = v0.pos.rotated(half_angle, center);
                print_circular_interpolation(out, v0.pos, middle, half_angle);
                print_circular_interpolation(out, middle, v1.pos, v0.angle - half_angle);
            } else {
                // Fallback: Use a single arc segment and hope the Excellon
                // parser can handle it.
                log::error!(
                    "Failed to split arc segment into two pieces for ExcellonGenerator export!"
                );
                print_circular_interpolation(out, v0.pos, v1.pos, v0.angle);
            }
        } else {
            print_circular_interpolation(out, v0.pos, v1.pos, v0.angle);
        }
    }
    out.push_str("M16\n"); // Retract With Clamping
    out.push_str("G05\n"); // Drill Mode
}

fn print_circular_interpolation(out: &mut String, from: Point, to: Point, angle: Angle) {
    let cmd = if angle < Angle::DEG0 { "G02" } else { "G03" };
    let radius = toolbox::arc_radius(from, to, angle).unwrap_or_else(|| {
        log::error!("Failed to calculate arc radius in ExcellonGenerator, will apply clipping.");
        Length::new(1_000_000_000_000) // 1e6 mm
    });
    out.push_str(&format!(
        "{cmd}X{}Y{}A{}\n",
        mm(to.x),
        mm(to.y),
        mm(radius.abs())
    ));
}

fn mm(length: Length) -> String {
    length.to_mm_string()
}
