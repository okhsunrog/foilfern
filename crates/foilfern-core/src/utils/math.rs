//! Port of libs/librepcb/rust-core/src/math.rs (the deterministic math the
//! C++ types and `Toolbox` call through the Rust FFI).
//!
//! All functions are deterministic across platforms because they use the
//! floating point implementation of the [`libm`] crate, exactly like upstream.

/// π/180 as a deterministic constant (bit-identical to upstream).
const PI_DIV_180: f64 = f64::from_bits(0x3F91DF46A2529D39);

/// Converts degrees to radians.
pub fn to_radians(degrees: f64) -> f64 {
    degrees * PI_DIV_180
}

/// Converts radians to degrees.
pub fn to_degrees(radians: f64) -> f64 {
    radians / PI_DIV_180
}

/// Rotates the point `(x, y)` by `angle` degrees (counter-clockwise) around
/// the origin.
///
/// This always uses floating point math; callers should handle multiples of
/// 90° explicitly. `angle` is expected to be in the range ]-360..360[.
pub fn rotate_point(x: f64, y: f64, angle: f64) -> (f64, f64) {
    debug_assert!((angle > -360.0) && (angle < 360.0));
    let radians = to_radians(angle);
    let sin = libm::sin(radians);
    let cos = libm::cos(radians);
    (cos * x - sin * y, sin * x + cos * y)
}

/// Returns the angle in degrees from the origin to the point `(x, y)` (east =
/// 0, north = 90, west = 180, south = -90).
///
/// This always uses floating point math; callers should handle multiples of
/// 90° explicitly.
pub fn angle_to_point(x: f64, y: f64) -> f64 {
    to_degrees(libm::atan2(y, x))
}

/// Returns the radius of the arc from the origin to `(dx, dy)` with the
/// given angle in degrees (counter-clockwise, in the range ]-360..360[).
///
/// Invalid input (e.g. a zero angle) leads to NaN or infinite output, which
/// the caller has to check.
pub fn arc_radius(dx: f64, dy: f64, angle: f64) -> f64 {
    debug_assert!((angle > -360.0) && (angle < 360.0));
    let d = libm::sqrt(dx * dx + dy * dy);
    d / (2.0 * libm::sin(to_radians(angle / 2.0)))
}

/// Returns the radius and center of the arc from the origin to `(dx, dy)`
/// with the given angle in degrees (counter-clockwise, in the range
/// ]-360..360[). See <https://math.stackexchange.com/questions/27535/>.
///
/// Invalid input (e.g. a zero angle) leads to NaN or infinite output, which
/// the caller has to check.
pub fn arc_radius_and_center(dx: f64, dy: f64, angle: f64) -> (f64, (f64, f64)) {
    debug_assert!((angle > -360.0) && (angle < 360.0));
    let d = libm::sqrt(dx * dx + dy * dy);
    let r = d / (2.0 * libm::sin(to_radians(angle / 2.0)));
    let mut h0 = r * r - d * d / 4.0;
    if h0 <= 0.0 {
        h0 = 0.0; // Fixes https://github.com/LibrePCB/LibrePCB/issues/974.
    }
    let h = libm::sqrt(h0);
    let u = dx / d;
    let v = dy / d;
    let angle_sgn = if angle >= 0.0 { 1.0 } else { -1.0 };
    let x = (dx / 2.0) - h * v * angle_sgn;
    let y = (dy / 2.0) + h * u * angle_sgn;
    (r, (x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc() {
        let r = arc_radius(2.0, 0.0, 180.0);
        assert!((r - 1.0).abs() < 1e-12);
        let (r, (x, y)) = arc_radius_and_center(2.0, 0.0, -180.0);
        assert!((r + 1.0).abs() < 1e-12);
        assert!((x - 1.0).abs() < 1e-12);
        assert!(y.abs() < 1e-12);
        assert!((angle_to_point(0.0, 1.0) - 90.0).abs() < 1e-12);
    }

    #[test]
    fn degrees_radians_roundtrip() {
        assert_eq!(to_degrees(std::f64::consts::PI), 180.0);
        assert_eq!(to_radians(180.0), std::f64::consts::PI);
    }

    #[test]
    fn rotate() {
        let (x, y) = rotate_point(10.0, 0.0, 90.0);
        assert!(x.abs() < 1e-12);
        assert!((y - 10.0).abs() < 1e-12);
    }
}
