//! Conversions between core types and the Slint API types, and the pure
//! helper callbacks of the `Backend` global.
//!
//! Port of libs/librepcb/editor/utils/uihelpers.{h,cpp} (`s2l()`/`l2s()`
//! conversions) and of the `Backend` callbacks registered in
//! `GuiApplication::createNewWindow()` (libs/librepcb/editor/guiapplication.cpp):
//! `format-length`, `parse-length-input`, `format-angle`,
//! `parse-angle-input`, `format-ratio`, `parse-ratio-input` and
//! `is-shortcut`.
//!
//! Numbers are formatted with `.` as decimal separator (no `QLocale`, see
//! `COMPAT.md`).

use librepcb_app_ui as ui;
use librepcb_canvas::peniko;
use librepcb_core::types::{Angle, GridStyle, Length, LengthUnit, Ratio};
use librepcb_core::utils::math_parser::MathParser;
use slint::SharedString;

/// Converts the UI's 64 bit integer (`Int64 { msb, lsb }`) to `i64`.
pub fn int64_to_i64(v: ui::Int64) -> i64 {
    (i64::from(v.msb) << 32) | i64::from(v.lsb as u32)
}

/// Converts an `i64` to the UI's 64 bit integer.
pub fn i64_to_int64(v: i64) -> ui::Int64 {
    ui::Int64 {
        msb: (v >> 32) as i32,
        lsb: v as u32 as i32,
    }
}

/// Converts a length to the UI's 64 bit integer.
pub fn length_to_ui(l: Length) -> ui::Int64 {
    i64_to_int64(l.to_nm())
}

/// Converts the UI's 64 bit integer to a length.
pub fn length_from_ui(v: ui::Int64) -> Length {
    Length::new(int64_to_i64(v))
}

/// Converts a length unit to the UI enum.
pub fn unit_to_ui(u: LengthUnit) -> ui::LengthUnit {
    match u {
        LengthUnit::Millimeters => ui::LengthUnit::Millimeters,
        LengthUnit::Micrometers => ui::LengthUnit::Micrometers,
        LengthUnit::Nanometers => ui::LengthUnit::Nanometers,
        LengthUnit::Inches => ui::LengthUnit::Inches,
        LengthUnit::Mils => ui::LengthUnit::Mils,
    }
}

/// Converts the UI enum to a length unit.
pub fn unit_from_ui(u: ui::LengthUnit) -> LengthUnit {
    match u {
        ui::LengthUnit::Millimeters => LengthUnit::Millimeters,
        ui::LengthUnit::Micrometers => LengthUnit::Micrometers,
        ui::LengthUnit::Nanometers => LengthUnit::Nanometers,
        ui::LengthUnit::Inches => LengthUnit::Inches,
        ui::LengthUnit::Mils => LengthUnit::Mils,
    }
}

/// Converts a grid style to the UI enum.
pub fn grid_style_to_ui(s: GridStyle) -> ui::GridStyle {
    match s {
        GridStyle::None => ui::GridStyle::None,
        GridStyle::Dots => ui::GridStyle::Dots,
        GridStyle::Lines => ui::GridStyle::Lines,
    }
}

/// Converts the UI enum to a grid style.
pub fn grid_style_from_ui(s: ui::GridStyle) -> GridStyle {
    match s {
        ui::GridStyle::None => GridStyle::None,
        ui::GridStyle::Dots => GridStyle::Dots,
        ui::GridStyle::Lines => GridStyle::Lines,
    }
}

/// Converts a canvas (peniko) color to a Slint color.
pub fn color_to_ui(c: peniko::Color) -> slint::Color {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    slint::Color::from_argb_u8(a, r, g, b)
}

/// Parses a `#rrggbb` or `#aarrggbb` string (upstream's notation).
pub fn parse_hex_color(s: &str) -> Option<slint::Color> {
    let hex = s.strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some(slint::Color::from_rgb_u8(
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        )),
        8 => Some(slint::Color::from_argb_encoded(v)),
        _ => None,
    }
}

/// Upstream `Toolbox::floatToString()`: fixed decimals with trailing zeros
/// removed, keeping at least one decimal.
pub fn float_to_string(value: f64, decimals: usize) -> String {
    let mut s = format!("{value:.decimals$}");
    if decimals > 0 {
        let min_len = s.find('.').map_or(s.len(), |dot| dot + 2);
        while s.len() > min_len && s.ends_with('0') {
            s.pop();
        }
    }
    s
}

/// `Backend.format-length`.
pub fn format_length(value: ui::Int64, unit: ui::LengthUnit) -> SharedString {
    let unit = unit_from_ui(unit);
    float_to_string(
        unit.convert_to_unit(length_from_ui(value)),
        unit.reasonable_number_of_decimals(),
    )
    .into()
}

/// `Backend.parse-length-input`: evaluates a math expression with an
/// optional unit suffix; valid if the result is at least `minimum`.
pub fn parse_length_input(
    text: &str,
    unit: ui::LengthUnit,
    minimum: ui::Int64,
) -> ui::LengthEditParseResult {
    let mut res = ui::LengthEditParseResult {
        valid: false,
        evaluated_value: i64_to_int64(0),
        evaluated_unit: unit,
    };
    let mut expression = text.to_owned();
    if let Some(parsed) = LengthUnit::extract_from_expression(&mut expression) {
        res.evaluated_unit = unit_to_ui(parsed);
    }
    let unit = unit_from_ui(res.evaluated_unit);
    if let Ok(value) = MathParser::new().parse(&expression)
        && let Ok(length) = unit.convert_from_unit(value)
        && length >= length_from_ui(minimum)
    {
        res.evaluated_value = length_to_ui(length);
        res.valid = true;
    }
    res
}

/// `Backend.format-angle` (value in microdegrees).
pub fn format_angle(value: i32) -> SharedString {
    float_to_string(Angle::new(value).to_deg(), 3).into()
}

/// `Backend.parse-angle-input`.
pub fn parse_angle_input(text: &str) -> ui::AngleEditParseResult {
    let expression = text.replace('°', "");
    match MathParser::new()
        .parse(&expression)
        .ok()
        .and_then(|deg| Angle::from_deg(deg).ok())
    {
        Some(angle) => ui::AngleEditParseResult {
            valid: true,
            evaluated_value: angle.to_micro_deg(),
        },
        None => ui::AngleEditParseResult {
            valid: false,
            evaluated_value: 0,
        },
    }
}

/// `Backend.format-ratio` (value in ppm).
pub fn format_ratio(value: i32) -> SharedString {
    float_to_string(Ratio::new(value).to_percent(), 3).into()
}

/// `Backend.parse-ratio-input` (bounds in ppm).
pub fn parse_ratio_input(text: &str, minimum: i32, maximum: i32) -> ui::RatioEditParseResult {
    let expression = text.replace(['%', ' '], "");
    match MathParser::new().parse(&expression) {
        Ok(percent) if percent.is_finite() => {
            let ratio = Ratio::from_percent_f64(percent).to_ppm();
            let valid = (minimum..=maximum).contains(&ratio);
            ui::RatioEditParseResult {
                valid,
                evaluated_value: if valid { ratio } else { 0 },
            }
        }
        _ => ui::RatioEditParseResult {
            valid: false,
            evaluated_value: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int64_roundtrip() {
        for v in [0, 1, -1, i64::MAX, i64::MIN, 0x1_2345_6789, -0x1_0000_0000] {
            assert_eq!(int64_to_i64(i64_to_int64(v)), v);
        }
        let i = i64_to_int64(0x1_ffff_ffff);
        assert_eq!((i.msb, i.lsb), (1, -1));
    }

    #[test]
    fn float_formatting() {
        assert_eq!(float_to_string(1.5, 3), "1.5");
        assert_eq!(float_to_string(2.0, 3), "2.0");
        assert_eq!(float_to_string(0.1234, 3), "0.123");
        assert_eq!(float_to_string(-2.54, 6), "-2.54");
        assert_eq!(float_to_string(12.0, 0), "12");
        assert_eq!(float_to_string(1000.25, 2), "1000.25");
    }

    #[test]
    fn lengths() {
        let mm = ui::LengthUnit::Millimeters;
        assert_eq!(format_length(i64_to_int64(2_540_000), mm), "2.54");
        assert_eq!(
            format_length(i64_to_int64(2_540_000), ui::LengthUnit::Mils),
            "100.0"
        );
        let r = parse_length_input("2*0.5", mm, i64_to_int64(0));
        assert!(r.valid);
        assert_eq!(int64_to_i64(r.evaluated_value), 1_000_000);
        let r = parse_length_input("10mil", mm, i64_to_int64(0));
        assert!(r.valid);
        assert_eq!(r.evaluated_unit, ui::LengthUnit::Mils);
        assert_eq!(int64_to_i64(r.evaluated_value), 254_000);
        assert!(!parse_length_input("-1", mm, i64_to_int64(0)).valid);
        assert!(!parse_length_input("foo", mm, i64_to_int64(0)).valid);
    }

    #[test]
    fn angles_and_ratios() {
        assert_eq!(format_angle(90_000_000), "90.0");
        assert_eq!(format_angle(12_345_678), "12.346");
        let a = parse_angle_input("45°");
        assert!(a.valid);
        assert_eq!(a.evaluated_value, 45_000_000);
        assert!(!parse_angle_input("x").valid);
        assert_eq!(format_ratio(500_000), "50.0");
        let r = parse_ratio_input("12.5 %", 0, 1_000_000);
        assert!(r.valid);
        assert_eq!(r.evaluated_value, 125_000);
        assert!(!parse_ratio_input("150", 0, 1_000_000).valid);
    }

    #[test]
    fn colors() {
        let c = parse_hex_color("#80ff0000").unwrap();
        assert_eq!((c.alpha(), c.red(), c.green(), c.blue()), (0x80, 255, 0, 0));
        let c = parse_hex_color("#29d682").unwrap();
        assert_eq!((c.alpha(), c.red()), (255, 0x29));
        assert!(parse_hex_color("29d682").is_none());
    }
}
