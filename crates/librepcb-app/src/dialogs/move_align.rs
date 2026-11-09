//! The "Move/Align Elements" dialog.
//!
//! Port of libs/librepcb/editor/dialogs/movealigndialog.{ui,cpp}: moves
//! the selected elements to an absolute or relative reference position,
//! centers them around the axes and distributes them with a constant pitch.
//! Upstream uses it in the package editor (`PackageEditorState_Select`);
//! the dialog returns the new positions to the tab which opened it
//! ([`TabDialogResult::Positions`]), in the order of the given positions.
//!
//! Difference to upstream: the positions are applied when the dialog is
//! accepted (upstream moves the elements live while editing).

use librepcb_core::types::{Length, LengthUnit, Point};
use librepcb_i18n::tr;

use super::{Applied, DialogContext, DialogOptions, FieldEvent, Form, FormDialog, TabDialogResult};

const MAD: &str = "librepcb::editor::MoveAlignDialog";

/// The move/align dialog (upstream `MoveAlignDialog`).
pub struct MoveAlignDialog {
    form: Form,
    positions: Vec<Point>,
    /// The distinct positions in the order they are distributed.
    ordered: Vec<Point>,
    /// Whether the reference position is relative.
    relative: bool,
}

/// Upstream `calcCenter()`.
fn center(points: &[Point]) -> Point {
    let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)) = (
        points.iter().map(|p| p.x).min(),
        points.iter().map(|p| p.x).max(),
        points.iter().map(|p| p.y).min(),
        points.iter().map(|p| p.y).max(),
    ) else {
        return Point::new(Length::new(0), Length::new(0));
    };
    Point::new((min_x + max_x) / 2, (min_y + max_y) / 2)
}

impl MoveAlignDialog {
    /// Opens the dialog for the positions of the selected elements.
    pub fn new(positions: Vec<Point>, unit: LengthUnit) -> Self {
        // Spread in X and Y direction.
        let spread = |f: fn(&Point) -> Length| {
            let (min, max) = (positions.iter().map(f).min(), positions.iter().map(f).max());
            match (min, max) {
                (Some(min), Some(max)) if positions.len() >= 2 => max - min,
                _ => Length::new(0),
            }
        };
        let (spread_x, spread_y) = (spread(|p| p.x), spread(|p| p.y));
        let mut ordered: Vec<Point> = Vec::new();
        for p in &positions {
            if !ordered.contains(p) {
                ordered.push(*p);
            }
        }
        if spread_x > spread_y {
            // Horizontal: X ascending, then Y descending.
            ordered.sort_by(|a, b| a.x.cmp(&b.x).then(b.y.cmp(&a.y)));
        } else {
            // Vertical: Y descending, then X ascending.
            ordered.sort_by(|a, b| b.y.cmp(&a.y).then(a.x.cmp(&b.x)));
        }
        let reference = ordered
            .first()
            .copied()
            .unwrap_or(Point::new(Length::new(0), Length::new(0)));
        let steps: Vec<Point> = ordered
            .windows(2)
            .map(|w| Point::new(w[1].x - w[0].x, w[1].y - w[0].y))
            .collect();
        let default_interval = steps
            .first()
            .map_or(Point::new(Length::new(0), Length::new(0)), |s| {
                Point::new(s.x, -s.y)
            });
        let constant = |f: fn(&Point) -> Length| {
            let mut values: Vec<Length> = steps.iter().map(f).collect();
            values.sort();
            values.dedup();
            values.len() == 1
        };
        let single = positions.len() == 1;

        let mut form = Form::new(unit);
        form.header(tr!(MAD, "Reference Position (Top Left)"));
        // Upstream: relative mode by default for a single element.
        form.radio(
            "mode",
            tr!(MAD, "Mode:"),
            &[tr!(MAD, "Absolute"), tr!(MAD, "Relative")],
            usize::from(single),
        );
        let (x, y) = if single {
            (Length::new(0), Length::new(0))
        } else {
            (reference.x, reference.y)
        };
        form.length("x", tr!(MAD, "X:"), x, Length::MIN);
        form.checkbox("center_h", "", tr!(MAD, "Center around Y-axis"), false);
        form.length("y", tr!(MAD, "Y:"), y, Length::MIN);
        form.checkbox("center_v", "", tr!(MAD, "Center around X-axis"), false);
        form.set_enabled("center_h", !single);
        form.set_enabled("center_v", !single);
        form.header(tr!(MAD, "Pitch"));
        form.checkbox("interval_x_on", "", tr!(MAD, "ΔX:"), constant(|p| p.x));
        form.length("interval_x", "", default_interval.x, Length::MIN);
        form.button("align_vertically", "", tr!(MAD, "Align vertically (ΔX=0)"));
        form.checkbox("interval_y_on", "", tr!(MAD, "ΔY:"), constant(|p| p.y));
        form.length("interval_y", "", default_interval.y, Length::MIN);
        form.button(
            "align_horizontally",
            "",
            tr!(MAD, "Align horizontally (ΔY=0)"),
        );
        let dialog = Self {
            form,
            positions,
            ordered,
            relative: single,
        };
        dialog.update_enabled();
        dialog
    }

    fn update_enabled(&self) {
        let f = &self.form;
        let pitch = self.positions.len() >= 2;
        for id in ["interval_x_on", "interval_y_on"] {
            f.set_enabled(id, pitch);
        }
        f.set_enabled("x", !f.get_checked("center_h"));
        f.set_enabled("y", !f.get_checked("center_v"));
        f.set_enabled("interval_x", pitch && f.get_checked("interval_x_on"));
        f.set_enabled("interval_y", pitch && f.get_checked("interval_y_on"));
        f.set_enabled(
            "align_vertically",
            pitch
                && (!f.get_checked("interval_x_on")
                    || f.get_length("interval_x") != Length::new(0)),
        );
        f.set_enabled(
            "align_horizontally",
            pitch
                && (!f.get_checked("interval_y_on")
                    || f.get_length("interval_y") != Length::new(0)),
        );
    }

    fn reference(&self) -> Point {
        self.ordered
            .first()
            .copied()
            .unwrap_or(Point::new(Length::new(0), Length::new(0)))
    }

    /// The new positions (upstream `updateNewPositions()`), in the order
    /// of the positions passed to [`Self::new()`].
    pub fn new_positions(&self) -> Vec<Point> {
        let f = &self.form;
        let first_old = self.reference();
        let mut first_new = Point::new(f.get_length("x"), f.get_length("y"));
        if f.get_index("mode") == Some(1) {
            first_new = Point::new(first_new.x + first_old.x, first_new.y + first_old.y);
        }
        let delta = Point::new(first_new.x - first_old.x, first_new.y - first_old.y);
        let (interval_x, interval_y) = (
            f.get_checked("interval_x_on") && self.positions.len() >= 2,
            f.get_checked("interval_y_on") && self.positions.len() >= 2,
        );
        let (dx, dy) = (f.get_length("interval_x"), f.get_length("interval_y"));
        let mut positions: Vec<Point> = self
            .positions
            .iter()
            .map(|p| {
                let index = self.ordered.iter().position(|o| o == p).unwrap_or(0) as i64;
                let mut n = Point::new(p.x + delta.x, p.y + delta.y);
                if interval_x {
                    n.x = first_new.x + dx * index;
                }
                if interval_y {
                    n.y = first_new.y - dy * index;
                }
                n
            })
            .collect();
        if !positions.is_empty() {
            let c = center(&positions);
            let offset = Point::new(
                if f.get_checked("center_h") {
                    c.x
                } else {
                    Length::new(0)
                },
                if f.get_checked("center_v") {
                    c.y
                } else {
                    Length::new(0)
                },
            );
            for p in &mut positions {
                *p = Point::new(p.x - offset.x, p.y - offset.y);
            }
        }
        positions
    }
}

impl FormDialog for MoveAlignDialog {
    fn title(&self) -> String {
        tr!(MAD, "Move/Align Elements")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            width: 420.0,
            label_width: 80.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, event: FieldEvent) {
        let reference = self.reference();
        match (id, event) {
            ("mode", FieldEvent::Edited) => {
                // Convert the reference position between absolute and
                // relative coordinates (the radio buttons' `toggled`).
                let relative = self.form.get_index("mode") == Some(1);
                if relative == self.relative {
                    return;
                }
                self.relative = relative;
                let (x, y) = (self.form.get_length("x"), self.form.get_length("y"));
                let (x, y) = if relative {
                    (x - reference.x, y - reference.y)
                } else {
                    (x + reference.x, y + reference.y)
                };
                self.form.set_length("x", x);
                self.form.set_length("y", y);
            }
            ("align_vertically", FieldEvent::Clicked) => {
                self.form.set_length("interval_x", Length::new(0));
                self.form.set_checked("interval_x_on", true);
            }
            ("align_horizontally", FieldEvent::Clicked) => {
                self.form.set_length("interval_y", Length::new(0));
                self.form.set_checked("interval_y_on", true);
            }
            _ => {}
        }
        self.update_enabled();
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        Ok(Applied::Tab(TabDialogResult::Positions(
            self.new_positions(),
        )))
    }
}
