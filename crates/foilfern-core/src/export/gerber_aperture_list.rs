//! Port of libs/librepcb/core/export/gerberaperturelist.{h,cpp}.
//!
//! **Attention:** this code is critical for generating Gerber files which
//! are widely compatible with the CAM software used by PCB fabricators. A lot
//! of know-how is contained in it (no regular polygons, no macro variables or
//! arithmetic, no circle rotation parameter, no center lines, ...). When
//! changing anything, read the Gerber specs very carefully and add unit
//! tests for each new requirement.

use std::collections::HashMap;

use super::{ApertureFunction, GerberAttribute, GerberAttributeWriter};
use crate::geometry::{Path, StraightAreaPath};
use crate::types::{Angle, Length, Point, PositiveLength, UnsignedLength};

/// Number of the first aperture (0..9 are reserved by the Gerber specs).
const FIRST_APERTURE_NUMBER: u32 = 10;

/// Placeholder for the aperture number in aperture definitions.
const NUMBER_PLACEHOLDER: &str = "{}";

type Aperture = (Option<ApertureFunction>, String);

/// Generates the aperture definitions of a Gerber file.
///
/// Identical apertures are added only once, i.e. adding a circle of 1 mm
/// several times creates only one aperture. In addition, always the simplest
/// aperture representing the desired image is created, e.g. an obround with
/// width == height becomes a circle (ignoring the rotation), and rotations
/// are normalized so that equal images share one aperture.
#[derive(Debug, Clone, Default)]
pub struct GerberApertureList {
    /// Function and definition of each aperture (index 0 = aperture 10),
    /// with the placeholder `{}` instead of the aperture number.
    apertures: Vec<Aperture>,
    /// Reverse lookup of `apertures`.
    numbers: HashMap<Aperture, u32>,
}

impl GerberApertureList {
    /// Creates an empty aperture list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Generates the aperture definitions (0..n lines), including the
    /// function attributes.
    pub fn generate_string(&self) -> String {
        let mut s = String::new();
        let mut attribute_writer = GerberAttributeWriter::new();
        for (number, (function, definition)) in (FIRST_APERTURE_NUMBER..).zip(&self.apertures) {
            let attributes: Vec<_> = function
                .iter()
                .map(|f| GerberAttribute::aperture_function(*f))
                .collect();
            s.push_str(&attribute_writer.set_attributes(&attributes));
            s.push_str(&definition.replace(NUMBER_PLACEHOLDER, &number.to_string()));
        }

        // Explicitly clear all attributes at the end of the aperture list to
        // avoid propagating attributes to the rest of the Gerber file!
        s.push_str(&attribute_writer.set_attributes(&[]));
        s
    }

    /// Adds a circle aperture and returns its number. According to the
    /// Gerber specs, a diameter of zero is allowed.
    pub fn add_circle(&mut self, dia: UnsignedLength, function: Option<ApertureFunction>) -> u32 {
        self.add_aperture(format!("%ADD{{}}C,{}*%\n", dia.to_mm_string()), function)
    }

    /// Adds an obround aperture (a circle if `w == h`) and returns its
    /// number.
    pub fn add_obround(
        &mut self,
        w: PositiveLength,
        h: PositiveLength,
        rot: Angle,
        function: Option<ApertureFunction>,
    ) -> u32 {
        if w == h {
            // For maximum compatibility, use a circle if width==height.
            self.add_circle(w.into(), function)
        } else if rot % Angle::DEG180 == Angle::DEG0 {
            self.add_aperture(format!("%ADD{{}}O,{}X{}*%\n", mm(*w), mm(*h)), function)
        } else if rot % Angle::DEG90 == Angle::DEG0 {
            self.add_aperture(format!("%ADD{{}}O,{}X{}*%\n", mm(*h), mm(*w)), function)
        } else if w < h {
            // Same as below, but swap width and height and rotate by 90° to
            // simplify calculations and to merge all combinations of
            // parameters leading to the same image.
            self.add_obround(h, w, rot + Angle::DEG90, function)
        } else {
            // Rotation is not a multiple of 90° --> we need an aperture macro.
            // Normalize the rotation to 0..180° to avoid generating multiple
            // different apertures which represent exactly the same image.
            let rot = rot.mapped_to_0_360deg() % Angle::DEG180;
            let start = Point::new(-*w / 2 + *h / 2, Length::ZERO).rotated(rot, Point::ORIGIN);
            let end = Point::new(*w / 2 - *h / 2, Length::ZERO).rotated(rot, Point::ORIGIN);
            let (h, sx, sy, ex, ey) = (mm(*h), mm(start.x), mm(start.y), mm(end.x), mm(end.y));
            // ATTENTION: Don't use the optional rotation parameter in the
            // circles! It causes critical issues with some crappy CAM software!
            let s = format!(
                "%AMROTATEDOBROUND{{}}*1,1,{h},{sx},{sy}*1,1,{h},{ex},{ey}\
                 *20,1,{h},{sx},{sy},{ex},{ey},0*%\n%ADD{{}}ROTATEDOBROUND{{}}*%\n"
            );
            self.add_aperture(s, function)
        }
    }

    /// Adds a rectangular aperture with corner radius `r` and returns its
    /// number.
    pub fn add_rect(
        &mut self,
        w: PositiveLength,
        h: PositiveLength,
        r: UnsignedLength,
        rot: Angle,
        function: Option<ApertureFunction>,
    ) -> u32 {
        // Handle simple cases first.
        let sharp = r == UnsignedLength::ZERO;
        if sharp && (rot % Angle::DEG180 == Angle::DEG0) {
            return self.add_aperture(format!("%ADD{{}}R,{}X{}*%\n", mm(*w), mm(*h)), function);
        } else if sharp && (rot % Angle::DEG90 == Angle::DEG0) {
            return self.add_aperture(format!("%ADD{{}}R,{}X{}*%\n", mm(*h), mm(*w)), function);
        } else if w < h {
            // Swap width and height and rotate by 90° to simplify
            // calculations and to merge all combinations of parameters
            // leading to the same image.
            return self.add_rect(h, w, r, rot + Angle::DEG90, function);
        }

        // Normalize the rotation to 0..90° (w==h) resp. 0..180° (w!=h) to
        // avoid generating multiple different apertures which represent
        // exactly the same image.
        let modulo = if w == h { Angle::DEG90 } else { Angle::DEG180 };
        let unique_rot = rot.mapped_to_0_360deg() % modulo;
        let rot_str = unique_rot.to_deg_string();
        let (w, h, r) = (*w, *h, *r);

        // More complex cases --> we need an aperture macro. But don't use the
        // "Center Line (Code 21)" since some Gerber parsers interpret its
        // rotation parameter the wrong way (see Gerber specs). Use the
        // "Vector Line (Code 20)" instead.
        if sharp {
            let s = format!(
                "%AMROTATEDRECT{{}}*20,1,{},{},0.0,{},0.0,{rot_str}*%\n\
                 %ADD{{}}ROTATEDRECT{{}}*%\n",
                mm(h),
                mm(-w / 2),
                mm(w / 2),
            );
            self.add_aperture(s, function)
        } else if r >= w.min(h) / 2 {
            // The radius is too large for the given size, it's an obround.
            self.add_obround(positive(w), positive(h), rot, function)
        } else {
            // Rounded corners: a macro with two rects and four circles.
            let circles = [
                Point::new(r - (w / 2), (h / 2) - r),
                Point::new((w / 2) - r, (h / 2) - r),
                Point::new((w / 2) - r, r - (h / 2)),
                Point::new(r - (w / 2), r - (h / 2)),
            ];
            let mut s = format!(
                "%AMROUNDEDRECT{{}}*20,1,{},{},0.0,{},0.0,{rot_str}*",
                mm(h),
                mm(r - (w / 2)),
                mm((w / 2) - r),
            );
            s += &format!(
                "20,1,{},{},0.0,{},0.0,{rot_str}*",
                mm(h - (r * 2)),
                mm(-w / 2),
                mm(w / 2),
            );
            for p in circles.map(|p| p.rotated(unique_rot, Point::ORIGIN)) {
                s += &format!("1,1,{},{},{}*", mm(r * 2), mm(p.x), mm(p.y));
            }
            s += "%\n%ADD{}ROUNDEDRECT{}*%\n";
            self.add_aperture(s, function)
        }
    }

    /// Adds an octagon aperture with corner radius `r` and returns its
    /// number.
    pub fn add_octagon(
        &mut self,
        w: PositiveLength,
        h: PositiveLength,
        r: UnsignedLength,
        rot: Angle,
        function: Option<ApertureFunction>,
    ) -> u32 {
        // Note: If w==h, we could theoretically use the "Regular Polygon (P)"
        // aperture. But some CAD/CAM software render such polygons the wrong
        // way (inside vs. outside diameter), so always use an outline macro.

        // Normalize the rotation to 0..45° (w==h) resp. 0..180° (w!=h) to
        // avoid generating multiple different apertures which represent
        // exactly the same image.
        let modulo = if w == h { Angle::DEG45 } else { Angle::DEG180 };
        let unique_rot = rot.mapped_to_0_360deg() % modulo;
        let inner_width = *w - (*r * 2);
        let inner_height = *h - (*r * 2);

        if w < h {
            // Swap width and height and rotate by 90° to simplify
            // calculations and to merge all combinations of parameters
            // leading to the same image.
            self.add_octagon(h, w, r, rot + Angle::DEG90, function)
        } else if r == UnsignedLength::ZERO {
            self.add_outline_macro(
                "ROTATEDOCTAGON",
                &Path::octagon(w, h, r),
                unique_rot,
                function,
            )
        } else if (inner_width <= Length::ZERO) || (inner_height <= Length::ZERO) {
            // The radius is too large for the given size, it's an obround.
            self.add_obround(w, h, rot, function)
        } else {
            // Rounded corners: a macro with the outline and eight circles.
            let without_arcs: Path = Path::octagon(w, h, r)
                .vertices()
                .iter()
                .map(|v| v.pos.into())
                .collect();
            let mut s = String::from("%AMROUNDEDOCTAGON{}*");
            s += &build_outline_macro(without_arcs, unique_rot);
            let inner = Path::octagon(
                positive(inner_width),
                positive(inner_height),
                UnsignedLength::ZERO,
            )
            .rotated(unique_rot, Point::ORIGIN);
            // Skip [0], it equals the last (closing) vertex.
            for v in inner.vertices().iter().skip(1) {
                s += &format!("1,1,{},{},{}*", mm(*r * 2), mm(v.pos.x), mm(v.pos.y));
            }
            s += "%\n%ADD{}ROUNDEDOCTAGON{}*%\n";
            self.add_aperture(s, function)
        }
    }

    /// Adds a custom outline aperture and returns its number.
    pub fn add_outline(
        &mut self,
        path: &StraightAreaPath,
        rot: Angle,
        function: Option<ApertureFunction>,
    ) -> u32 {
        self.add_outline_macro("OUTLINE", path, rot.mapped_to_0_360deg(), function)
    }

    /// Adds the component main aperture (for component layers only) and
    /// returns its number.
    pub fn add_component_main(&mut self) -> u32 {
        // Note: The aperture shape, size and function is defined in the
        // Gerber specs, do not change them!
        self.add_circle(
            UnsignedLength::new(Length::new(300_000)).expect("positive constant"),
            Some(ApertureFunction::ComponentMain),
        )
    }

    /// Adds a component pin aperture (for component layers only) and returns
    /// its number.
    pub fn add_component_pin(&mut self, is_pin1: bool) -> u32 {
        // Note: The aperture shape, size and function is defined in the
        // Gerber specs, do not change them!
        let definition = if is_pin1 {
            "%ADD{}P,0.36X4X0.0*%\n"
        } else {
            "%ADD{}C,0*%\n"
        };
        self.add_aperture(definition.to_owned(), Some(ApertureFunction::ComponentPin))
    }

    /// Adds an outline macro aperture named `name` (characters A..Z only).
    /// The path must have at least 4 vertices after closing it and no arcs.
    fn add_outline_macro(
        &mut self,
        name: &str,
        path: &Path,
        rot: Angle,
        function: Option<ApertureFunction>,
    ) -> u32 {
        let s = format!(
            "%AM{name}{{}}*{}%\n%ADD{{}}{name}{{}}*%\n",
            build_outline_macro(path.clone(), rot)
        );
        self.add_aperture(s, function)
    }

    /// Adds a new aperture or returns the number of an identical existing
    /// one. `aperture` is the full definition (without attributes), with
    /// `{}` as placeholder for the aperture number.
    fn add_aperture(&mut self, aperture: String, function: Option<ApertureFunction>) -> u32 {
        let key = (function, aperture);
        if let Some(&number) = self.numbers.get(&key) {
            return number;
        }
        let count = u32::try_from(self.apertures.len()).expect("less than 2^32 apertures");
        let number = FIRST_APERTURE_NUMBER + count;
        self.apertures.push(key.clone());
        self.numbers.insert(key, number);
        number
    }
}

/// Builds the content of an outline macro primitive. The path must have at
/// least 4 vertices after closing it and must not contain arcs.
fn build_outline_macro(mut path: Path, rot: Angle) -> String {
    path.close();
    debug_assert!(path.vertices().len() >= 4);
    let mut s = format!("4,1,{},", path.vertices().len().saturating_sub(1));
    for v in path.vertices() {
        debug_assert!(v.angle == Angle::DEG0);
        s += &format!("{},{},", mm(v.pos.x), mm(v.pos.y));
    }
    s += &format!("{}*", rot.to_deg_string());
    s
}

fn mm(length: Length) -> String {
    length.to_mm_string()
}

/// Converts a length which the caller checked to be positive.
fn positive(length: Length) -> PositiveLength {
    PositiveLength::new(length).expect("length checked to be positive")
}
