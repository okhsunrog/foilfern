//! Unit conversion between the tool DTOs (millimeters and degrees as JSON
//! numbers) and the core types (integer nanometers and microdegrees).
//!
//! Inputs are converted with [`Length::from_mm()`] / [`Angle::from_deg()`]
//! (rounded to the nearest nanometer / microdegree, range checked);
//! outputs are the exact quotient as `f64`, which serializes as the
//! shortest decimal representation (`2540000` nm → `2.54`).

use librepcb_core::types::{Angle, Length, Point};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{ToolError, ToolResult};

/// A point in millimeters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PointMm {
    /// X coordinate in millimeters.
    pub x: f64,
    /// Y coordinate in millimeters (upwards).
    pub y: f64,
}

impl PointMm {
    /// Converts to a core point (exact to the nanometer).
    pub fn to_point(self) -> ToolResult<Point> {
        Ok(Point::new(length(self.x)?, length(self.y)?))
    }
}

impl From<Point> for PointMm {
    fn from(p: Point) -> Self {
        Self {
            x: mm(p.x),
            y: mm(p.y),
        }
    }
}

/// Converts a length to millimeters.
pub fn mm(length: impl Into<Length>) -> f64 {
    length.into().to_nm() as f64 / 1e6
}

/// Converts an angle to degrees.
pub fn deg(angle: Angle) -> f64 {
    f64::from(angle.to_micro_deg()) / 1e6
}

/// Converts millimeters to a length.
pub fn length(millimeters: f64) -> ToolResult<Length> {
    if !millimeters.is_finite() {
        return Err(ToolError::invalid(format!(
            "invalid length: {millimeters} mm"
        )));
    }
    Ok(Length::from_mm(millimeters)?)
}

/// Converts degrees to an angle.
pub fn angle(degrees: f64) -> ToolResult<Angle> {
    if !degrees.is_finite() {
        return Err(ToolError::invalid(format!("invalid angle: {degrees}°")));
    }
    Ok(Angle::from_deg(degrees)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_conversion() {
        assert_eq!(length(2.54).unwrap().to_nm(), 2_540_000);
        assert_eq!(length(-0.1).unwrap().to_nm(), -100_000);
        assert_eq!(mm(Length::new(2_540_000)), 2.54);
        assert_eq!(
            serde_json::to_string(&mm(Length::new(100_000))).unwrap(),
            "0.1"
        );
        assert_eq!(angle(90.0).unwrap().to_micro_deg(), 90_000_000);
        assert_eq!(deg(Angle::new(45_500_000)), 45.5);
        assert!(length(f64::NAN).is_err());
    }
}
