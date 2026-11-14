//! Port of parseagle/common/enums.h (libs/parseagle).
//!
//! Each enum has an `Unknown` variant for attribute values which could not
//! be parsed; the parse error is appended to the list of parse errors
//! (upstream `QStringList* errors`).

macro_rules! eagle_enum {
    ($(#[$meta:meta])* $name:ident, $what:literal { $($variant:ident = $token:literal,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            /// Failed to parse the XML attribute.
            Unknown,
            $(
                #[doc = concat!("`", $token, "`")]
                $variant,
            )*
        }

        impl $name {
            /// Parses the attribute value `s`; unknown values are reported in
            /// `errors`.
            pub fn parse(s: &str, errors: &mut Vec<String>) -> Self {
                match s {
                    $($token => Self::$variant,)*
                    _ => {
                        errors.push(format!(concat!("Unknown ", $what, ": {}"), s));
                        Self::Unknown
                    }
                }
            }
        }
    };
}

eagle_enum!(
    /// Text alignment.
    Alignment, "alignment" {
        BottomLeft = "bottom-left",
        BottomCenter = "bottom-center",
        BottomRight = "bottom-right",
        CenterLeft = "center-left",
        Center = "center",
        CenterRight = "center-right",
        TopLeft = "top-left",
        TopCenter = "top-center",
        TopRight = "top-right",
    }
);

eagle_enum!(
    /// Display mode of an attribute.
    AttributeDisplay, "attribute display" {
        Off = "off",
        Value = "value",
        Name = "name",
        Both = "both",
    }
);

eagle_enum!(
    /// Text font.
    Font, "font" {
        Fixed = "fixed",
        Proportional = "proportional",
        Vector = "vector",
    }
);

eagle_enum!(
    /// Add level of a gate.
    GateAddLevel, "gate add level" {
        Must = "must",
        Can = "can",
        Next = "next",
        Request = "request",
        Always = "always",
    }
);

eagle_enum!(
    /// Grid style.
    GridStyle, "grid style" {
        Lines = "lines",
        Dots = "dots",
    }
);

eagle_enum!(
    /// Grid unit.
    GridUnit, "grid unit" {
        Micrometers = "mic",
        Millimeters = "mm",
        Mils = "mil",
        Inches = "inch",
    }
);

eagle_enum!(
    /// Shape of a THT pad.
    PadShape, "pad shape" {
        Square = "square",
        Round = "round",
        Octagon = "octagon",
        Long = "long",
        Offset = "offset",
    }
);

eagle_enum!(
    /// Electrical direction of a symbol pin.
    PinDirection, "pin direction" {
        NotConnected = "nc",
        Input = "in",
        Output = "out",
        Io = "io",
        OpenCollector = "oc",
        Power = "pwr",
        Passive = "pas",
        HighZ = "hiz",
        Supply = "sup",
    }
);

eagle_enum!(
    /// Graphical function of a symbol pin.
    PinFunction, "pin function" {
        None = "none",
        Dot = "dot",
        Clock = "clk",
        DotClock = "dotclk",
    }
);

eagle_enum!(
    /// Length of a symbol pin.
    PinLength, "pin length" {
        Point = "point",
        Short = "short",
        Middle = "middle",
        Long = "long",
    }
);

eagle_enum!(
    /// Visibility of pin and pad names.
    PinVisibility, "pin visibility" {
        Off = "off",
        Pad = "pad",
        Pin = "pin",
        Both = "both",
    }
);

eagle_enum!(
    /// Pour mode of a polygon.
    PolygonPour, "polygon pour" {
        Solid = "solid",
        Hatch = "hatch",
        Cutout = "cutout",
    }
);

eagle_enum!(
    /// Shape of a via.
    ViaShape, "via shape" {
        Square = "square",
        Round = "round",
        Octagon = "octagon",
    }
);

eagle_enum!(
    /// Cap style of a wire.
    WireCap, "wire cap" {
        Flat = "flat",
        Round = "round",
    }
);

eagle_enum!(
    /// Line style of a wire.
    WireStyle, "wire style" {
        Continuous = "continuous",
        LongDash = "longdash",
        ShortDash = "shortdash",
        DashDot = "dashdot",
    }
);
