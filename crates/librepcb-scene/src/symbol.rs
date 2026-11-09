//! Scene builder of library symbols: port of the drawing semantics of
//! libs/librepcb/core/library/sym/symbolpainter.{h,cpp} (used by the
//! graphics export of symbols, e.g. `librepcb-cli open-symbol --export`).
//!
//! The canvas layers are the schematic color roles like in
//! [`SchematicScene`](crate::SchematicScene). Items are ordered like the
//! painter draws them: grab areas, filled shapes, images, other shapes,
//! texts, then the pins (line, name and the placeholder number `1…`).
//! Texts are drawn raw (e.g. `{{NAME}}`), like upstream. Differences to
//! upstream:
//!
//! - Texts are drawn with a stroke font (see [`crate::text`]).
//! - Images are not drawn yet, only their borders.
//!
//! Every item is mapped to its model object ([`SymbolSceneObject`]) for
//! hit testing and selection highlighting in the symbol editor; the scene
//! is rebuilt after each modification (symbols are small).

use librepcb_canvas::kurbo::{Affine, Rect, Shape};
use std::collections::HashMap;

use librepcb_canvas::{
    Item, ItemId, Layer as CanvasLayer, LayerId, Scene, SelectionMode, Style, convert,
};
use librepcb_core::font::StrokeFont;
use librepcb_core::library::sym::{Symbol, SymbolPin};
use librepcb_core::types::{Length, Point, Uuid};
use librepcb_core::utils::toolbox;

use crate::colors::ColorScheme;
use crate::schematic::{ROLES, SchematicScene};
use crate::shapes::{self, Fill};
use crate::text::{RenderedText, TextRenderer, TextStyle};

/// Draw order of the item kinds (upstream `SymbolPainter::paint()`).
mod z {
    pub const GRAB_AREAS: i32 = 0;
    pub const FILLED: i32 = 1;
    pub const IMAGES: i32 = 2;
    pub const SHAPES: i32 = 3;
    pub const TEXTS: i32 = 4;
    pub const PINS: i32 = 5;
    pub const PIN_TEXTS: i32 = 6;
}

/// Line width of symbol pins (upstream `drawSymbolPin()`).
const PIN_LINE_WIDTH: Length = Length::new(158_750);

/// The model object an item of a [`SymbolScene`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SymbolSceneObject {
    /// A pin (line, name and number texts).
    Pin(Uuid),
    /// A polygon.
    Polygon(Uuid),
    /// A circle.
    Circle(Uuid),
    /// A text.
    Text(Uuid),
    /// An image (its border).
    Image(Uuid),
}

/// A library symbol as a canvas [`Scene`].
#[derive(Debug)]
pub struct SymbolScene {
    scene: Scene,
    scheme: ColorScheme,
    objects: HashMap<ItemId, SymbolSceneObject>,
}

static_assertions::assert_impl_all!(SymbolScene: Send, Sync);

impl SymbolScene {
    /// Builds the scene of a symbol with a color scheme (e.g.
    /// [`ColorScheme::SCHEMATIC_LIGHT`]); texts are drawn with `font` (none
    /// without a font, see [`crate::default_stroke_font()`]).
    pub fn build(symbol: &Symbol, font: Option<&StrokeFont>, scheme: &ColorScheme) -> Self {
        let mut scene = Scene::new();
        add_schematic_layers(&mut scene, scheme);
        let mut builder = Builder {
            scene,
            scheme,
            text: TextRenderer::new(font),
            objects: HashMap::new(),
            current: None,
        };
        builder.add_shapes(symbol);
        builder.add_texts(symbol);
        builder.add_pins(symbol);
        Self {
            scene: builder.scene,
            scheme: scheme.clone(),
            objects: builder.objects,
        }
    }

    /// The model object of an item.
    pub fn object(&self, item: ItemId) -> Option<SymbolSceneObject> {
        self.objects.get(&item).copied()
    }

    /// The items of a model object (e.g. to highlight the selection).
    pub fn items_of(&self, object: SymbolSceneObject) -> Vec<ItemId> {
        let mut items: Vec<ItemId> = self
            .objects
            .iter()
            .filter(|(_, o)| **o == object)
            .map(|(i, _)| *i)
            .collect();
        items.sort();
        items
    }

    /// The objects with an item within `tolerance` of `pos`, topmost first
    /// (without duplicates).
    pub fn objects_at(&self, pos: Point, tolerance: Length) -> Vec<SymbolSceneObject> {
        let mut objects = Vec::new();
        for item in self.scene.items_at(convert::point(pos), tolerance.to_mm()) {
            if let Some(o) = self.object(item)
                && !objects.contains(&o)
            {
                objects.push(o);
            }
        }
        objects
    }

    /// The objects with an item intersecting the rectangle spanned by `p1`
    /// and `p2`.
    pub fn objects_in_rect(&self, p1: Point, p2: Point) -> Vec<SymbolSceneObject> {
        let rect = Rect::from_points(convert::point(p1), convert::point(p2));
        let mut objects: Vec<SymbolSceneObject> = self
            .scene
            .items_in_rect(rect, SelectionMode::Intersects)
            .into_iter()
            .filter_map(|i| self.object(i))
            .collect();
        objects.sort();
        objects.dedup();
        objects
    }

    /// The scene.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The scene for modification.
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// The background color of the color scheme.
    pub fn background(&self) -> librepcb_canvas::peniko::Color {
        self.scheme.color_or_transparent("schematic_background")
    }

    /// Bounding box of all visible items (world coordinates, mm).
    pub fn content_bounds(&self) -> Option<Rect> {
        crate::render::visible_bounds(&self.scene)
    }
}

/// Adds the canvas layers of the schematic color roles (hidden if the
/// scheme has no color for them).
pub(crate) fn add_schematic_layers(scene: &mut Scene, scheme: &ColorScheme) {
    let count = ROLES.len() as i32;
    for (i, role) in ROLES.iter().enumerate() {
        let color = scheme.color(role);
        scene.set_layer(
            LayerId(i as u16),
            CanvasLayer::new(color.unwrap_or(librepcb_canvas::peniko::Color::TRANSPARENT))
                .with_order(count - i as i32),
        );
    }
}

/// Upstream `getFillColor()` for grab areas of schematic shapes.
pub(crate) fn schematic_grab_area_fill(
    scheme: &ColorScheme,
    layer: librepcb_core::types::Layer,
) -> Fill {
    // Upstream `ColorRole::getGrabAreaRole()`.
    if layer.color_role() == "schematic_outlines" {
        scheme
            .color("schematic_grab_areas")
            .map_or(Fill::None, Fill::Solid)
    } else {
        Fill::None
    }
}

struct Builder<'a> {
    scene: Scene,
    scheme: &'a ColorScheme,
    text: TextRenderer<'a>,
    objects: HashMap<ItemId, SymbolSceneObject>,
    /// The object of the items inserted next.
    current: Option<SymbolSceneObject>,
}

impl Builder<'_> {
    fn insert(&mut self, item: Option<Item>) {
        if let Some(item) = item
            && !item.geometry.is_empty()
        {
            let id = self.scene.insert(item);
            if let Some(object) = self.current {
                self.objects.insert(id, object);
            }
        }
    }

    fn insert_text(&mut self, layer: LayerId, z: i32, rendered: Option<RenderedText>) {
        let item =
            rendered.map(|t| Item::new(layer, t.path, Style::stroke(t.stroke_width)).with_z(z));
        self.insert(item);
    }

    fn add_shapes(&mut self, symbol: &Symbol) {
        for polygon in symbol.polygons().iter() {
            self.current = Some(SymbolSceneObject::Polygon(polygon.uuid()));
            let Some(layer) = SchematicScene::layer_id(polygon.layer().color_role()) else {
                continue;
            };
            let (z, fill) = if polygon.is_filled() {
                (z::FILLED, Fill::Layer)
            } else if polygon.is_grab_area() {
                (
                    z::GRAB_AREAS,
                    schematic_grab_area_fill(self.scheme, polygon.layer()),
                )
            } else {
                (z::SHAPES, Fill::None)
            };
            let item = shapes::polygon(
                layer,
                polygon.path(),
                Affine::IDENTITY,
                *polygon.line_width(),
                fill,
            )
            .map(|i| i.with_z(z));
            self.insert(item);
        }
        for circle in symbol.circles().iter() {
            self.current = Some(SymbolSceneObject::Circle(circle.uuid()));
            let Some(layer) = SchematicScene::layer_id(circle.layer().color_role()) else {
                continue;
            };
            let (z, fill) = if circle.is_filled() {
                (z::FILLED, Fill::Layer)
            } else if circle.is_grab_area() {
                (
                    z::GRAB_AREAS,
                    schematic_grab_area_fill(self.scheme, circle.layer()),
                )
            } else {
                (z::SHAPES, Fill::None)
            };
            let item = shapes::circle(
                layer,
                convert::point(circle.center()),
                *circle.diameter(),
                *circle.line_width(),
                fill,
            )
            .with_z(z);
            self.insert(Some(item));
        }
        // Image borders (the images themselves are not drawn yet).
        if let Some(layer) = SchematicScene::layer_id("schematic_image_borders") {
            for image in symbol.images().iter() {
                self.current = Some(SymbolSceneObject::Image(image.uuid()));
                if let Some(border) = image.border_width() {
                    let rect = Rect::new(0.0, 0.0, image.width().to_mm(), image.height().to_mm());
                    let xf = Affine::translate(convert::point(image.position()).to_vec2())
                        * Affine::rotate(convert::angle(image.rotation()));
                    let item = Item::new(
                        layer,
                        xf * rect.to_path(1e-4),
                        Style::stroke(shapes::pen(*border)),
                    )
                    .with_z(z::IMAGES);
                    self.insert(Some(item));
                }
            }
        }
    }

    fn add_texts(&mut self, symbol: &Symbol) {
        for text in symbol.texts().iter() {
            self.current = Some(SymbolSceneObject::Text(text.uuid()));
            let Some(layer) = SchematicScene::layer_id(text.layer().color_role()) else {
                continue;
            };
            let rendered = self.text.text(
                text.text(),
                text.position(),
                TextStyle {
                    height: text.height().to_mm(),
                    align: text.align(),
                    rotation: text.rotation(),
                    auto_rotate: true,
                    mirror_in_place: false,
                    parse_overlines: false,
                },
            );
            self.insert_text(layer, z::TEXTS, rendered);
        }
    }

    fn add_pins(&mut self, symbol: &Symbol) {
        let lines = SchematicScene::layer_id("schematic_pin_lines");
        let names = SchematicScene::layer_id("schematic_pin_names");
        let numbers = SchematicScene::layer_id("schematic_pin_numbers");
        for pin in symbol.pins().iter() {
            self.current = Some(SymbolSceneObject::Pin(pin.uuid()));
            let rotation = pin.rotation();
            if let Some(layer) = lines {
                let end = pin.position()
                    + Point::new(*pin.length(), Length::ZERO).rotated(rotation, Point::ORIGIN);
                let item = shapes::line(layer, pin.position(), end, PIN_LINE_WIDTH).with_z(z::PINS);
                self.insert(Some(item));
            }
            if let Some(layer) = names {
                let rendered = self.text.text(
                    pin.name().as_str(),
                    pin.position() + pin.name_position().rotated(rotation, Point::ORIGIN),
                    TextStyle {
                        height: pin.name_height().to_mm(),
                        align: pin.name_alignment(),
                        rotation: rotation + pin.name_rotation(),
                        auto_rotate: true,
                        mirror_in_place: false,
                        parse_overlines: true,
                    },
                );
                self.insert_text(layer, z::PIN_TEXTS, rendered);
            }
            if let Some(layer) = numbers {
                let flipped = toolbox::is_text_upside_down(rotation);
                let rendered = self.text.text(
                    "1…",
                    pin.position()
                        + pin
                            .numbers_position(flipped)
                            .rotated(rotation, Point::ORIGIN),
                    TextStyle {
                        height: SymbolPin::numbers_height().to_mm(),
                        align: SymbolPin::numbers_alignment(flipped),
                        rotation,
                        auto_rotate: true,
                        mirror_in_place: false,
                        parse_overlines: false,
                    },
                );
                self.insert_text(layer, z::PIN_TEXTS, rendered);
            }
        }
    }
}
