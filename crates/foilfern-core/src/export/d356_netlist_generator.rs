//! Port of libs/librepcb/core/export/d356netlistgenerator.{h,cpp}.
//!
//! Differences to upstream:
//! - The application version is passed in (upstream
//!   `Application::getVersion()`).
//! - Position, size and rotation of a pad/via are grouped into
//!   [`D356Land`].
//! - Numbers are rounded with integer arithmetic (half away from zero, like
//!   Qt's `QString::arg(double, width, 'f', 0)` for the exact values of
//!   nanometers / 1000 and microdegrees / 10⁶).
//!
//! See <https://www.downstreamtech.com/downloads/IPCD356_Simplified.pdf> and
//! <https://web.pa.msu.edu/hep/atlas/l1calo/hub/hardware/components/circuit_board/ipc_356a_net_list.pdf>.

use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization;

use super::Timestamp;
use crate::types::{Angle, Length, Point, PositiveLength};

/// Position, size and rotation of a pad or via land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct D356Land {
    /// Center position.
    pub position: Point,
    /// Width (X size before rotation).
    pub width: PositiveLength,
    /// Height (Y size before rotation).
    pub height: PositiveLength,
    /// Rotation.
    pub rotation: Angle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationCode {
    Continuation = 27,
    BlindOrBuriedVia = 307,
    ThroughHole = 317,
    SurfaceMount = 327,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SolderMask {
    None = 0,
    PrimarySide = 1,
    SecondarySide = 2,
    BothSides = 3,
}

#[derive(Debug, Clone)]
struct Record {
    code: OperationCode,
    signal_name: Option<String>,
    component_name: String,
    pad_name: String,
    mid_point: bool,
    /// Drill diameter and whether it is plated.
    hole: Option<(PositiveLength, bool)>,
    access_code: Option<i32>,
    position: Point,
    width: Option<PositiveLength>,
    height: Option<PositiveLength>,
    rotation: Option<Angle>,
    solder_mask: Option<SolderMask>,
    start_layer: Option<i32>,
    end_layer: Option<i32>,
}

impl Record {
    fn new(code: OperationCode, position: Point) -> Self {
        Self {
            code,
            signal_name: None,
            component_name: String::new(),
            pad_name: String::new(),
            mid_point: false,
            hole: None,
            access_code: None,
            position,
            width: None,
            height: None,
            rotation: None,
            solder_mask: None,
            start_layer: None,
            end_layer: None,
        }
    }

    fn with_land(mut self, land: &D356Land) -> Self {
        self.width = Some(land.width);
        self.height = Some(land.height);
        self.rotation = Some(land.rotation);
        self
    }
}

/// Maximum length of signal names (including the unique suffix).
const SIGNAL_NAME_LENGTH: usize = 14;

/// Generates an IPC-D-356A netlist.
#[derive(Debug, Clone)]
pub struct D356NetlistGenerator {
    comments: Vec<String>,
    records: Vec<Record>,
}

impl D356NetlistGenerator {
    /// Creates a generator with the given metadata.
    pub fn new(
        app_version: &str,
        project_name: &str,
        project_revision: &str,
        board_name: &str,
        generation_date: &Timestamp,
    ) -> Self {
        let comments = [
            "IPC-D-356A Netlist".to_owned(),
            String::new(),
            format!("Project Name:        {project_name}"),
            format!("Project Version:     {project_revision}"),
            format!("Board Name:          {board_name}"),
            format!("Generation Software: LibrePCB {app_version}"),
            format!("Generation Date:     {}", generation_date.to_iso_string()),
            String::new(),
            "Note that due to limitations of this file format, LibrePCB".to_owned(),
            "applies the following operations during the export:".to_owned(),
            "  - suffix net names with unique numbers within braces".to_owned(),
            "  - truncate long net names (uniqueness guaranteed by suffix)".to_owned(),
            "  - truncate long component names (uniqueness not guaranteed)".to_owned(),
            "  - truncate long pad names (uniqueness not guaranteed)".to_owned(),
            "  - clip drill/pad sizes to 9.999mm".to_owned(),
            String::new(),
        ];
        Self {
            comments: comments.into(),
            records: Vec::new(),
        }
    }

    /// Adds an SMT pad on copper layer `layer` (1 = top).
    pub fn smt_pad(
        &mut self,
        net_name: &str,
        component_name: &str,
        pad_name: &str,
        land: &D356Land,
        layer: i32,
    ) {
        self.records.push(Record {
            signal_name: Some(net_name.to_owned()),
            component_name: checked_component_name(component_name),
            pad_name: checked_pad_name(pad_name),
            access_code: Some(layer),
            solder_mask: Some(if layer == 1 {
                SolderMask::SecondarySide
            } else {
                SolderMask::PrimarySide
            }),
            ..Record::new(OperationCode::SurfaceMount, land.position).with_land(land)
        });
    }

    /// Adds a THT pad.
    pub fn tht_pad(
        &mut self,
        net_name: &str,
        component_name: &str,
        pad_name: &str,
        land: &D356Land,
        drill_diameter: PositiveLength,
    ) {
        self.records.push(Record {
            signal_name: Some(net_name.to_owned()),
            component_name: checked_component_name(component_name),
            pad_name: checked_pad_name(pad_name),
            hole: Some((drill_diameter, true)),
            access_code: Some(0),
            solder_mask: Some(SolderMask::None),
            ..Record::new(OperationCode::ThroughHole, land.position).with_land(land)
        });
    }

    /// Adds a through-hole via.
    pub fn through_via(
        &mut self,
        net_name: &str,
        land: &D356Land,
        drill_diameter: PositiveLength,
        solder_mask_covered: bool,
    ) {
        self.records.push(Record {
            signal_name: Some(net_name.to_owned()),
            component_name: "VIA".to_owned(),
            mid_point: true,
            hole: Some((drill_diameter, true)),
            access_code: Some(0),
            solder_mask: Some(if solder_mask_covered {
                SolderMask::BothSides
            } else {
                SolderMask::None
            }),
            ..Record::new(OperationCode::ThroughHole, land.position).with_land(land)
        });
    }

    /// Adds a blind via from copper layer `start_layer` to `end_layer`
    /// (1 = top).
    pub fn blind_via(
        &mut self,
        net_name: &str,
        land: &D356Land,
        drill_diameter: PositiveLength,
        start_layer: i32,
        end_layer: i32,
        solder_mask_covered: bool,
    ) {
        let is_top = start_layer == 1;
        let access_code = if is_top { start_layer } else { end_layer };
        let mask = if solder_mask_covered {
            SolderMask::BothSides
        } else if is_top {
            SolderMask::SecondarySide
        } else {
            SolderMask::PrimarySide
        };
        self.records.push(Record {
            signal_name: Some(net_name.to_owned()),
            component_name: "VIA".to_owned(),
            mid_point: true,
            hole: Some((drill_diameter, true)),
            access_code: Some(access_code),
            solder_mask: Some(mask),
            start_layer: Some(start_layer),
            end_layer: Some(end_layer),
            ..Record::new(OperationCode::BlindOrBuriedVia, land.position)
        });
        self.records.push(Record {
            component_name: "VIA".to_owned(),
            access_code: Some(access_code),
            ..Record::new(OperationCode::Continuation, land.position).with_land(land)
        });
    }

    /// Adds a buried via from copper layer `start_layer` to `end_layer`.
    pub fn buried_via(
        &mut self,
        net_name: &str,
        position: Point,
        drill_diameter: PositiveLength,
        start_layer: i32,
        end_layer: i32,
    ) {
        self.records.push(Record {
            signal_name: Some(net_name.to_owned()),
            component_name: "VIA".to_owned(),
            mid_point: true,
            hole: Some((drill_diameter, true)),
            solder_mask: Some(SolderMask::BothSides),
            start_layer: Some(start_layer),
            end_layer: Some(end_layer),
            ..Record::new(OperationCode::BlindOrBuriedVia, position)
        });
    }

    /// Generates the file content (plain ASCII, `\n` line endings).
    pub fn generate(&self) -> String {
        let mut lines: Vec<String> = Vec::new();

        // Header, limited to 80 characters (with or without newline?).
        for comment in &self.comments {
            lines.push(left(&clean_string(&format!("C  {comment}")), 79));
        }
        lines.push("P  UNITS CUST 1".to_owned()); // Millimeters / degrees

        // Guarantee unique signal names by adding their index as a suffix.
        let mut signal_names: HashMap<&str, String> = HashMap::new();
        for record in &self.records {
            if let Some(name) = record.signal_name.as_deref()
                && !signal_names.contains_key(name)
            {
                let unique = if name.is_empty() {
                    "N/C".to_owned()
                } else {
                    let nbr = format!("{{{}}}", signal_names.len() + 1);
                    left(&clean_string(name), SIGNAL_NAME_LENGTH - nbr.len()) + &nbr
                };
                signal_names.insert(name, unique);
            }
        }

        // Records.
        for record in &self.records {
            let mut line = format!("{:03}", record.code as i32);
            match record.signal_name.as_deref() {
                Some(name) => {
                    let unique = signal_names.get(name).map_or("", String::as_str);
                    line += &format!("{unique:<SIGNAL_NAME_LENGTH$}");
                }
                None => line += &" ".repeat(SIGNAL_NAME_LENGTH),
            }
            line += "   ";
            line += &format!("{:<6}", left(&clean_string(&record.component_name), 6));
            line += if record.pad_name.is_empty() { " " } else { "-" };
            line += &format!("{:<4}", left(&clean_string(&record.pad_name), 4));
            line += if record.mid_point { "M" } else { " " };
            match record.hole {
                Some((dia, plated)) => {
                    line += &format!(
                        "D{}{}",
                        format_length(*dia, false, 4),
                        if plated { "P" } else { "U" }
                    );
                }
                None => line += "      ",
            }
            match record.access_code {
                Some(code) => line += &format!("A{code:02}"),
                None => line += "   ",
            }
            line += &format!(
                "X{}Y{}",
                format_length(record.position.x, true, 6),
                format_length(record.position.y, true, 6)
            );
            match record.width {
                Some(width) => line += &format!("X{}", format_length(*width, false, 4)),
                None => line += "     ",
            }
            match record.height {
                Some(height) => line += &format!("Y{}", format_length(*height, false, 4)),
                None => line += "     ",
            }
            match record.rotation {
                Some(rotation) => line += &format!("R{:03}", round_degrees(rotation)),
                None => line += "    ",
            }
            line += " ";
            match record.solder_mask {
                Some(mask) => line += &format!("S{}", mask as i32),
                None => line += "  ",
            }
            for layer in [record.start_layer, record.end_layer] {
                match layer {
                    Some(layer) => line += &format!("L{layer:02}"),
                    None => line += "   ",
                }
            }
            lines.push(line.trim().to_owned());
        }

        // Footer, including a final line break.
        lines.push("999\n".to_owned());
        lines.join("\n")
    }
}

/// Returns the first `n` characters of `s`.
fn left(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Removes line breaks and all characters which are not allowed in the
/// file format (after NFKD normalization, so e.g. `ä` becomes `a`).
fn clean_string(s: &str) -> String {
    s.replace('\r', "")
        .replace('\n', " ")
        .nfkd()
        .filter(|&c| c.is_ascii_alphanumeric() || "-_+/!?<>\"'(){}.|&@# ,;$:=~".contains(c))
        .collect()
}

fn checked_component_name(name: &str) -> String {
    let lower = name.to_lowercase();
    if (lower == "via") || (lower == "noref") {
        format!("{name}_")
    } else if !name.is_empty() {
        name.to_owned()
    } else {
        // The spec is not clear about what to export for records not related
        // to a component, but in their examples there are a lot of "NOREF".
        "NOREF".to_owned()
    }
}

fn checked_pad_name(name: &str) -> String {
    if !name.is_empty() {
        name.to_owned()
    } else {
        // The spec is not clear about what to export for records not related
        // to a component, but in their examples there are a lot of "NPAD".
        "NPAD".to_owned()
    }
}

/// Divides by `divisor`, rounding halfway cases away from zero.
fn div_round(value: i64, divisor: i64) -> i64 {
    let half = divisor / 2;
    if value >= 0 {
        value.saturating_add(half) / divisor
    } else {
        value.saturating_sub(half) / divisor
    }
}

/// Formats a length in micrometers with `digits` digits (zero-padded,
/// clipped to all nines if too large), optionally with sign.
fn format_length(value: Length, is_signed: bool, digits: usize) -> String {
    let micrometers = div_round(value.abs().to_nm(), 1000);
    let mut s = format!("{micrometers:0digits$}");
    if s.len() > digits {
        log::warn!("Too large number in IPC-D-356A export clipped!");
        s = "9".repeat(digits);
    }
    if is_signed {
        s.insert(0, if value < Length::ZERO { '-' } else { '+' });
    }
    s
}

/// Returns the rotation in whole degrees within 0..=360.
fn round_degrees(rotation: Angle) -> i64 {
    div_round(
        i64::from(rotation.mapped_to_0_360deg().to_micro_deg()),
        1_000_000,
    )
}
