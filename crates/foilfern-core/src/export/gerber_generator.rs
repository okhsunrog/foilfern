//! Port of libs/librepcb/core/export/gerbergenerator.{h,cpp}.
//!
//! Differences to upstream:
//! - The application version and creation date are passed in
//!   ([`GerberFileInfo`]) instead of taken from `Application::getVersion()`.
//! - The long parameter lists of the draw/flash methods are grouped into
//!   [`ObjectAttributes`] and [`ComponentAttributes`].
//! - [`GerberGenerator::generate()`] returns the file content instead of
//!   storing it; [`GerberGenerator::save_to_file()`] generates and writes it.
//! - Invalid paths are ignored with a `log::warn!()` like upstream's
//!   `qWarning()`.

use std::fmt::Write as _;

use md5::{Digest, Md5};

use super::{
    ApertureFunction, BoardSide, CopperSide, GerberApertureList, GerberAttribute,
    GerberAttributeWriter, MountType, Polarity, Result, Timestamp,
};
use crate::fileio::{FilePath, file_utils};
use crate::geometry::{Path, StraightAreaPath, Vertex};
use crate::types::{Angle, Length, Point, PositiveLength, UnsignedLength, Uuid};
use crate::utils::toolbox;

/// Metadata written into the header of Gerber and Excellon files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GerberFileInfo {
    /// Version of the generating application (upstream
    /// `Application::getVersion()`), e.g. `"2.0.0"`.
    pub app_version: String,
    /// Creation date of the file.
    pub creation_date: Timestamp,
    /// Project name.
    pub project_name: String,
    /// Project UUID.
    pub project_uuid: Uuid,
    /// Project revision (version).
    pub project_revision: String,
}

impl GerberFileInfo {
    /// Returns the file attributes common to all Gerber and Excellon files.
    pub(super) fn file_attributes(&self) -> Vec<GerberAttribute> {
        vec![
            GerberAttribute::file_generation_software("LibrePCB", "LibrePCB", &self.app_version),
            GerberAttribute::file_creation_date(&self.creation_date),
            GerberAttribute::file_project_id(
                &self.project_name,
                &self.project_uuid,
                &self.project_revision,
            ),
            GerberAttribute::file_part_single(),
            GerberAttribute::file_same_coordinates(""),
        ]
    }
}

/// Attributes of a drawn or flashed object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectAttributes<'a> {
    /// Aperture function.
    pub function: Option<ApertureFunction>,
    /// Net name (`Some("")` means "no net", `None` omits the attribute).
    pub net: Option<&'a str>,
    /// Component designator (empty to omit).
    pub component: &'a str,
    /// Pin number/name (empty to omit; only exported by the flash methods,
    /// together with a component).
    pub pin: &'a str,
    /// Pin signal (empty to omit; only exported together with a pin).
    pub signal: &'a str,
}

/// Attributes of a component on a Gerber X3 component (assembly) layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentAttributes<'a> {
    /// Component designator.
    pub designator: &'a str,
    /// Component value (empty to omit).
    pub value: &'a str,
    /// Mount type.
    pub mount_type: MountType,
    /// Manufacturer (empty to omit).
    pub manufacturer: &'a str,
    /// Manufacturer part number (empty to omit).
    pub mpn: &'a str,
    /// Footprint name (empty to omit).
    pub footprint: &'a str,
}

/// All attributes which can be set on an object.
#[derive(Default)]
struct CurrentAttributes<'a> {
    aperture_function: Option<ApertureFunction>,
    net: Option<&'a str>,
    component: &'a str,
    pin: &'a str,
    signal: &'a str,
    value: &'a str,
    mount_type: Option<MountType>,
    manufacturer: &'a str,
    mpn: &'a str,
    footprint: &'a str,
    rotation: Option<Angle>,
}

impl<'a> CurrentAttributes<'a> {
    fn of_component(c: &ComponentAttributes<'a>, rotation: Angle) -> Self {
        Self {
            component: c.designator,
            value: c.value,
            mount_type: Some(c.mount_type),
            manufacturer: c.manufacturer,
            mpn: c.mpn,
            footprint: c.footprint,
            rotation: Some(rotation),
            ..Self::default()
        }
    }
}

/// Generates a Gerber X2 file (with X3 component attributes if used).
///
/// Coordinates are written in the format 6.6 (i.e. directly in nanometers)
/// in millimeters, and multi quadrant arc mode is used.
#[derive(Debug, Clone)]
pub struct GerberGenerator {
    file_attributes: Vec<GerberAttribute>,
    content: String,
    attribute_writer: GerberAttributeWriter,
    aperture_list: GerberApertureList,
    current_aperture: Option<u32>,
}

impl GerberGenerator {
    /// Creates a generator for a file with the given metadata.
    pub fn new(info: &GerberFileInfo) -> Self {
        Self {
            file_attributes: info.file_attributes(),
            content: String::new(),
            attribute_writer: GerberAttributeWriter::new(),
            aperture_list: GerberApertureList::new(),
            current_aperture: None,
        }
    }

    /// Declares the file as board outline.
    pub fn set_file_function_outlines(&mut self, plated: bool) {
        self.file_attributes
            .push(GerberAttribute::file_function_profile(plated));
    }

    /// Declares the file as copper layer `layer` (1 = top).
    pub fn set_file_function_copper(&mut self, layer: i32, side: CopperSide, polarity: Polarity) {
        self.file_attributes
            .push(GerberAttribute::file_function_copper(layer, side));
        self.file_attributes
            .push(GerberAttribute::file_polarity(polarity));
    }

    /// Declares the file as solder mask.
    pub fn set_file_function_solder_mask(&mut self, side: BoardSide, polarity: Polarity) {
        self.file_attributes
            .push(GerberAttribute::file_function_solder_mask(side));
        self.file_attributes
            .push(GerberAttribute::file_polarity(polarity));
    }

    /// Declares the file as legend (silkscreen).
    pub fn set_file_function_legend(&mut self, side: BoardSide, polarity: Polarity) {
        self.file_attributes
            .push(GerberAttribute::file_function_legend(side));
        self.file_attributes
            .push(GerberAttribute::file_polarity(polarity));
    }

    /// Declares the file as solder paste.
    pub fn set_file_function_paste(&mut self, side: BoardSide, polarity: Polarity) {
        self.file_attributes
            .push(GerberAttribute::file_function_paste(side));
        self.file_attributes
            .push(GerberAttribute::file_polarity(polarity));
    }

    /// Declares the file as glue.
    pub fn set_file_function_glue(&mut self, side: BoardSide, polarity: Polarity) {
        self.file_attributes
            .push(GerberAttribute::file_function_glue(side));
        self.file_attributes
            .push(GerberAttribute::file_polarity(polarity));
    }

    /// Declares the file as Gerber X3 component (assembly) layer.
    pub fn set_file_function_component(&mut self, layer: i32, side: BoardSide) {
        self.file_attributes
            .push(GerberAttribute::file_function_component(layer, side));
    }

    /// Sets the polarity of the following objects.
    pub fn set_layer_polarity(&mut self, polarity: Polarity) {
        self.content.push_str(match polarity {
            Polarity::Positive => "%LPD*%\n",
            Polarity::Negative => "%LPC*%\n",
        });
    }

    /// Draws a line (pin and signal of `attributes` are not exported).
    pub fn draw_line(
        &mut self,
        start: Point,
        end: Point,
        width: UnsignedLength,
        attributes: &ObjectAttributes<'_>,
    ) {
        let aperture = self.aperture_list.add_circle(width, attributes.function);
        self.set_current_aperture(aperture);
        self.set_current_attributes(&CurrentAttributes {
            net: attributes.net,
            component: attributes.component,
            ..CurrentAttributes::default()
        });
        self.move_to_position(start);
        self.linear_interpolate_to_position(end);
    }

    /// Draws the outline of a path with the given line width (pin and
    /// signal of `attributes` are not exported). Paths with less than two
    /// vertices are ignored.
    pub fn draw_path_outline(
        &mut self,
        path: &Path,
        line_width: UnsignedLength,
        attributes: &ObjectAttributes<'_>,
    ) {
        if path.vertices().len() < 2 {
            log::warn!("Invalid path was ignored in gerber output!");
            return;
        }
        let aperture = self
            .aperture_list
            .add_circle(line_width, attributes.function);
        self.set_current_aperture(aperture);
        self.set_current_attributes(&CurrentAttributes {
            net: attributes.net,
            component: attributes.component,
            ..CurrentAttributes::default()
        });
        self.draw_vertices(path.vertices());
    }

    /// Draws a filled area (region) of a closed path (pin and signal of
    /// `attributes` are not exported). Non-closed paths are ignored.
    pub fn draw_path_area(&mut self, path: &Path, attributes: &ObjectAttributes<'_>) {
        if !path.is_closed() {
            log::warn!("Non-closed path was ignored in gerber output!");
            return;
        }
        // Note: Actually G36/G37 regions do not have an aperture attached.
        // But for compatibility reasons, it's better to still select an
        // aperture as usual. A size of 0 caused issues in the past and the
        // Gerber specs recommend against zero-size apertures, so use 0.01mm
        // (it has no impact on the rendered image anyway).
        let aperture = self.aperture_list.add_circle(
            UnsignedLength::new(Length::new(10_000)).expect("positive constant"),
            attributes.function,
        );
        self.set_current_aperture(aperture);
        self.set_current_attributes(&CurrentAttributes {
            aperture_function: attributes.function,
            net: attributes.net,
            component: attributes.component,
            ..CurrentAttributes::default()
        });
        self.content.push_str("G36*\n");
        self.draw_vertices(path.vertices());
        self.content.push_str("G37*\n");
    }

    /// Draws a component outline on a component layer. Paths with less than
    /// two vertices are ignored.
    pub fn draw_component_outline(
        &mut self,
        path: &Path,
        rot: Angle,
        component: &ComponentAttributes<'_>,
        function: Option<ApertureFunction>,
    ) {
        if path.vertices().len() < 2 {
            log::warn!("Invalid path was ignored in gerber output!");
            return;
        }
        let width = UnsignedLength::new(Length::new(100_000)).expect("positive constant");
        let aperture = self.aperture_list.add_circle(width, function);
        self.set_current_aperture(aperture);
        self.set_current_attributes(&CurrentAttributes::of_component(component, rot));
        self.draw_vertices(path.vertices());
    }

    /// Flashes a circle.
    pub fn flash_circle(
        &mut self,
        pos: Point,
        dia: PositiveLength,
        attributes: &ObjectAttributes<'_>,
    ) {
        let aperture = self
            .aperture_list
            .add_circle(dia.into(), attributes.function);
        self.flash(pos, aperture, attributes);
    }

    /// Flashes a rectangle with corner radius `radius`.
    pub fn flash_rect(
        &mut self,
        pos: Point,
        w: PositiveLength,
        h: PositiveLength,
        radius: UnsignedLength,
        rot: Angle,
        attributes: &ObjectAttributes<'_>,
    ) {
        let aperture = self
            .aperture_list
            .add_rect(w, h, radius, rot, attributes.function);
        self.flash(pos, aperture, attributes);
    }

    /// Flashes an obround.
    pub fn flash_obround(
        &mut self,
        pos: Point,
        w: PositiveLength,
        h: PositiveLength,
        rot: Angle,
        attributes: &ObjectAttributes<'_>,
    ) {
        let aperture = self
            .aperture_list
            .add_obround(w, h, rot, attributes.function);
        self.flash(pos, aperture, attributes);
    }

    /// Flashes an octagon with corner radius `radius`.
    pub fn flash_octagon(
        &mut self,
        pos: Point,
        w: PositiveLength,
        h: PositiveLength,
        radius: UnsignedLength,
        rot: Angle,
        attributes: &ObjectAttributes<'_>,
    ) {
        let aperture = self
            .aperture_list
            .add_octagon(w, h, radius, rot, attributes.function);
        self.flash(pos, aperture, attributes);
    }

    /// Flashes a custom outline.
    pub fn flash_outline(
        &mut self,
        pos: Point,
        path: &StraightAreaPath,
        rot: Angle,
        attributes: &ObjectAttributes<'_>,
    ) {
        let aperture = self
            .aperture_list
            .add_outline(path, rot, attributes.function);
        self.flash(pos, aperture, attributes);
    }

    /// Flashes a component center on a component layer.
    pub fn flash_component(&mut self, pos: Point, rot: Angle, component: &ComponentAttributes<'_>) {
        let aperture = self.aperture_list.add_component_main();
        self.set_current_aperture(aperture);
        self.set_current_attributes(&CurrentAttributes::of_component(component, rot));
        self.flash_at_position(pos);
    }

    /// Flashes a component pin on a component layer.
    pub fn flash_component_pin(
        &mut self,
        pos: Point,
        rot: Angle,
        component: &ComponentAttributes<'_>,
        pin: &str,
        signal: &str,
        is_pin1: bool,
    ) {
        let aperture = self.aperture_list.add_component_pin(is_pin1);
        self.set_current_aperture(aperture);
        self.set_current_attributes(&CurrentAttributes {
            pin,
            signal,
            ..CurrentAttributes::of_component(component, rot)
        });
        self.flash_at_position(pos);
    }

    /// Generates the complete file content.
    pub fn generate(&self) -> String {
        let mut output = String::new();

        // Header.
        output.push_str("G04 --- HEADER BEGIN --- *\n");
        for attribute in &self.file_attributes {
            output.push_str(&attribute.to_gerber_string());
        }
        // Coordinate format specification:
        //  - leading zeros omitted
        //  - absolute coordinates
        //  - coordinate format "6.6" --> allows us to directly use nanometers
        output.push_str("%FSLAX66Y66*%\n");
        // Set unit to millimeters.
        output.push_str("%MOMM*%\n");
        // Start linear interpolation mode.
        output.push_str("G01*\n");
        // Use multi quadrant arc mode (single quadrant mode is buggy in some
        // CAM software and is deprecated in the current Gerber specs).
        // See https://github.com/LibrePCB/LibrePCB/issues/247.
        output.push_str("G75*\n");
        output.push_str("G04 --- HEADER END --- *\n");

        // Aperture list.
        output.push_str("G04 --- APERTURE LIST BEGIN --- *\n");
        output.push_str(&self.aperture_list.generate_string());
        output.push_str("G04 --- APERTURE LIST END --- *\n");

        // Content.
        output.push_str("G04 --- BOARD BEGIN --- *\n");
        output.push_str(&self.content);
        output.push_str("G04 --- BOARD END --- *\n");

        // Footer: MD5 checksum over the content (according to the RS-274C
        // standard, line breaks are not included in the checksum).
        let data: String = output.chars().filter(|&c| c != '\n').collect();
        let mut md5 = String::new();
        for byte in Md5::digest(data.as_bytes()) {
            // Writing to a String cannot fail.
            let _ = write!(md5, "{byte:02x}");
        }
        output.push_str(&GerberAttribute::file_md5(&md5).to_gerber_string());

        // End of file.
        output.push_str("M02*\n");
        output
    }

    /// Generates the file and writes it (UTF-8).
    ///
    /// Usually the content is plain ASCII for maximum compatibility with
    /// legacy readers; unicode is only used for Gerber X3 assembly
    /// attributes.
    pub fn save_to_file(&self, filepath: &FilePath) -> Result<()> {
        file_utils::write_file(filepath, self.generate().as_bytes())?;
        Ok(())
    }

    fn flash(&mut self, pos: Point, aperture: u32, attributes: &ObjectAttributes<'_>) {
        self.set_current_aperture(aperture);
        self.set_current_attributes(&CurrentAttributes {
            net: attributes.net,
            component: attributes.component,
            pin: attributes.pin,
            signal: attributes.signal,
            ..CurrentAttributes::default()
        });
        self.flash_at_position(pos);
    }

    fn set_current_attributes(&mut self, a: &CurrentAttributes<'_>) {
        let mut attributes = Vec::new();
        if let Some(function) = a.aperture_function {
            attributes.push(GerberAttribute::aperture_function(function));
        }
        if let Some(net) = a.net {
            attributes.push(GerberAttribute::object_net(net));
        }
        if !a.component.is_empty() {
            attributes.push(GerberAttribute::object_component(a.component));
            if !a.pin.is_empty() {
                attributes.push(GerberAttribute::object_pin(a.component, a.pin, a.signal));
            }
        }
        if !a.value.is_empty() {
            attributes.push(GerberAttribute::component_value(a.value));
        }
        if let Some(mount_type) = a.mount_type {
            attributes.push(GerberAttribute::component_mount_type(mount_type));
        }
        if !a.manufacturer.is_empty() {
            attributes.push(GerberAttribute::component_manufacturer(a.manufacturer));
        }
        if !a.mpn.is_empty() {
            attributes.push(GerberAttribute::component_mpn(a.mpn));
        }
        if !a.footprint.is_empty() {
            attributes.push(GerberAttribute::component_footprint(a.footprint));
        }
        if let Some(rotation) = a.rotation {
            attributes.push(GerberAttribute::component_rotation(rotation));
        }
        let s = self.attribute_writer.set_attributes(&attributes);
        self.content.push_str(&s);
    }

    fn set_current_aperture(&mut self, number: u32) {
        if self.current_aperture != Some(number) {
            self.content.push_str(&format!("D{number}*\n"));
            self.current_aperture = Some(number);
        }
    }

    /// Moves to the first vertex and interpolates along all segments.
    fn draw_vertices(&mut self, vertices: &[Vertex]) {
        if let Some(first) = vertices.first() {
            self.move_to_position(first.pos);
        }
        for pair in vertices.windows(2) {
            self.interpolate_between(pair[0], pair[1]);
        }
    }

    fn move_to_position(&mut self, pos: Point) {
        self.push_coordinate(pos, "D02");
    }

    fn linear_interpolate_to_position(&mut self, pos: Point) {
        self.push_coordinate(pos, "D01");
    }

    fn flash_at_position(&mut self, pos: Point) {
        self.push_coordinate(pos, "D03");
    }

    fn push_coordinate(&mut self, pos: Point, operation: &str) {
        self.content.push_str(&format!(
            "X{}Y{}{operation}*\n",
            pos.x.to_nm(),
            pos.y.to_nm()
        ));
    }

    fn interpolate_between(&mut self, from: Vertex, to: Vertex) {
        if let Some(center) = toolbox::arc_center(from.pos, to.pos, from.angle) {
            // Arc segment.
            self.content.push_str(if from.angle < Angle::DEG0 {
                "G02*\n"
            } else {
                "G03*\n"
            });
            let diff = center - from.pos;
            self.content.push_str(&format!(
                "X{}Y{}I{}J{}D01*\n",
                to.pos.x.to_nm(),
                to.pos.y.to_nm(),
                diff.x.to_nm(),
                diff.y.to_nm()
            ));
            self.content.push_str("G01*\n");
        } else {
            // Linear segment.
            self.linear_interpolate_to_position(to.pos);
        }
    }
}
