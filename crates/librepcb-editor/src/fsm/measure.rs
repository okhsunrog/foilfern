//! Port of libs/librepcb/editor/utils/measuretool.{h,cpp} and the snap
//! helpers of libs/librepcb/editor/editorcommandset/editortoolbox
//! (`snapCandidatesFromPath()`, `snapCandidatesFromCircle()`,
//! `snapPosition()`), shared by the schematic and board editor FSMs.
//!
//! The snap candidates are computed by the caller (the editor state knows
//! its page or board); the tool keeps the measurement and writes the ruler,
//! scene cursor, info box and status bar message into a [`ViewState`].
//!
//! Differences to upstream: the info box is plain text (upstream HTML with
//! `&nbsp;`), the angle is computed with `libm::atan2`; the copied value
//! is returned to the caller for the clipboard (text).

use std::collections::BTreeSet;

use librepcb_core::geometry::Path;
use librepcb_core::types::{Angle, Length, LengthUnit, Point, PositiveLength};
use librepcb_i18n::tr;

use super::{Modifiers, SceneCursor, ViewState};

/// Snap candidates of a path: vertices and, for 180° arcs, the center and
/// the middle of the arc (upstream `snapCandidatesFromPath()`).
pub fn snap_candidates_from_path(path: &Path) -> BTreeSet<Point> {
    let mut candidates = BTreeSet::new();
    let vertices = path.vertices();
    for (i, v) in vertices.iter().enumerate() {
        candidates.insert(v.pos);
        if v.angle.abs() == Angle::DEG180
            && let Some(next) = vertices.get(i + 1)
        {
            let center = (v.pos + next.pos) / 2;
            let middle = v.pos.rotated(v.angle / 2, center);
            candidates.insert(center);
            candidates.insert(middle);
        }
    }
    candidates
}

/// Snap candidates of a circle: center and the four extreme points
/// (upstream `snapCandidatesFromCircle()`).
pub fn snap_candidates_from_circle(center: Point, diameter: PositiveLength) -> BTreeSet<Point> {
    let r = diameter.get() / 2;
    [
        center,
        center + Point::new(Length::ZERO, r),
        center + Point::new(Length::ZERO, -r),
        center + Point::new(r, Length::ZERO),
        center + Point::new(-r, Length::ZERO),
    ]
    .into_iter()
    .collect()
}

/// Snaps the cursor to the nearest candidate if it is not farther away
/// than the nearest grid point, else to the grid (upstream
/// `snapPosition()`). Returns the position and whether it snapped to a
/// candidate.
pub fn snap_position(
    cursor: Point,
    grid: PositiveLength,
    candidates: &BTreeSet<Point>,
) -> (Point, bool) {
    if candidates.contains(&cursor) {
        return (cursor, true);
    }
    if cursor.is_on_grid(grid) {
        return (cursor, false);
    }
    let nearest = candidates
        .iter()
        .map(|c| (*(cursor - *c).length(), *c))
        .min_by_key(|(d, _)| *d);
    let on_grid = cursor.mapped_to_grid(grid);
    let grid_distance = *(cursor - on_grid).length();
    match nearest {
        Some((d, c)) if d <= grid_distance => (c, true),
        _ => (on_grid, false),
    }
}

/// The measure tool (upstream `MeasureTool`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeasureTool {
    candidates: BTreeSet<Point>,
    grid: Option<PositiveLength>,
    unit: LengthUnit,
    last_pos: Point,
    cursor_pos: Point,
    cursor_snapped: bool,
    start: Option<Point>,
    end: Option<Point>,
}

impl MeasureTool {
    /// Creates the tool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the snap candidates (upstream `setSchematic()`,
    /// `setBoard()`, ...).
    pub fn set_snap_candidates(&mut self, candidates: BTreeSet<Point>) {
        self.candidates = candidates;
    }

    /// The measured start and end points.
    pub fn measurement(&self) -> (Option<Point>, Option<Point>) {
        (self.start, self.end)
    }

    /// Enters the tool (upstream `enter()`).
    pub fn enter(
        &mut self,
        view: &mut ViewState,
        grid: PositiveLength,
        unit: LengthUnit,
        pos: Point,
    ) {
        self.grid = Some(grid);
        self.unit = unit;
        self.last_pos = pos;
        view.gray_out = true;
        self.update_cursor_position(view, Modifiers::NONE);
        self.update_status_bar_message(view);
    }

    /// Leaves the tool (upstream `leave()`); the measurement is kept for
    /// the next time.
    pub fn leave(&mut self, view: &mut ViewState) {
        view.scene_cursor = None;
        view.ruler = None;
        view.gray_out = false;
        view.info_box.clear();
        view.set_status(String::new(), None);
    }

    /// Shift pressed or released (snapping off/on).
    pub fn modifiers_changed(&mut self, view: &mut ViewState, modifiers: Modifiers) {
        self.update_cursor_position(view, modifiers);
    }

    /// The pointer moved.
    pub fn pointer_moved(&mut self, view: &mut ViewState, pos: Point, modifiers: Modifiers) {
        self.last_pos = pos;
        self.update_cursor_position(view, modifiers);
    }

    /// Left click: sets the start or the end point.
    pub fn left_pressed(&mut self, view: &mut ViewState) {
        if self.start.is_none() || self.end.is_some() {
            self.start = Some(self.cursor_pos);
            self.end = None;
        } else {
            self.end = Some(self.cursor_pos);
        }
        self.update_ruler_positions(view);
        self.update_status_bar_message(view);
    }

    /// Copy: returns the measured distance in the current unit as text
    /// (for the clipboard), if a measurement exists.
    pub fn copy(&mut self, view: &mut ViewState) -> Option<String> {
        let (start, end) = (self.start?, self.end?);
        let value = self.unit.convert_to_unit(*(end - start).length());
        let text = format_value(value, 12);
        view.set_status(
            tr!(
                "librepcb::editor::MeasureTool",
                "Copied to clipboard: {0}",
                text.clone()
            ),
            Some(3000),
        );
        Some(text)
    }

    /// Remove: clears a complete measurement.
    pub fn remove(&mut self, view: &mut ViewState) -> bool {
        if self.start.is_some() && self.end.is_some() {
            self.start = None;
            self.end = None;
            self.update_ruler_positions(view);
            self.update_status_bar_message(view);
            return true;
        }
        false
    }

    /// Abort: clears a started measurement.
    pub fn abort(&mut self, view: &mut ViewState) -> bool {
        if self.start.is_some() && self.end.is_none() {
            self.start = None;
            self.update_ruler_positions(view);
            self.update_status_bar_message(view);
            return true;
        }
        false
    }

    fn update_cursor_position(&mut self, view: &mut ViewState, modifiers: Modifiers) {
        self.cursor_pos = self.last_pos;
        self.cursor_snapped = false;
        if !modifiers.shift
            && let Some(grid) = self.grid
        {
            (self.cursor_pos, self.cursor_snapped) =
                snap_position(self.cursor_pos, grid, &self.candidates);
        }
        self.update_ruler_positions(view);
    }

    fn update_ruler_positions(&mut self, view: &mut ViewState) {
        view.scene_cursor = Some(SceneCursor {
            pos: self.cursor_pos,
            cross: self.start.is_none() || self.end.is_some(),
            circle: self.cursor_snapped,
        });
        let start = self.start.unwrap_or(self.cursor_pos);
        let end = self.end.unwrap_or(self.cursor_pos);
        view.ruler = self.start.map(|_| (start, end));
        let diff = end - start;
        let length = *diff.length();
        let (dx, dy) = diff.to_mm();
        let angle = libm::atan2(dy, dx).to_degrees();
        let decimals = self.unit.reasonable_number_of_decimals() + 1;
        let unit = self.unit.to_short_str();
        let v = |l: Length| self.unit.convert_to_unit(l);
        let mut lines = vec![
            format!("X0: {:>10.decimals$} {unit}", v(start.x)),
            format!("Y0: {:>10.decimals$} {unit}", v(start.y)),
            format!("X1: {:>10.decimals$} {unit}", v(end.x)),
            format!("Y1: {:>10.decimals$} {unit}", v(end.y)),
            String::new(),
            format!("ΔX: {:>10.decimals$} {unit}", v(diff.x)),
            format!("ΔY: {:>10.decimals$} {unit}", v(diff.y)),
            String::new(),
        ];
        lines.push(format!("Δ: {:>11.decimals$} {unit}", v(length)));
        let width = 14usize.saturating_sub(decimals);
        lines.push(format!("∠: {angle:>width$.3}°"));
        view.info_box = lines.join("\n");
    }

    fn update_status_bar_message(&self, view: &mut ViewState) {
        let note = format!(
            " {}",
            tr!(
                "librepcb::editor::MeasureTool",
                "(press {0} to disable snap)",
                "Shift"
            )
        );
        let text = if self.end.is_some() {
            tr!(
                "librepcb::editor::MeasureTool",
                "Press {0} to copy the value to clipboard or {1} to clear the measurement",
                "Ctrl+C",
                "Del"
            )
        } else if self.start.is_some() {
            tr!(
                "librepcb::editor::MeasureTool",
                "Click to specify the end point"
            ) + &note
        } else {
            tr!(
                "librepcb::editor::MeasureTool",
                "Click to specify the start point"
            ) + &note
        };
        view.set_status(text, None);
    }
}

/// Formats a value with at most `precision` significant decimals, without
/// trailing zeros (upstream `Toolbox::floatToString()`).
fn format_value(value: f64, precision: usize) -> String {
    let s = format!("{value:.precision$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".to_owned() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mm(x: f64, y: f64) -> Point {
        Point::from_mm(x, y).unwrap()
    }

    #[test]
    fn snap_prefers_nearer_candidate() {
        let grid = PositiveLength::new(Length::new(2_540_000)).unwrap();
        let candidates: BTreeSet<Point> = [mm(1.0, 1.0)].into_iter().collect();
        assert_eq!(
            snap_position(mm(1.1, 1.0), grid, &candidates),
            (mm(1.0, 1.0), true)
        );
        assert_eq!(
            snap_position(mm(2.5, 0.1), grid, &candidates),
            (mm(2.54, 0.0), false)
        );
    }

    #[test]
    fn measure_start_end() {
        let grid = PositiveLength::new(Length::new(1_000_000)).unwrap();
        let mut tool = MeasureTool::new();
        let mut view = ViewState::default();
        tool.enter(&mut view, grid, LengthUnit::Millimeters, mm(0.0, 0.0));
        tool.left_pressed(&mut view);
        tool.pointer_moved(&mut view, mm(3.1, 3.9), Modifiers::NONE);
        tool.left_pressed(&mut view);
        assert_eq!(tool.measurement(), (Some(mm(0.0, 0.0)), Some(mm(3.0, 4.0))));
        assert_eq!(tool.copy(&mut view).as_deref(), Some("5"));
        assert!(view.info_box.contains("5.0000 mm"));
        assert!(tool.remove(&mut view));
        assert_eq!(view.ruler, None);
    }
}
