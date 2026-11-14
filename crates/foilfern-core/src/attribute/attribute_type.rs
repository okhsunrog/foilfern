//! Port of libs/librepcb/core/attribute/{attributetype,attributeunit,
//! attrtype*}.{h,cpp}.
//!
//! The upstream class hierarchy with one singleton per type is replaced by
//! the enum [`AttributeType`] and one static table of [`AttributeUnit`]s per
//! type.

use std::fmt;
use std::str::FromStr;

use foilfern_i18n::translate;

use super::Error;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// A unit of an [`AttributeType`], e.g. "millivolt".
///
/// All units available in LibrePCB are provided by
/// [`AttributeType::available_units()`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AttributeUnit {
    name: &'static str,
    symbol: &'static str,
    user_input_suffixes: &'static [&'static str],
}

impl AttributeUnit {
    /// Creates a unit.
    ///
    /// - `name`: identifier used in files, e.g. `"millivolt"`.
    /// - `symbol`: symbol appended to values, e.g. `"mV"`.
    /// - `user_input_suffixes`: suffixes recognized in user input, e.g.
    ///   `["m", "mv", "mV"]`.
    pub const fn new(
        name: &'static str,
        symbol: &'static str,
        user_input_suffixes: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            symbol,
            user_input_suffixes,
        }
    }

    /// Returns the identifier used in files, e.g. `"millivolt"`.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the symbol, e.g. `"mV"` (upstream `getSymbolTr()`, although
    /// it is not translated).
    pub fn symbol(&self) -> &'static str {
        self.symbol
    }

    /// Returns the suffixes recognized in user input, e.g. `["k"]`.
    pub fn user_input_suffixes(&self) -> &'static [&'static str] {
        self.user_input_suffixes
    }
}

impl ToSExpression for AttributeUnit {
    /// Serializes as token of the unit name.
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.name)
    }
}

/// Type of an attribute value (upstream `AttributeType::Type_t` together
/// with the `AttrType*` singleton classes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttributeType {
    /// Arbitrary text, no units.
    String,
    /// Resistance (Ω).
    Resistance,
    /// Capacitance (F).
    Capacitance,
    /// Inductance (H).
    Inductance,
    /// Voltage (V).
    Voltage,
    /// Current (A).
    Current,
    /// Power (W).
    Power,
    /// Frequency (Hz).
    Frequency,
}

/// Unit table of a type: all units in ascending order and the index of the
/// default unit.
struct Units {
    all: &'static [AttributeUnit],
    default: usize,
}

/// Shorthand for the unit tables below.
#[allow(non_snake_case)]
const fn U(
    name: &'static str,
    symbol: &'static str,
    suffixes: &'static [&'static str],
) -> AttributeUnit {
    AttributeUnit::new(name, symbol, suffixes)
}

static RESISTANCE: [AttributeUnit; 5] = [
    U("microohm", "μΩ", &["u"]),
    U("milliohm", "mΩ", &["m"]),
    U("ohm", "Ω", &["r", "R"]),
    U("kiloohm", "kΩ", &["k"]),
    U("megaohm", "MΩ", &["M", "meg"]),
];

static CAPACITANCE: [AttributeUnit; 5] = [
    U("picofarad", "pF", &["p", "pf", "pF"]),
    U("nanofarad", "nF", &["n", "nf", "nF"]),
    U("microfarad", "μF", &["u", "uf", "uF"]),
    U("millifarad", "mF", &["m", "mf", "mF"]),
    U("farad", "F", &["f", "F"]),
];

static INDUCTANCE: [AttributeUnit; 4] = [
    U("nanohenry", "nH", &["n", "nh", "nH"]),
    U("microhenry", "μH", &["u", "uh", "uH"]),
    U("millihenry", "mH", &["m", "mh", "mH"]),
    U("henry", "H", &["h", "H"]),
];

static VOLTAGE: [AttributeUnit; 6] = [
    U("nanovolt", "nV", &["n", "nv", "nV"]),
    U("microvolt", "μV", &["u", "uv", "uV"]),
    U("millivolt", "mV", &["m", "mv", "mV"]),
    U("volt", "V", &["v", "V"]),
    U("kilovolt", "kV", &["k", "kv", "kV"]),
    U("megavolt", "MV", &["M", "meg", "MV"]),
];

static CURRENT: [AttributeUnit; 7] = [
    U("picoampere", "pA", &["p", "pa", "pA"]),
    U("nanoampere", "nA", &["n", "na", "nA"]),
    U("microampere", "μA", &["u", "ua", "uA"]),
    U("milliampere", "mA", &["m", "ma", "mA"]),
    U("ampere", "A", &["a", "A"]),
    U("kiloampere", "kA", &["k", "ka", "kA"]),
    U("megaampere", "MA", &["M", "meg", "MA"]),
];

static POWER: [AttributeUnit; 7] = [
    U("nanowatt", "nW", &["n", "nw", "nW"]),
    U("microwatt", "μW", &["u", "uw", "uW"]),
    U("milliwatt", "mW", &["m", "mw", "mW"]),
    U("watt", "W", &["w", "W"]),
    U("kilowatt", "kW", &["k", "kw", "kW"]),
    U("megawatt", "MW", &["M", "meg", "MW"]),
    U("gigawatt", "GW", &["g", "G", "gw", "GW"]),
];

static FREQUENCY: [AttributeUnit; 6] = [
    U("microhertz", "μHz", &["u", "uhz", "uHz"]),
    U("millihertz", "mHz", &["m", "mhz", "mHz"]),
    U("hertz", "Hz", &["hz", "Hz"]),
    U("kilohertz", "kHz", &["k", "khz", "kHz"]),
    U("megahertz", "MHz", &["M", "meg", "MHz"]),
    U("gigahertz", "GHz", &["g", "G", "ghz", "GHz"]),
];

impl AttributeType {
    /// All types in upstream order.
    pub const ALL: [Self; 8] = [
        Self::String,
        Self::Resistance,
        Self::Capacitance,
        Self::Inductance,
        Self::Voltage,
        Self::Current,
        Self::Power,
        Self::Frequency,
    ];

    /// Returns the identifier used in files, e.g. `"voltage"`.
    pub fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Resistance => "resistance",
            Self::Capacitance => "capacitance",
            Self::Inductance => "inductance",
            Self::Voltage => "voltage",
            Self::Current => "current",
            Self::Power => "power",
            Self::Frequency => "frequency",
        }
    }

    /// Returns the translated name, e.g. "Voltage".
    pub fn name_tr(self) -> String {
        // Contexts as extracted by lupdate from the upstream subclasses.
        let (context, text) = match self {
            Self::String => ("librepcb::AttrTypeString", "String"),
            Self::Resistance => ("librepcb::AttrTypeResistance", "Resistance"),
            Self::Capacitance => ("librepcb::AttrTypeCapacitance", "Capacitance"),
            Self::Inductance => ("librepcb::AttrTypeInductance", "Inductance"),
            Self::Voltage => ("librepcb::AttrTypeVoltage", "Voltage"),
            Self::Current => ("librepcb::AttrTypeCurrent", "Current"),
            Self::Power => ("librepcb::AttrTypePower", "Power"),
            Self::Frequency => ("librepcb::AttrTypeFrequency", "Frequency"),
        };
        translate(context, text).into_owned()
    }

    fn units(self) -> Units {
        let (all, default): (&'static [AttributeUnit], usize) = match self {
            Self::String => (&[], 0),
            Self::Resistance => (&RESISTANCE, 2),
            Self::Capacitance => (&CAPACITANCE, 2),
            Self::Inductance => (&INDUCTANCE, 2),
            Self::Voltage => (&VOLTAGE, 3),
            Self::Current => (&CURRENT, 4),
            Self::Power => (&POWER, 3),
            Self::Frequency => (&FREQUENCY, 2),
        };
        Units { all, default }
    }

    /// Returns all units of this type (empty for [`AttributeType::String`]),
    /// in ascending order.
    pub fn available_units(self) -> &'static [AttributeUnit] {
        self.units().all
    }

    /// Returns the default unit (`None` if the type has no units).
    pub fn default_unit(self) -> Option<&'static AttributeUnit> {
        let units = self.units();
        units.all.get(units.default)
    }

    /// Looks up a unit by its name.
    ///
    /// For types without units, the empty string and `"none"` return
    /// `Ok(None)`; any other string (including `"none"` for types with
    /// units) is an error.
    pub fn unit_from_string(self, unit: &str) -> Result<Option<&'static AttributeUnit>, Error> {
        let units = self.available_units();
        if (unit.is_empty() || unit == "none") && units.is_empty() {
            return Ok(None);
        }
        units
            .iter()
            .find(|u| u.name == unit)
            .map(Some)
            .ok_or_else(|| Error::UnknownUnit {
                type_name: self.name(),
                unit: unit.to_owned(),
            })
    }

    /// Returns whether `unit` is valid for this type: `None` for types
    /// without units, one of [`available_units()`](Self::available_units)
    /// otherwise.
    pub fn is_unit_available(self, unit: Option<&AttributeUnit>) -> bool {
        let units = self.available_units();
        match unit {
            None => units.is_empty(),
            Some(unit) => units.contains(unit),
        }
    }

    /// Splits a unit suffix off a user input value, e.g. `"4.7 k"` into
    /// `("4.7", kiloohm)` for [`AttributeType::Resistance`].
    ///
    /// The first unit (in ascending order) with a matching suffix wins; the
    /// remaining value is trimmed. Returns `None` if no suffix matches.
    pub fn extract_unit_from_value(self, value: &str) -> Option<(&str, &'static AttributeUnit)> {
        self.available_units().iter().find_map(|unit| {
            unit.user_input_suffixes
                .iter()
                .find_map(|suffix| value.strip_suffix(suffix))
                .map(|rest| (rest.trim(), unit))
        })
    }

    /// Returns whether `value` is valid for this type: any string for
    /// [`AttributeType::String`], otherwise empty or a floating point number
    /// (surrounding whitespace allowed).
    ///
    /// The accepted numbers are exactly those accepted by upstream's
    /// `QString::toFloat()` (file interoperability in both directions):
    /// `inf` (optional sign) and `nan` (no sign) in any case, and otherwise
    /// finite numbers within the `f32` range which don't underflow to zero.
    pub fn is_value_valid(self, value: &str) -> bool {
        match self {
            Self::String => true,
            _ => value.is_empty() || is_valid_float(value.trim()),
        }
    }

    /// Returns the value as displayed to the user, with the unit symbol
    /// appended if `unit` is given and `value` is not empty (never for
    /// [`AttributeType::String`]).
    ///
    /// Like upstream, the number is not formatted with the user's locale to
    /// keep output files deterministic (upstream issue #1814).
    pub fn printable_value_tr(self, value: &str, unit: Option<&AttributeUnit>) -> String {
        match (self, unit) {
            (Self::String, _) | (_, None) => value.to_owned(),
            (_, Some(_)) if value.is_empty() => String::new(),
            (_, Some(unit)) => format!("{value}{}", unit.symbol),
        }
    }
}

/// Returns whether `s` is a finite floating point number representable as
/// non-zero `f32` (or exactly zero).
fn is_valid_float(s: &str) -> bool {
    // Special values: "inf" with optional sign and "nan" without sign, both
    // case-insensitive (other spellings like "infinity" are invalid).
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    if unsigned.eq_ignore_ascii_case("inf") || s.eq_ignore_ascii_case("nan") {
        return true;
    }
    let Ok(value) = s.parse::<f64>() else {
        return false;
    };
    // Like upstream, the `f64` is rounded to `f32`; values rounding to
    // infinity overflow.
    if !value.is_finite() || !(value as f32).is_finite() {
        return false; // Other inf/nan spellings, or overflow.
    }
    // A non-zero literal must not round to zero (underflow).
    let mantissa = s.split(['e', 'E']).next().unwrap_or_default();
    let is_zero_literal = !mantissa.bytes().any(|b| (b'1'..=b'9').contains(&b));
    is_zero_literal || (value as f32) != 0.0
}

impl fmt::Display for AttributeType {
    /// Formats the identifier, e.g. `voltage`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for AttributeType {
    type Err = Error;
    /// Parses the identifier, e.g. `voltage` (upstream `fromString()`).
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::ALL
            .into_iter()
            .find(|t| t.name() == s)
            .ok_or_else(|| Error::InvalidType(s.to_owned()))
    }
}

// Serde: serialized as the identifier, e.g. `"voltage"`.
crate::utils::serde_string::serde_string!(AttributeType);

impl ToSExpression for AttributeType {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.name())
    }
}

impl FromSExpression for AttributeType {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_units() {
        let names: Vec<_> = AttributeType::ALL
            .iter()
            .map(|t| t.default_unit().map(AttributeUnit::name))
            .collect();
        assert_eq!(
            names,
            [
                None,
                Some("ohm"),
                Some("microfarad"),
                Some("millihenry"),
                Some("volt"),
                Some("ampere"),
                Some("watt"),
                Some("hertz"),
            ]
        );
    }

    #[test]
    fn unit_from_string() {
        let t = AttributeType::String;
        assert_eq!(t.unit_from_string(""), Ok(None));
        assert_eq!(t.unit_from_string("none"), Ok(None));
        assert!(t.unit_from_string("volt").is_err());
        let t = AttributeType::Voltage;
        assert_eq!(t.unit_from_string("volt").unwrap().unwrap().name(), "volt");
        assert!(t.unit_from_string("none").is_err());
        assert!(t.unit_from_string("ohm").is_err());
    }

    #[test]
    fn unit_available() {
        assert!(AttributeType::String.is_unit_available(None));
        assert!(!AttributeType::Voltage.is_unit_available(None));
        let volt = AttributeType::Voltage.default_unit();
        assert!(AttributeType::Voltage.is_unit_available(volt));
        assert!(!AttributeType::Current.is_unit_available(volt));
    }

    #[test]
    fn extract_unit() {
        let t = AttributeType::Resistance;
        let (value, unit) = t.extract_unit_from_value("4.7 k").unwrap();
        assert_eq!((value, unit.name()), ("4.7", "kiloohm"));
        let (value, unit) = t.extract_unit_from_value("10meg").unwrap();
        assert_eq!((value, unit.name()), ("10", "megaohm"));
        assert!(t.extract_unit_from_value("100").is_none());
        assert!(
            AttributeType::String
                .extract_unit_from_value("1k")
                .is_none()
        );
    }

    #[test]
    fn value_validity() {
        assert!(AttributeType::String.is_value_valid("foo"));
        assert!(AttributeType::Voltage.is_value_valid(""));
        assert!(AttributeType::Voltage.is_value_valid("4.2"));
        assert!(AttributeType::Voltage.is_value_valid(" 1e3 "));
        assert!(!AttributeType::Voltage.is_value_valid("foo"));
        assert!(!AttributeType::Voltage.is_value_valid("4,2"));
        let t = AttributeType::Resistance;
        // Results verified against QString::toFloat() of Qt 6.11.
        for valid in [
            "0",
            "-0.0",
            "0e-999",
            "+1.5",
            "-3.4e38",
            "3.4028235e38",
            "1e-40",
            ".5",
            "5.",
            " 4.2 ",
            "inf",
            "INF",
            "Inf",
            "+inf",
            "-inf",
            "-INF",
            " inf",
            "nan",
            "NaN",
            "NAN",
        ] {
            assert!(t.is_value_valid(valid), "{valid:?}");
        }
        for invalid in [
            "infinity",
            "Infinity",
            "+nan",
            "-nan",
            "-NaN",
            "nanx",
            "infx",
            "- inf",
            "1e39",
            "-1e39",
            "3.5e38",
            "3.4028236e38",
            "1e400",
            "1e-46",
            "1e-50",
            "1e-400",
            "1 5",
            "1e",
            "0x10",
            "1,5",
            "++1",
            "+-1",
        ] {
            assert!(!t.is_value_valid(invalid), "{invalid:?}");
        }
    }

    #[test]
    fn printable_value() {
        let t = AttributeType::Capacitance;
        let unit = t.default_unit();
        assert_eq!(t.printable_value_tr("100", unit), "100μF");
        assert_eq!(t.printable_value_tr("100", None), "100");
        assert_eq!(t.printable_value_tr("", unit), "");
        assert_eq!(AttributeType::String.printable_value_tr("x", unit), "x");
    }

    #[test]
    fn from_str() {
        for t in AttributeType::ALL {
            assert_eq!(t.name().parse::<AttributeType>(), Ok(t));
        }
        assert_eq!(
            "foo".parse::<AttributeType>(),
            Err(Error::InvalidType("foo".into()))
        );
    }
}
