//! The backend of the `LengthEdit` tool bar widget.
//!
//! Port of libs/librepcb/editor/utils/lengtheditcontext.{h,cpp}: holds the
//! value, unit and minimum of a length edit and computes the step up/down
//! values from predefined steps (the up/down buttons and the `+`/`-`
//! shortcuts set `increase`/`decrease` in the UI data, the backend applies
//! the step and resets them).
//!
//! Not ported: storing the chosen unit in the user settings
//! (`QSettings`); the unit is kept per tab.

use librepcb_app_ui as ui;
use librepcb_core::types::{Length, LengthUnit};

use crate::helpers::{length_from_ui, length_to_ui, unit_from_ui, unit_to_ui};

/// Predefined steps (upstream `LengthEditContext::Steps`).
pub mod steps {
    use librepcb_core::types::Length;

    /// Generic lengths.
    pub const GENERIC: &[Length] = &[
        Length::new(10_000),
        Length::new(25_400),
        Length::new(100_000),
        Length::new(254_000),
        Length::new(1_000_000),
        Length::new(2_540_000),
    ];

    /// Text heights.
    pub const TEXT_HEIGHT: &[Length] = &[
        Length::new(100_000),
        Length::new(254_000),
        Length::new(500_000),
    ];

    /// Drill diameters.
    pub const DRILL_DIAMETER: &[Length] = &[Length::new(254_000), Length::new(100_000)];

    /// Symbol pin lengths.
    pub const PIN_LENGTH: &[Length] = &[Length::new(2_500_000), Length::new(2_540_000)];
}

/// State of a length edit (upstream `LengthEditContext` with
/// `StepBehavior::PredefinedSteps`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LengthEdit {
    minimum: Length,
    steps: &'static [Length],
    unit: LengthUnit,
    value: Length,
    step_up: Length,
    step_down: Length,
}

impl Default for LengthEdit {
    fn default() -> Self {
        Self::new(steps::GENERIC)
    }
}

impl LengthEdit {
    /// A length edit with value 0 and no minimum.
    pub fn new(steps: &'static [Length]) -> Self {
        Self {
            minimum: Length::MIN,
            steps,
            unit: LengthUnit::Millimeters,
            value: Length::new(0),
            step_up: Length::new(0),
            step_down: Length::new(0),
        }
    }

    /// Configures the edit for a value (upstream `configure()`); the
    /// minimum is 0 for unsigned and 1 nm for positive values.
    pub fn configure(&mut self, value: Length, minimum: Length, steps: &'static [Length]) {
        self.minimum = Length::MIN;
        self.set_value(value);
        self.steps = steps;
        self.update_steps();
        self.minimum = minimum;
    }

    /// The value.
    pub fn value(&self) -> Length {
        self.value
    }

    /// The unit.
    pub fn unit(&self) -> LengthUnit {
        self.unit
    }

    /// Sets the value (ignored if below the minimum).
    pub fn set_value(&mut self, value: Length) {
        if value != self.value && value >= self.minimum {
            self.value = value;
            self.update_steps();
        }
    }

    /// The UI data (upstream `getUiData()`).
    pub fn ui_data(&self) -> ui::LengthEditData {
        ui::LengthEditData {
            value: length_to_ui(self.value),
            unit: unit_to_ui(self.unit),
            minimum: length_to_ui(self.minimum),
            can_increase: self.step_up > Length::new(0),
            can_decrease: self.step_down > Length::new(0) && self.value > self.minimum,
            increase: false,
            decrease: false,
        }
    }

    /// Applies UI data (upstream `setUiData()`); returns the new value if
    /// it changed.
    pub fn set_ui_data(&mut self, data: &ui::LengthEditData) -> Option<Length> {
        let old = self.value;
        self.set_value(length_from_ui(data.value.clone()));
        self.unit = unit_from_ui(data.unit);
        if data.increase && self.step_up > Length::new(0) {
            self.set_value(Length::new(
                self.value.to_nm().saturating_add(self.step_up.to_nm()),
            ));
        } else if data.decrease && self.step_down > Length::new(0) {
            self.set_value(Length::new(
                self.value.to_nm().saturating_sub(self.step_down.to_nm()),
            ));
        }
        (self.value != old).then_some(self.value)
    }

    fn update_steps(&mut self) {
        if self.value == Length::new(0) || self.value == self.minimum {
            return; // Keep the last step values.
        }
        let mut up = Length::new(0);
        let mut down = Length::new(0);
        for step in self.steps {
            if self.value.to_nm() % step.to_nm() == 0 {
                up = *step;
                if self.value.abs() > *step || down == Length::new(0) {
                    down = *step;
                }
            }
        }
        if self.value < Length::new(0) {
            std::mem::swap(&mut up, &mut down);
        }
        if down > Length::new(0)
            && self.value.to_nm() < self.minimum.to_nm().saturating_add(down.to_nm())
        {
            down = Length::new(0);
        }
        self.step_up = up;
        self.step_down = down;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predefined_steps() {
        let mut e = LengthEdit::default();
        e.configure(Length::new(300_000), Length::new(0), steps::GENERIC);
        let d = e.ui_data();
        assert!(d.can_increase && d.can_decrease);
        let mut d = e.ui_data();
        d.increase = true;
        assert_eq!(e.set_ui_data(&d), Some(Length::new(400_000)));
        let mut d = e.ui_data();
        d.decrease = true;
        assert_eq!(e.set_ui_data(&d), Some(Length::new(300_000)));
        // Positive edit: the step down to 0 is rejected (like upstream).
        e.configure(Length::new(10_000), Length::new(1), steps::GENERIC);
        let mut d = e.ui_data();
        d.decrease = true;
        assert_eq!(e.set_ui_data(&d), None);
        assert_eq!(e.value(), Length::new(10_000));
    }
}
