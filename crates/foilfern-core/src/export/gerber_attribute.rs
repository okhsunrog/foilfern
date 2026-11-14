//! Port of libs/librepcb/core/export/gerberattribute.{h,cpp}.
//!
//! Differences to upstream:
//! - There is no invalid (default constructed) attribute; upstream only
//!   creates one for out-of-range enum values.
//! - Values are truncated to 65535 characters instead of 65535 UTF-16 code
//!   units (see COMPAT.md).

use unicode_normalization::UnicodeNormalization;

use super::Timestamp;
use crate::types::{Angle, Uuid};

/// The kind (scope) of a [`GerberAttribute`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GerberAttributeType {
    /// File attribute (`TF`).
    File,
    /// Aperture attribute (`TA`).
    Aperture,
    /// Object attribute (`TO`).
    Object,
    /// Attribute deletion (`TD`).
    Delete,
}

/// Image polarity of a Gerber file or layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Polarity {
    /// Dark (adds material).
    Positive,
    /// Clear (removes material).
    Negative,
}

/// Side of the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoardSide {
    /// Top side.
    Top,
    /// Bottom side.
    Bottom,
}

/// Position of a copper layer in the stackup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CopperSide {
    /// Top copper layer.
    Top,
    /// Inner copper layer.
    Inner,
    /// Bottom copper layer.
    Bottom,
}

/// Mount type of a component (Gerber X3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MountType {
    /// Through-hole component.
    Tht,
    /// Surface mount component.
    Smt,
    /// Fiducial.
    Fiducial,
    /// Anything else.
    Other,
}

/// Function of an aperture (`.AperFunction` attribute).
///
/// The ordering is the declaration order (it defines the Excellon tool
/// order, like the underlying integer values upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApertureFunction {
    // Available on all layers:
    /// Board outline.
    Profile,
    // Available only on drill/rout layers:
    /// Drill of a via (usually plated).
    ViaDrill,
    /// Drill for component pads (usually plated).
    ComponentDrill,
    /// Drill for press-fit component pads (plated).
    ComponentDrillPressFit,
    /// Drill for mechanical purpose (usually not plated).
    MechanicalDrill,
    // Available only on copper layers:
    /// Copper with electrical function.
    Conductor,
    /// Copper without electrical function.
    NonConductor,
    /// THT pad.
    ComponentPad,
    /// SMT pad, copper-defined.
    SmdPadCopperDefined,
    /// SMT pad, stopmask-defined.
    SmdPadSolderMaskDefined,
    /// BGA pad, copper-defined.
    BgaPadCopperDefined,
    /// BGA pad, stopmask-defined.
    BgaPadSolderMaskDefined,
    /// Edge connector pad.
    ConnectorPad,
    /// Heat sink or thermal pad.
    HeatsinkPad,
    /// Via.
    ViaPad,
    /// Test pad.
    TestPad,
    /// Local fiducial pad.
    FiducialPadLocal,
    /// Global fiducial pad.
    FiducialPadGlobal,
    // Available only on component layers:
    /// Center of component.
    ComponentMain,
    /// Component pin.
    ComponentPin,
    /// Component body outline.
    ComponentOutlineBody,
    /// Component courtyard outline.
    ComponentOutlineCourtyard,
}

impl ApertureFunction {
    /// Returns the values of the `.AperFunction` attribute.
    fn values(self) -> &'static [&'static str] {
        match self {
            Self::Profile => &["Profile"],
            Self::ViaDrill => &["ViaDrill"],
            Self::ComponentDrill => &["ComponentDrill"],
            Self::ComponentDrillPressFit => &["ComponentDrill", "PressFit"],
            Self::MechanicalDrill => &["MechanicalDrill"],
            Self::Conductor => &["Conductor"],
            Self::NonConductor => &["NonConductor"],
            Self::ComponentPad => &["ComponentPad"],
            Self::SmdPadCopperDefined => &["SMDPad", "CuDef"],
            Self::SmdPadSolderMaskDefined => &["SMDPad", "SMDef"],
            Self::BgaPadCopperDefined => &["BGAPad", "CuDef"],
            Self::BgaPadSolderMaskDefined => &["BGAPad", "SMDef"],
            Self::ConnectorPad => &["ConnectorPad"],
            Self::HeatsinkPad => &["HeatsinkPad"],
            Self::ViaPad => &["ViaPad"],
            Self::TestPad => &["TestPad"],
            Self::FiducialPadLocal => &["FiducialPad", "Local"],
            Self::FiducialPadGlobal => &["FiducialPad", "Global"],
            Self::ComponentMain => &["ComponentMain"],
            Self::ComponentPin => &["ComponentPin"],
            Self::ComponentOutlineBody => &["ComponentOutline", "Body"],
            Self::ComponentOutlineCourtyard => &["ComponentOutline", "Courtyard"],
        }
    }
}

fn side_token(side: BoardSide) -> &'static str {
    match side {
        BoardSide::Top => "Top",
        BoardSide::Bottom => "Bot",
    }
}

/// A Gerber X2/X3 attribute.
///
/// Attributes are written as `G04` comments (see
/// [`to_gerber_string()`](Self::to_gerber_string)) since some PCB
/// fabricators fail to parse real X2 attributes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GerberAttribute {
    attribute_type: GerberAttributeType,
    key: String,
    values: Vec<String>,
}

impl GerberAttribute {
    fn new<S: Into<String>>(
        attribute_type: GerberAttributeType,
        key: &str,
        values: impl IntoIterator<Item = S>,
    ) -> Self {
        Self {
            attribute_type,
            key: key.to_owned(),
            values: values.into_iter().map(Into::into).collect(),
        }
    }

    fn file<S: Into<String>>(key: &str, values: impl IntoIterator<Item = S>) -> Self {
        Self::new(GerberAttributeType::File, key, values)
    }

    fn object<S: Into<String>>(key: &str, values: impl IntoIterator<Item = S>) -> Self {
        Self::new(GerberAttributeType::Object, key, values)
    }

    /// Returns the attribute type.
    pub fn attribute_type(&self) -> GerberAttributeType {
        self.attribute_type
    }

    /// Returns the attribute name (e.g. `.FileFunction`).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Returns the raw (unescaped) values.
    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// Returns the attribute as a Gerber `G04` comment line, e.g.
    /// `G04 #@! TF.Part,Single*\n`.
    pub fn to_gerber_string(&self) -> String {
        format!("G04 #@! {}*\n", self.body())
    }

    /// Returns the attribute as an Excellon comment line, e.g.
    /// `; #@! TF.Part,Single\n`.
    pub fn to_excellon_string(&self) -> String {
        format!("; #@! {}\n", self.body())
    }

    fn body(&self) -> String {
        let (letter, strict_ascii) = match self.attribute_type {
            GerberAttributeType::File => ('F', true),
            GerberAttributeType::Aperture => ('A', true),
            // ASCII is not sufficient for component values like μ or Ω, thus
            // unicode is allowed for object attributes. They should appear in
            // Gerber X3 assembly files anyway (not in PCB data files), so only
            // modern X3 readers will need to handle unicode.
            GerberAttributeType::Object => ('O', false),
            GerberAttributeType::Delete => ('D', true),
        };
        let mut s = format!("T{letter}{}", self.key);
        for value in &self.values {
            s.push(',');
            s.push_str(&escape_value(value, strict_ascii));
        }
        s
    }

    /// Deletes the attribute `key`, or all attributes if `key` is empty.
    pub fn unset(key: &str) -> Self {
        Self::new::<String>(GerberAttributeType::Delete, key, [])
    }

    /// `.GenerationSoftware` (the version is omitted if empty).
    pub fn file_generation_software(vendor: &str, application: &str, version: &str) -> Self {
        let mut values = vec![vendor, application];
        if !version.is_empty() {
            values.push(version);
        }
        Self::file(".GenerationSoftware", values)
    }

    /// `.CreationDate`.
    pub fn file_creation_date(date: &Timestamp) -> Self {
        Self::file(".CreationDate", [date.to_iso_string()])
    }

    /// `.ProjectId`.
    pub fn file_project_id(name: &str, uuid: &Uuid, revision: &str) -> Self {
        Self::file(
            ".ProjectId",
            [name.to_owned(), uuid.to_string(), revision.to_owned()],
        )
    }

    /// `.Part,Single`.
    pub fn file_part_single() -> Self {
        Self::file(".Part", ["Single"])
    }

    /// `.SameCoordinates` (the identifier is omitted if empty).
    pub fn file_same_coordinates(identifier: &str) -> Self {
        Self::file(
            ".SameCoordinates",
            Some(identifier).filter(|s| !s.is_empty()),
        )
    }

    /// `.FileFunction,Profile,P|NP`.
    pub fn file_function_profile(plated: bool) -> Self {
        Self::file(
            ".FileFunction",
            ["Profile", if plated { "P" } else { "NP" }],
        )
    }

    /// `.FileFunction,Copper,L<layer>,Top|Inr|Bot`.
    pub fn file_function_copper(layer: i32, side: CopperSide) -> Self {
        let side = match side {
            CopperSide::Top => "Top",
            CopperSide::Inner => "Inr",
            CopperSide::Bottom => "Bot",
        };
        Self::file(
            ".FileFunction",
            ["Copper".to_owned(), format!("L{layer}"), side.to_owned()],
        )
    }

    /// `.FileFunction,Soldermask,Top|Bot`.
    pub fn file_function_solder_mask(side: BoardSide) -> Self {
        Self::file(".FileFunction", ["Soldermask", side_token(side)])
    }

    /// `.FileFunction,Legend,Top|Bot`.
    pub fn file_function_legend(side: BoardSide) -> Self {
        Self::file(".FileFunction", ["Legend", side_token(side)])
    }

    /// `.FileFunction,Paste,Top|Bot`.
    pub fn file_function_paste(side: BoardSide) -> Self {
        Self::file(".FileFunction", ["Paste", side_token(side)])
    }

    /// `.FileFunction,Glue,Top|Bot`.
    pub fn file_function_glue(side: BoardSide) -> Self {
        Self::file(".FileFunction", ["Glue", side_token(side)])
    }

    /// `.FileFunction,Plated,<from>,<to>,PTH`.
    pub fn file_function_plated_through_hole(from_layer: i32, to_layer: i32) -> Self {
        Self::file(
            ".FileFunction",
            [
                "Plated".to_owned(),
                from_layer.to_string(),
                to_layer.to_string(),
                "PTH".to_owned(),
            ],
        )
    }

    /// `.FileFunction,NonPlated,<from>,<to>,NPTH`.
    pub fn file_function_non_plated_through_hole(from_layer: i32, to_layer: i32) -> Self {
        Self::file(
            ".FileFunction",
            [
                "NonPlated".to_owned(),
                from_layer.to_string(),
                to_layer.to_string(),
                "NPTH".to_owned(),
            ],
        )
    }

    /// `.FileFunction,MixedPlating,<from>,<to>`.
    ///
    /// Not an official Gerber attribute (the specs require separate NPTH
    /// and PTH files), but some PCB fabricators require a single drill file
    /// and Ucamco recommends this file function for it.
    pub fn file_function_mixed_plating(from_layer: i32, to_layer: i32) -> Self {
        Self::file(
            ".FileFunction",
            [
                "MixedPlating".to_owned(),
                from_layer.to_string(),
                to_layer.to_string(),
            ],
        )
    }

    /// `.FileFunction,Component,L<layer>,Top|Bot`.
    pub fn file_function_component(layer: i32, side: BoardSide) -> Self {
        Self::file(
            ".FileFunction",
            [
                "Component".to_owned(),
                format!("L{layer}"),
                side_token(side).to_owned(),
            ],
        )
    }

    /// `.FilePolarity,Positive|Negative`.
    pub fn file_polarity(polarity: Polarity) -> Self {
        let value = match polarity {
            Polarity::Positive => "Positive",
            Polarity::Negative => "Negative",
        };
        Self::file(".FilePolarity", [value])
    }

    /// `.MD5`.
    pub fn file_md5(md5: &str) -> Self {
        Self::file(".MD5", [md5])
    }

    /// `.AperFunction`.
    pub fn aperture_function(function: ApertureFunction) -> Self {
        Self::new(
            GerberAttributeType::Aperture,
            ".AperFunction",
            function.values().iter().copied(),
        )
    }

    /// `.AperFunction` prefixed with the plating (`Plated,PTH` or
    /// `NonPlated,NPTH`). Only for mixed-plating Excellon files, see
    /// [`file_function_mixed_plating()`](Self::file_function_mixed_plating).
    pub fn aperture_function_mixed_plating_drill(plated: bool, function: ApertureFunction) -> Self {
        let prefix: [&str; 2] = if plated {
            ["Plated", "PTH"]
        } else {
            ["NonPlated", "NPTH"]
        };
        Self::new(
            GerberAttributeType::Aperture,
            ".AperFunction",
            prefix.into_iter().chain(function.values().iter().copied()),
        )
    }

    /// `.N` (net name).
    pub fn object_net(net: &str) -> Self {
        Self::object(".N", [net])
    }

    /// `.C` (component designator).
    pub fn object_component(component: &str) -> Self {
        Self::object(".C", [component])
    }

    /// `.P` (component pin; the signal is omitted if empty).
    pub fn object_pin(component: &str, pin: &str, signal: &str) -> Self {
        let mut values = vec![component, pin];
        if !signal.is_empty() {
            values.push(signal);
        }
        Self::object(".P", values)
    }

    /// `.CRot` (component rotation in degrees).
    pub fn component_rotation(rotation: Angle) -> Self {
        Self::object(".CRot", [rotation.to_deg_string()])
    }

    /// `.CMfr` (component manufacturer).
    pub fn component_manufacturer(manufacturer: &str) -> Self {
        Self::object(".CMfr", [manufacturer])
    }

    /// `.CMPN` (component manufacturer part number).
    pub fn component_mpn(mpn: &str) -> Self {
        Self::object(".CMPN", [mpn])
    }

    /// `.CVal` (component value).
    pub fn component_value(value: &str) -> Self {
        Self::object(".CVal", [value])
    }

    /// `.CMnt` (component mount type).
    pub fn component_mount_type(mount_type: MountType) -> Self {
        let value = match mount_type {
            MountType::Tht => "TH",
            MountType::Smt => "SMD",
            MountType::Fiducial => "Fiducial",
            MountType::Other => "Other",
        };
        Self::object(".CMnt", [value])
    }

    /// `.CFtp` (component footprint name).
    pub fn component_footprint(footprint: &str) -> Self {
        Self::object(".CFtp", [footprint])
    }
}

/// Maximum length of an attribute value.
const MAX_VALUE_LENGTH: usize = 65535;

/// Escapes an attribute value: line breaks become spaces; with
/// `strict_ascii`, the value is NFKD-normalized and reduced to the
/// characters every reader understands, otherwise the Gerber special
/// characters are escaped as `\uXXXX`.
fn escape_value(value: &str, strict_ascii: bool) -> String {
    let value = value.replace('\r', "").replace('\n', " ");
    let mut ret = if strict_ascii {
        value
            .nfkd()
            .filter(|&c| c.is_ascii_alphanumeric() || "-_+/!?<>\"'(){}.|&@# ;$:=".contains(c))
            .collect()
    } else {
        value
            .replace('\\', "\\u005C")
            .replace('%', "\\u0025")
            .replace('*', "\\u002A")
            .replace(',', "\\u002C")
    };
    if let Some((index, _)) = ret.char_indices().nth(MAX_VALUE_LENGTH) {
        ret.truncate(index);
    }
    ret
}
