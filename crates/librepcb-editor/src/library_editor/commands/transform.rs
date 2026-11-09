//! Geometric transformations of the objects of symbols and footprints,
//! with the exact semantics of upstream's edit commands (port of the
//! `translate()`, `rotate()`, `mirror*()` and `snapToGrid()` methods of
//! libs/librepcb/editor/cmd/{cmdpolygonedit,cmdcircleedit,cmdtextedit,
//! cmdstroketextedit,cmdholeedit,cmdzoneedit,cmdimageedit}.cpp and
//! libs/librepcb/editor/library/cmd/{cmdsymbolpinedit,cmdfootprintpadedit}.cpp).
//!
//! Note that upstream is not consistent in which mirror orientation turns
//! the rotation into `180° - rotation`; this is ported as is.

use librepcb_core::geometry::ComponentSide;
use librepcb_core::geometry::{
    Circle, Hole, Image, NonEmptyPath, Polygon, StrokeText, Text, Zone, ZoneLayers,
};
use librepcb_core::library::pkg::FootprintPad;
use librepcb_core::library::sym::SymbolPin;
use librepcb_core::types::{Angle, Length, Orientation, Point, PositiveLength};

/// Transformations of an object (upstream `Cmd*Edit` methods).
pub trait Transformable {
    /// The reference position of the object (used to compute the center
    /// of a selection and the positions of "move/align").
    fn positions(&self) -> Vec<Point>;
    /// Whether all reference positions are on the grid.
    fn is_on_grid(&self, grid: PositiveLength) -> bool {
        self.positions().iter().all(|p| p.is_on_grid(grid))
    }
    /// Moves the object by `delta`.
    fn translate(&mut self, delta: Point);
    /// Rotates the object around `center`.
    fn rotate(&mut self, angle: Angle, center: Point);
    /// Mirrors the geometry of the object at `center`.
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point);
    /// Moves the object to the other board side (no-op for schematic
    /// objects).
    fn mirror_layer(&mut self) {}
    /// Snaps the object to the grid.
    fn snap_to_grid(&mut self, grid: PositiveLength);
}

/// Returns `180° - rotation` if `orientation == flip`, else `-rotation`.
fn mirrored_rotation(rotation: Angle, orientation: Orientation, flip: Orientation) -> Angle {
    if orientation == flip {
        Angle::DEG180 - rotation
    } else {
        -rotation
    }
}

impl Transformable for Polygon {
    fn positions(&self) -> Vec<Point> {
        self.path().vertices().iter().map(|v| v.pos).collect()
    }
    fn translate(&mut self, delta: Point) {
        self.set_path(self.path().translated(delta));
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        self.set_path(self.path().rotated(angle, center));
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        self.set_path(self.path().mirrored(orientation, center));
    }
    fn mirror_layer(&mut self) {
        self.set_layer(self.layer().mirrored(None));
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        self.set_path(self.path().mapped_to_grid(grid));
    }
}

impl Transformable for Circle {
    fn positions(&self) -> Vec<Point> {
        vec![self.center()]
    }
    fn translate(&mut self, delta: Point) {
        self.set_center(self.center() + delta);
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        self.set_center(self.center().rotated(angle, center));
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        self.set_center(self.center().mirrored(orientation, center));
    }
    fn mirror_layer(&mut self) {
        self.set_layer(self.layer().mirrored(None));
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        self.set_center(self.center().mapped_to_grid(grid));
    }
}

impl Transformable for Text {
    fn positions(&self) -> Vec<Point> {
        vec![self.position()]
    }
    fn translate(&mut self, delta: Point) {
        self.set_position(self.position() + delta);
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        self.set_position(self.position().rotated(angle, center));
        self.set_rotation(self.rotation() + angle);
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        // Upstream CmdTextEdit::mirror(Qt::Orientation, ...).
        self.set_position(self.position().mirrored(orientation, center));
        self.set_rotation(mirrored_rotation(
            self.rotation(),
            orientation,
            Orientation::Horizontal,
        ));
        self.set_align(self.align().mirrored_v());
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        self.set_position(self.position().mapped_to_grid(grid));
    }
}

impl Transformable for StrokeText {
    fn positions(&self) -> Vec<Point> {
        vec![self.position()]
    }
    fn translate(&mut self, delta: Point) {
        self.set_position(self.position() + delta);
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        self.set_position(self.position().rotated(angle, center));
        self.set_rotation(self.rotation() + angle);
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        self.set_position(self.position().mirrored(orientation, center));
        self.set_rotation(mirrored_rotation(
            self.rotation(),
            orientation,
            Orientation::Vertical,
        ));
        self.set_align(self.align().mirrored_h());
    }
    fn mirror_layer(&mut self) {
        self.set_layer(self.layer().mirrored(None));
        self.set_mirrored(!self.mirrored());
        self.set_align(self.align().mirrored_h());
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        self.set_position(self.position().mapped_to_grid(grid));
    }
}

impl Transformable for SymbolPin {
    fn positions(&self) -> Vec<Point> {
        vec![self.position()]
    }
    fn translate(&mut self, delta: Point) {
        self.set_position(self.position() + delta);
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        self.set_position(self.position().rotated(angle, center));
        self.set_rotation(self.rotation() + angle);
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        self.set_position(self.position().mirrored(orientation, center));
        self.set_rotation(mirrored_rotation(
            self.rotation(),
            orientation,
            Orientation::Horizontal,
        ));
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        self.set_position(self.position().mapped_to_grid(grid));
    }
}

impl Transformable for FootprintPad {
    fn positions(&self) -> Vec<Point> {
        vec![self.pad().position()]
    }
    fn translate(&mut self, delta: Point) {
        let pad = self.pad_mut();
        pad.set_position(pad.position() + delta);
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        let pad = self.pad_mut();
        pad.set_position(pad.position().rotated(angle, center));
        pad.set_rotation(pad.rotation() + angle);
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        let pad = self.pad_mut();
        pad.set_position(pad.position().mirrored(orientation, center));
        pad.set_rotation(mirrored_rotation(
            pad.rotation(),
            orientation,
            Orientation::Horizontal,
        ));
        let outline = pad
            .custom_shape_outline()
            .mirrored(orientation, Point::ORIGIN);
        pad.set_custom_shape_outline(outline);
    }
    fn mirror_layer(&mut self) {
        let pad = self.pad_mut();
        pad.set_component_side(match pad.component_side() {
            ComponentSide::Top => ComponentSide::Bottom,
            ComponentSide::Bottom => ComponentSide::Top,
        });
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        let pad = self.pad_mut();
        pad.set_position(pad.position().mapped_to_grid(grid));
    }
}

impl Transformable for Zone {
    fn positions(&self) -> Vec<Point> {
        self.outline().vertices().iter().map(|v| v.pos).collect()
    }
    fn translate(&mut self, delta: Point) {
        self.set_outline(self.outline().translated(delta));
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        self.set_outline(self.outline().rotated(angle, center));
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        self.set_outline(self.outline().mirrored(orientation, center));
    }
    fn mirror_layer(&mut self) {
        let layers = self.layers();
        let mut tmp = layers;
        tmp.set(ZoneLayers::TOP, layers.contains(ZoneLayers::BOTTOM));
        tmp.set(ZoneLayers::BOTTOM, layers.contains(ZoneLayers::TOP));
        self.set_layers(tmp);
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        self.set_outline(self.outline().mapped_to_grid(grid));
    }
}

/// Replaces the path of a hole (the transformed path is never empty).
fn set_hole_path(hole: &mut Hole, path: librepcb_core::geometry::Path) {
    if let Ok(path) = NonEmptyPath::new(path) {
        hole.set_path(path);
    }
}

impl Transformable for Hole {
    fn positions(&self) -> Vec<Point> {
        vec![self.path().first().pos]
    }
    fn translate(&mut self, delta: Point) {
        let path = self.path().get().translated(delta);
        set_hole_path(self, path);
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        let path = self.path().get().rotated(angle, center);
        set_hole_path(self, path);
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        let path = self.path().get().mirrored(orientation, center);
        set_hole_path(self, path);
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        let p0 = self.path().first().pos;
        let p1 = p0.mapped_to_grid(grid);
        self.translate(p1 - p0);
    }
}

impl Transformable for Image {
    fn positions(&self) -> Vec<Point> {
        vec![self.position()]
    }
    fn translate(&mut self, delta: Point) {
        self.set_position(self.position() + delta);
    }
    fn rotate(&mut self, angle: Angle, center: Point) {
        self.set_position(self.position().rotated(angle, center));
        self.set_rotation(self.rotation() + angle);
    }
    fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
        // Images cannot be mirrored: move the origin to the other side.
        let offset = match orientation {
            Orientation::Horizontal => Point::new(-*self.width(), Length::ZERO),
            Orientation::Vertical => Point::new(Length::ZERO, -*self.height()),
        };
        self.set_position(
            self.position().mirrored(orientation, center)
                + offset.rotated(-self.rotation(), Point::ORIGIN),
        );
        self.set_rotation(-self.rotation());
    }
    fn snap_to_grid(&mut self, grid: PositiveLength) {
        self.set_position(self.position().mapped_to_grid(grid));
    }
}

/// Delegates [`Transformable`] to the object of an enum variant.
macro_rules! delegate_transformable {
    ($ty:ty, $($variant:ident),+) => {
        impl Transformable for $ty {
            fn positions(&self) -> Vec<Point> {
                match self { $(Self::$variant(o) => o.positions()),+ }
            }
            fn translate(&mut self, delta: Point) {
                match self { $(Self::$variant(o) => o.translate(delta)),+ }
            }
            fn rotate(&mut self, angle: Angle, center: Point) {
                match self { $(Self::$variant(o) => o.rotate(angle, center)),+ }
            }
            fn mirror_geometry(&mut self, orientation: Orientation, center: Point) {
                match self { $(Self::$variant(o) => o.mirror_geometry(orientation, center)),+ }
            }
            fn mirror_layer(&mut self) {
                match self { $(Self::$variant(o) => o.mirror_layer()),+ }
            }
            fn snap_to_grid(&mut self, grid: PositiveLength) {
                match self { $(Self::$variant(o) => o.snap_to_grid(grid)),+ }
            }
        }
    };
}

delegate_transformable!(super::SymbolObject, Pin, Polygon, Circle, Text, Image);
delegate_transformable!(
    super::FootprintObject,
    Pad,
    Polygon,
    Circle,
    StrokeText,
    Zone,
    Hole
);
