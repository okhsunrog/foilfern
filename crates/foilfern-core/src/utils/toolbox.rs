//! Port of libs/librepcb/core/utils/toolbox.{h,cpp} (plus
//! `increment_number_in_string()` of libs/librepcb/rust-core/src/toolbox.rs,
//! which upstream calls via FFI).
//!
//! Not ported:
//! - The Qt container helpers (`toSet()`, `sortedQSet()`, `sortNumeric()`,
//!   ...): use the standard library instead, with [`compare_numeric()`] as
//!   comparison for `sortNumeric()`.
//! - The Qt graphics helpers (`shapeFromPath()`, `boundingRectFromRadius()`,
//!   `adjustedBoundingRect()`) and the locale dependent `floatToString()` and
//!   `prettyPrintLocale()`: they belong to the UI layer.
//!
//! The geometry functions are hand-written instead of using a geometry crate
//! (e.g. `kurbo`) because their exact floating point operations and rounding
//! define the file content (arc centers, flattened arcs, ...).

use std::cmp::Ordering;
use std::sync::LazyLock;

use icu_collator::options::{AlternateHandling, CollatorOptions, Strength};
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{CollatorBorrowed, CollatorPreferences};
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use super::math;
use crate::types::{Angle, Error, Length, Point, UnsignedLength};

/// Returns whether a text with the given rotation would be upside down
/// (i.e. the rotation mapped to ]-180°..180°] is not within ]-90°..90°]).
pub fn is_text_upside_down(rotation: Angle) -> bool {
    let mapped180 = rotation.mapped_to_180deg();
    (mapped180 <= -Angle::DEG90) || (mapped180 > Angle::DEG90)
}

/// Returns the radius of an arc given by start point, end point and angle.
///
/// The radius is negative for clockwise arcs. Returns `None` if the input
/// is a straight line or the radius is too large.
pub fn arc_radius(p1: Point, p2: Point, angle: Angle) -> Option<Length> {
    let angle_mapped = angle.mapped_to_180deg();
    if (angle_mapped == Angle::DEG0) || (p1 == p2) {
        return None; // Given input is a straight line.
    }
    let delta = p2 - p1;
    let r = math::arc_radius(
        delta.x.to_nm() as f64,
        delta.y.to_nm() as f64,
        angle_mapped.to_deg(),
    );
    Length::from_nm_f64(r).ok() // `None` for too large radius.
}

/// Returns the center of an arc given by start point, end point and angle.
///
/// Returns `None` if the input is a straight line or the center is too far
/// away (i.e. it is practically a straight line).
pub fn arc_center(p1: Point, p2: Point, angle: Angle) -> Option<Point> {
    let angle_mapped = angle.mapped_to_180deg();
    if (angle_mapped == Angle::DEG0) || (p1 == p2) {
        return None; // Given input is a straight line.
    }
    let delta = p2 - p1;
    let (_, (x, y)) = math::arc_radius_and_center(
        delta.x.to_nm() as f64,
        delta.y.to_nm() as f64,
        angle_mapped.to_deg(),
    );
    let x = Length::from_nm_f64(x + p1.x.to_nm() as f64).ok()?;
    let y = Length::from_nm_f64(y + p1.y.to_nm() as f64).ok()?;
    Some(Point::new(x, y))
}

/// Returns the angle of the arc from `p1` to `p2` around `center`, mapped to
/// [0°..360°[ (0° if a point equals the center).
pub fn arc_angle(p1: Point, p2: Point, center: Point) -> Angle {
    let delta1 = p1 - center;
    let delta2 = p2 - center;
    if delta1.is_origin() || delta2.is_origin() {
        return Angle::DEG0;
    }
    let angle1 = libm::atan2(delta1.y.to_mm(), delta1.x.to_mm());
    let angle2 = libm::atan2(delta2.y.to_mm(), delta2.x.to_mm());
    Angle::from_rad(angle2 - angle1)
        .map(Angle::mapped_to_0_360deg)
        .unwrap_or(Angle::DEG0)
}

/// Returns the angle of the arc from `start` to `end` passing through `mid`
/// (approximation, not exact).
pub fn arc_angle_from_3_points(start: Point, mid: Point, end: Point) -> Angle {
    let (h, _) = shortest_distance_between_point_and_line(mid, start, end);
    let c = (end - start).length();
    let phi = 4.0 * libm::atan((2.0 * h.to_mm()) / c.to_mm());
    match Angle::from_rad(phi) {
        Ok(angle) => {
            let angle_mid = angle_between_points(start, mid);
            let angle_end = angle_between_points(start, end);
            let angle_delta = angle_mid - angle_end;
            if angle_delta.mapped_to_180deg() < Angle::DEG0 {
                angle
            } else {
                -angle
            }
        }
        Err(_) => Angle::DEG0,
    }
}

/// Returns the angle of the vector from `p1` to `p2`, mapped to [0°..360°[
/// (0° if both points are equal).
pub fn angle_between_points(p1: Point, p2: Point) -> Angle {
    let delta = p2 - p1;
    if delta.is_origin() {
        return Angle::DEG0;
    }
    // The angle is always within [-180°..180°], i.e. valid.
    Angle::from_deg(math::angle_to_point(
        delta.x.to_nm() as f64,
        delta.y.to_nm() as f64,
    ))
    .unwrap_or_default()
    .mapped_to_0_360deg()
}

/// Returns the point on the line segment `l1`..`l2` which is nearest to `p`.
pub fn nearest_point_on_line(p: Point, l1: Point, l2: Point) -> Point {
    let a = l2 - l1;
    let b = p - l1;
    let c = p - l2;
    let d = (b.x.to_mm() * a.x.to_mm()) + (b.y.to_mm() * a.y.to_mm());
    let e = (a.x.to_mm() * a.x.to_mm()) + (a.y.to_mm() * a.y.to_mm());
    if a.is_origin() || b.is_origin() || (d <= 0.0) {
        l1
    } else if c.is_origin() || (e <= d) {
        l2
    } else {
        // The result lies between l1 and l2, i.e. is always in range.
        let offset = Point::from_mm(a.x.to_mm() * d / e, a.y.to_mm() * d / e).unwrap_or_default();
        l1 + offset
    }
}

/// Returns the shortest distance between `p` and the line segment
/// `l1`..`l2`, together with the nearest point on the line.
pub fn shortest_distance_between_point_and_line(
    p: Point,
    l1: Point,
    l2: Point,
) -> (UnsignedLength, Point) {
    let nearest = nearest_point_on_line(p, l1, l2);
    ((p - nearest).length(), nearest)
}

/// Copies a string while incrementing its contained number.
///
/// If the string contains numbers, the last one gets incremented, otherwise
/// a `"1"` is appended. Useful to generate unique names like `"X1"`, `"X2"`.
pub fn increment_number_in_string(s: &str) -> String {
    static NUMBER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[0-9]+").expect("valid regex literal"));
    if let Some(m) = NUMBER.find_iter(s).last()
        && let Ok(num) = m.as_str().parse::<i32>()
    {
        // Wrapping like the upstream (release build) Rust code.
        let mut ret = s.to_owned();
        ret.replace_range(m.range(), &num.wrapping_add(1).to_string());
        return ret;
    }
    format!("{s}1") // Fallback: just add a "1" at the end.
}

/// Expands number and letter ranges in a string, e.g. `"X1..3"` to `["X1",
/// "X2", "X3"]` or `"0..1_A..B"` to `["0_A", "0_B", "1_A", "1_B"]`.
///
/// Signs are not considered part of numbers (`"X-1..3"` expands to `"X-1"`,
/// `"X-2"`, `"X-3"`), descending ranges are allowed, and at most 4 ranges
/// are expanded.
pub fn expand_ranges_in_string(s: &str) -> Vec<String> {
    static RANGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?<num>(?<num_start>[0-9]+)\.\.(?<num_end>[0-9]+))|(?<char>(?<char_start>[a-zA-Z])\.\.(?<char_end>[a-zA-Z]))",
        )
        .expect("valid regex literal")
    });
    let mut replacements: Vec<(std::ops::Range<usize>, Vec<String>)> = Vec::new();
    for caps in RANGE.captures_iter(s) {
        let mut values: Vec<String> = Vec::new();
        if let Some(num) = caps.name("num") {
            let start = caps["num_start"].parse::<i32>();
            let end = caps["num_end"].parse::<i32>();
            if let (Ok(start), Ok(end)) = (start, end) {
                values = if start > end {
                    (end..=start).rev().map(|i| i.to_string()).collect()
                } else {
                    (start..=end).map(|i| i.to_string()).collect()
                };
            }
            push_range(&mut replacements, num.range(), values);
        } else if let Some(chars) = caps.name("char") {
            let start = caps["char_start"].as_bytes()[0];
            let end = caps["char_end"].as_bytes()[0];
            let (lo, hi) = (start.min(end), start.max(end));
            if (lo >= b'a' && hi <= b'z') || (lo >= b'A' && hi <= b'Z') {
                values = (lo..=hi).map(|c| char::from(c).to_string()).collect();
                if start > end {
                    values.reverse();
                }
            }
            push_range(&mut replacements, chars.range(), values);
        }
    }
    expand_ranges(s, &replacements)
}

/// Adds a range replacement (max. 4 to avoid huge results).
fn push_range(
    replacements: &mut Vec<(std::ops::Range<usize>, Vec<String>)>,
    range: std::ops::Range<usize>,
    values: Vec<String>,
) {
    if !values.is_empty() && replacements.len() < 4 {
        replacements.push((range, values));
    }
}

fn expand_ranges(
    input: &str,
    replacements: &[(std::ops::Range<usize>, Vec<String>)],
) -> Vec<String> {
    let Some(((range, values), rest)) = replacements.split_first() else {
        return vec![input.to_owned()];
    };
    // Later ranges are replaced first, so the byte offsets stay valid.
    let tails = expand_ranges(input, rest);
    let mut result = Vec::with_capacity(values.len() * tails.len());
    for value in values {
        for tail in &tails {
            let mut s = tail.clone();
            s.replace_range(range.clone(), value);
            result.push(s);
        }
    }
    result
}

/// Converts a fixed point decimal number to a string, e.g. `1234567` with
/// `point_pos = 6` to `"1.234567"`.
///
/// Trailing zeros are removed, but at least one decimal is always kept (e.g.
/// `"1.0"`). Zero is always formatted as `"0.0"`.
pub fn decimal_fixed_point_to_string(value: i64, point_pos: usize) -> String {
    if value == 0 {
        return "0.0".to_owned();
    }
    let mut s = value.unsigned_abs().to_string();
    if s.len() > point_pos {
        s.insert(s.len() - point_pos, '.');
    } else {
        s.insert_str(0, &"0".repeat(point_pos - s.len()));
        s.insert_str(0, "0.");
    }
    while s.ends_with('0') && !s.ends_with(".0") {
        s.pop();
    }
    if value < 0 {
        s.insert(0, '-');
    }
    s
}

/// Target integer type of [`decimal_fixed_point_from_string()`].
///
/// Upstream is a template over the integer type, and the overflow checks are
/// performed in the corresponding *unsigned* type, which affects which inputs
/// are accepted. This trait provides the required limits.
pub trait FixedPointInt: Copy {
    /// `std::numeric_limits<std::make_unsigned<T>>::max()`.
    const MAX_UNSIGNED: u128;
    /// Absolute value of `std::numeric_limits<T>::min()`.
    const MIN_ABS: u128;
    /// `std::numeric_limits<T>::max()`.
    const MAX: u128;
    /// Converts the (range-checked) signed value to `Self`.
    fn from_i128(value: i128) -> Self;
}

impl FixedPointInt for i64 {
    const MAX_UNSIGNED: u128 = u64::MAX as u128;
    const MIN_ABS: u128 = i64::MIN.unsigned_abs() as u128;
    const MAX: u128 = i64::MAX as u128;
    fn from_i128(value: i128) -> Self {
        // Range is checked by the caller against MIN_ABS/MAX.
        value as i64
    }
}

impl FixedPointInt for i32 {
    const MAX_UNSIGNED: u128 = u32::MAX as u128;
    const MIN_ABS: u128 = i32::MIN.unsigned_abs() as u128;
    const MAX: u128 = i32::MAX as u128;
    fn from_i128(value: i128) -> Self {
        // Range is checked by the caller against MIN_ABS/MAX.
        value as i32
    }
}

/// Converts a fixed point decimal number string to an integer, e.g. `"1.5"`
/// with `point_pos = 6` to `1500000`.
///
/// Accepts an optional sign, an integer part, a fractional part and an
/// exponent (e.g. `"-1.5e-3"`). Fails if the string is malformed, has more
/// decimals than `point_pos`, or the value is out of range of `T`.
pub fn decimal_fixed_point_from_string<T: FixedPointInt>(
    s: &str,
    point_pos: u32,
) -> Result<T, Error> {
    parse_fixed_point::<T>(s, point_pos).ok_or_else(|| Error::InvalidFixedPointNumber(s.to_owned()))
}

fn parse_fixed_point<T: FixedPointInt>(s: &str, point_pos: u32) -> Option<T> {
    #[derive(PartialEq, Eq, Clone, Copy)]
    enum State {
        Invalid,
        Start,
        AfterSign,
        LonelyDot,
        IntPart,
        FracPart,
        Exp,
        ExpAfterSign,
        ExpDigits,
    }

    let max_u = T::MAX_UNSIGNED;
    let mut state = State::Start;
    let mut value_abs: u128 = 0;
    let mut sign = false;
    let mut exp_offset: i64 = i64::from(point_pos);
    let max_exp = u32::MAX;
    let mut exp: u32 = 0;
    let mut exp_sign = false;

    // Appends a digit to `value`, returning `None` on overflow of `max`.
    fn push_digit<V>(value: V, digit: V, max: V) -> Option<V>
    where
        V: Copy
            + PartialOrd
            + std::ops::Div<Output = V>
            + std::ops::Mul<Output = V>
            + std::ops::Add<Output = V>
            + std::ops::Sub<Output = V>
            + From<u8>,
    {
        if value > max / V::from(10) {
            return None;
        }
        let value = value * V::from(10);
        if value > max - digit {
            return None;
        }
        Some(value + digit)
    }

    for c in s.chars() {
        // Only ASCII digits (upstream `QChar::isDigit()` also accepts other
        // Unicode decimal digits, see COMPAT.md).
        let digit = c.to_digit(10);
        state = match state {
            State::Invalid => break,
            State::Start => match (c, digit) {
                ('-', _) => {
                    sign = true;
                    State::AfterSign
                }
                ('+', _) => State::AfterSign,
                ('.', _) => State::LonelyDot,
                (_, Some(d)) => {
                    value_abs = u128::from(d);
                    State::IntPart
                }
                _ => State::Invalid,
            },
            State::AfterSign => match (c, digit) {
                ('.', _) => State::LonelyDot,
                (_, Some(d)) => {
                    value_abs = u128::from(d);
                    State::IntPart
                }
                _ => State::Invalid,
            },
            State::LonelyDot => match digit {
                Some(d) => {
                    value_abs = u128::from(d);
                    exp_offset -= 1;
                    State::FracPart
                }
                None => State::Invalid,
            },
            State::IntPart => match (c, digit) {
                ('.', _) => State::FracPart,
                ('e' | 'E', _) => State::Exp,
                (_, Some(d)) => match push_digit(value_abs, u128::from(d), max_u) {
                    Some(v) => {
                        value_abs = v;
                        State::IntPart
                    }
                    None => State::Invalid,
                },
                _ => State::Invalid,
            },
            State::FracPart => match (c, digit) {
                ('e' | 'E', _) => State::Exp,
                (_, Some(d)) => match push_digit(value_abs, u128::from(d), max_u) {
                    Some(v) => {
                        value_abs = v;
                        exp_offset -= 1;
                        State::FracPart
                    }
                    None => State::Invalid,
                },
                _ => State::Invalid,
            },
            State::Exp => match (c, digit) {
                ('-', _) => {
                    exp_sign = true;
                    State::ExpAfterSign
                }
                ('+', _) => State::ExpAfterSign,
                (_, Some(d)) => {
                    exp = d;
                    State::ExpDigits
                }
                _ => State::Invalid,
            },
            State::ExpAfterSign => match digit {
                Some(d) => {
                    exp = d;
                    State::ExpDigits
                }
                None => State::Invalid,
            },
            State::ExpDigits => match digit.and_then(|d| push_digit(exp, d, max_exp)) {
                Some(e) => {
                    exp = e;
                    State::ExpDigits
                }
                None => State::Invalid,
            },
        };
    }

    if !matches!(state, State::IntPart | State::FracPart | State::ExpDigits) {
        return None;
    }

    // Merge the exponent offset (from the point position and the number of
    // fractional digits) into the exponent.
    let exp_offset_abs = u32::try_from(exp_offset.unsigned_abs()).ok()?;
    if exp_sign == (exp_offset < 0) {
        exp = exp.checked_add(exp_offset_abs)?;
    } else {
        if exp < exp_offset_abs {
            return None;
        }
        exp -= exp_offset_abs;
    }

    if value_abs == 0 {
        // No need to apply exponent or sign if the value is zero.
        return Some(T::from_i128(0));
    }
    for _ in 0..exp {
        if exp_sign {
            if !value_abs.is_multiple_of(10) {
                return None; // More decimal digits than allowed.
            }
            value_abs /= 10;
        } else {
            if value_abs > max_u / 10 {
                return None; // Would overflow.
            }
            value_abs *= 10;
        }
    }
    if sign {
        if value_abs > T::MIN_ABS {
            return None;
        }
        Some(T::from_i128(-(value_abs as i128)))
    } else {
        if value_abs > T::MAX {
            return None;
        }
        Some(T::from_i128(value_abs as i128))
    }
}

/// Cleans a user input string (equivalent of `Toolbox::cleanUserInputString()`).
///
/// Performs NFKD normalization, optional case conversion and trimming,
/// replaces spaces by `space_replacement`, removes all characters for which
/// `is_allowed` returns `false`, and truncates the result to `max_length`
/// characters (upstream: UTF-16 code units, which is the same for all
/// callers since they only allow ASCII characters).
pub fn clean_user_input_string(
    input: &str,
    is_allowed: impl Fn(char) -> bool,
    trim: bool,
    to_lower: bool,
    to_upper: bool,
    space_replacement: &str,
    max_length: Option<usize>,
) -> String {
    let mut ret: String = input.nfkd().collect();
    if to_lower {
        ret = ret.to_lowercase();
    }
    if to_upper {
        ret = ret.to_uppercase();
    }
    if trim {
        ret = ret.trim().to_owned();
    }
    ret = ret.replace(' ', space_replacement);
    ret.retain(is_allowed);
    if let Some(max) = max_length
        && let Some((index, _)) = ret.char_indices().nth(max)
    {
        ret.truncate(index);
    }
    if trim {
        ret = ret.trim().to_owned();
    }
    ret
}

/// The collator behind [`compare_numeric()`].
static NUMERIC_COLLATOR: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| {
    let mut prefs = CollatorPreferences::default();
    prefs.numeric_ordering = Some(CollationNumericOrdering::True);
    let mut options = CollatorOptions::default();
    // Secondary strength ignores case differences (like QCollator with
    // Qt::CaseInsensitive), non-ignorable punctuation is QCollator's
    // `ignorePunctuation == false`.
    options.strength = Some(Strength::Secondary);
    options.alternate_handling = Some(AlternateHandling::NonIgnorable);
    CollatorBorrowed::try_new(prefs, options)
        .expect("compiled collation data for the root locale is always available")
});

/// Compares two strings for sorting them "naturally" for humans: numbers
/// are compared numerically (`R2` < `R10`), case is ignored.
///
/// This is the comparison of upstream `Toolbox::sortNumeric()` (a
/// `QCollator` in numeric mode, case insensitive, punctuation not
/// ignored), implemented with ICU4X's collator for the root locale instead
/// of the system ICU with the system locale. Used to sort designators in
/// exported files (BOM, pick&place), so it defines the file content.
pub fn compare_numeric(lhs: &str, rhs: &str) -> Ordering {
    NUMERIC_COLLATOR.compare(lhs, rhs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_string() {
        assert_eq!(decimal_fixed_point_to_string(0, 6), "0.0");
        assert_eq!(decimal_fixed_point_to_string(1, 6), "0.000001");
        assert_eq!(decimal_fixed_point_to_string(-1_000_000, 6), "-1.0");
        assert_eq!(
            decimal_fixed_point_to_string(i64::MIN, 6),
            "-9223372036854.775808"
        );
    }

    #[test]
    fn from_string_width_matters() {
        // Overflow checks are done in the unsigned type of the target width.
        assert!(decimal_fixed_point_from_string::<i32>("1000000000000e-12", 6).is_err());
        assert_eq!(
            decimal_fixed_point_from_string::<i64>("1000000000000e-12", 6).unwrap(),
            1_000_000
        );
        assert_eq!(
            decimal_fixed_point_from_string::<i32>("1000000000e-9", 6).unwrap(),
            1_000_000
        );
    }

    #[test]
    fn from_string_exponent_quirk() {
        // Upstream rejects a negative exponent smaller than the number of
        // fixed point decimals (the exponent offset merge fails).
        assert!(decimal_fixed_point_from_string::<i64>("1e-3", 6).is_err());
        assert_eq!(
            decimal_fixed_point_from_string::<i64>("1e-6", 6).unwrap(),
            1
        );
        assert_eq!(
            decimal_fixed_point_from_string::<i64>("1e3", 6).unwrap(),
            1_000_000_000
        );
    }

    #[test]
    fn from_string_unicode_digits() {
        // Only ASCII digits are accepted (see COMPAT.md).
        assert!(decimal_fixed_point_from_string::<i64>("\u{0661}.\u{0665}", 6).is_err());
    }

    #[test]
    fn clean() {
        let s = clean_user_input_string(
            " Foo Bär ",
            |c| c.is_ascii_lowercase() || c == '-',
            true,
            true,
            false,
            "-",
            Some(32),
        );
        assert_eq!(s, "foo-bar");
    }
}
