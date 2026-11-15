//! Scene builder of library footprints: port of the drawing semantics of
//! libs/librepcb/core/library/pkg/footprintpainter.{h,cpp} (used by the
//! graphics export of packages, e.g. `librepcb-cli open-package --export`).
//!
//! Every board color role is a canvas layer ([`FootprintScene::layer_id()`]),
//! painted in upstream's graphics export order
//! (`GraphicsExportSettings::getPaintOrder()` of the default settings).
//! Per role, like the painter: pad areas (THT pads on `board_pads`, SMT
//! pads on their copper layer), polygons, circles, hole outlines and the
//! stroke texts (raw, e.g. `{{NAME}}`, with the default stroke font). The
//! outlines of pad holes are an extra layer ([`FootprintScene::PAD_HOLES`]),
//! shown only if holes but no pads are drawn (upstream `drawPadHoles`).
//! Zones are not drawn (like upstream). The invisible TrueType texts
//! upstream adds to PDF/SVG exports are not drawn.

use foilfern_canvas::kurbo::{Affine, Rect};
use foilfern_canvas::peniko::Color;
use std::collections::HashMap;

use foilfern_canvas::{
    Item, ItemId, Layer as CanvasLayer, LayerId, Scene, SelectionMode, Style, convert,
};
use foilfern_core::export::GraphicsExportSettings;
use foilfern_core::font::StrokeFont;
use foilfern_core::geometry::Path;
use foilfern_core::library::pkg::Footprint;
use foilfern_core::types::{Layer, Length, Point, Uuid};
use foilfern_core::utils::transform::Transform;

use crate::colors::ColorScheme;
use crate::shapes::{self, Fill};

/// Sub-ordering of items within one color role (upstream paint order).
mod sub {
    pub const AREAS: i32 = 0;
    pub const POLYGONS: i32 = 1;
    pub const HOLES: i32 = 2;
    pub const PAD_HOLES: i32 = 3;
}

/// The model object an item of a [`FootprintScene`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FootprintSceneObject {
    /// A pad (area and holes).
    Pad(Uuid),
    /// A polygon.
    Polygon(Uuid),
    /// A circle.
    Circle(Uuid),
    /// A stroke text.
    StrokeText(Uuid),
    /// A non-plated hole.
    Hole(Uuid),
    /// A keepout zone (only drawn by [`FootprintScene::build_for_editor()`]).
    Zone(Uuid),
}

/// A library footprint as a canvas [`Scene`].
///
/// Every item is mapped to its model object ([`FootprintSceneObject`]) for
/// hit testing and selection highlighting in the package editor; the scene
/// is rebuilt after each modification (footprints are small).
#[derive(Debug)]
pub struct FootprintScene {
    scene: Scene,
    roles: Vec<String>,
    objects: HashMap<ItemId, FootprintSceneObject>,
}

static_assertions::assert_impl_all!(FootprintScene: Send, Sync);

impl FootprintScene {
    /// The canvas layer of the pad hole outlines (drawn in the
    /// `board_holes` color).
    pub const PAD_HOLES: LayerId = LayerId(1000);
    /// Optional contextual preview labels, above all footprint geometry.
    pub const PAD_NAMES: LayerId = LayerId(1002);

    /// Builds the scene of a footprint with a color scheme (e.g. the colors
    /// of a graphics export); stroke texts are drawn with `font` (none
    /// without a font, see [`crate::default_stroke_font()`]). Roles without
    /// color in the scheme are hidden.
    pub fn build(footprint: &Footprint, font: Option<&StrokeFont>, scheme: &ColorScheme) -> Self {
        let roles: Vec<String> = GraphicsExportSettings::default()
            .paint_order()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut builder = Builder {
            scene: Scene::new(),
            roles: &roles,
            scheme,
            objects: HashMap::new(),
            current: None,
        };
        builder.add_layers();
        builder.add_polygons(footprint);
        builder.add_holes(footprint);
        builder.add_pads(footprint);
        if let Some(font) = font {
            builder.add_texts(footprint, font);
        }
        let objects = builder.objects;
        Self {
            scene: builder.scene,
            roles,
            objects,
        }
    }

    /// The canvas layer of the keepout zones (drawn in the `board_zones`
    /// color by [`Self::build_for_editor()`]).
    pub const ZONES: LayerId = LayerId(1001);

    /// Device-context preview with readable package pad names over the copper.
    /// The labels are preview annotations and never modify library geometry.
    pub fn build_with_pad_names(
        footprint: &Footprint,
        package: &foilfern_core::library::pkg::Package,
        font: Option<&StrokeFont>,
        scheme: &ColorScheme,
    ) -> Self {
        use crate::text::{TextRenderer, TextStyle};
        use foilfern_core::types::{Alignment, Angle, HAlign, VAlign};
        let mut result = Self::build(footprint, font, scheme);
        let layer = Self::PAD_NAMES;
        result
            .scene
            .set_layer(layer, CanvasLayer::new(Color::WHITE).with_order(1002));
        let mut renderer = TextRenderer::new(font);
        let mut labels = Vec::new();
        let mut scale = 1.0_f64;
        for pad in footprint.pads().iter() {
            let Some(name) = pad
                .package_pad_uuid()
                .and_then(|id| package.pads().by_uuid(&id))
                .map(|pad| pad.name().as_str())
            else {
                continue;
            };
            let position = pad.pad().position();
            if let Some(text) = renderer.text(
                name,
                position,
                TextStyle {
                    height: 0.7,
                    align: Alignment::new(HAlign::Center, VAlign::Center),
                    rotation: Angle::DEG0,
                    auto_rotate: true,
                    mirror_in_place: false,
                    parse_overlines: false,
                },
            ) {
                use foilfern_canvas::kurbo::Shape;
                let bounds = text.path.bounding_box();
                let span = bounds.width().max(bounds.height()) + text.stroke_width;
                let spacing = footprint
                    .pads()
                    .iter()
                    .filter(|other| other.uuid() != pad.uuid())
                    .map(|other| {
                        let delta = other.pad().position() - position;
                        delta.x.to_mm().abs().max(delta.y.to_mm().abs())
                    })
                    .filter(|distance| *distance > 0.0)
                    .reduce(f64::min);
                if let Some(spacing) = spacing
                    && span > 0.0
                {
                    scale = scale.min(0.8 * spacing / span);
                }
                labels.push((pad.uuid(), position, text));
            }
        }
        // One shared font scale keeps long pad names complete and separates
        // adjacent labels; individual numbers are never elided or guessed.
        for (uuid, position, text) in labels {
            let anchor = convert::point(position);
            let transform = Affine::translate(anchor.to_vec2())
                * Affine::scale(scale)
                * Affine::translate((-anchor.x, -anchor.y));
            let item = Item::new(
                layer,
                transform * text.path,
                Style::stroke(text.stroke_width * scale),
            )
            .with_z(1002);
            let id = result.scene.insert(item);
            result.objects.insert(id, FootprintSceneObject::Pad(uuid));
        }
        result
    }

    /// Like [`Self::build()`], but also draws the keepout zones (upstream
    /// `FootprintGraphicsItem` of the package editor), filled with the
    /// `board_zones` color and outlined, above all other layers.
    pub fn build_for_editor(
        footprint: &Footprint,
        font: Option<&StrokeFont>,
        scheme: &ColorScheme,
    ) -> Self {
        let mut scene = Self::build(footprint, font, scheme);
        let color = scheme.color("board_zones");
        let order = scene.roles.len() as i32;
        scene.scene.set_layer(
            Self::ZONES,
            CanvasLayer::new(color.unwrap_or(Color::TRANSPARENT))
                .with_order(order)
                .with_visible(color.is_some()),
        );
        for zone in footprint.zones().iter() {
            let mut path = zone.outline().clone();
            path.close();
            let item = shapes::polygon(
                LayerId::default(),
                &path,
                Affine::IDENTITY,
                Length::ZERO,
                Fill::Layer,
            )
            .map(|mut i| {
                i.style.stroke = Some(foilfern_canvas::StrokeStyle::hairline());
                i
            });
            if let Some(mut item) = item
                && !item.geometry.is_empty()
            {
                item.layer = Self::ZONES;
                item.z = order * 4;
                let id = scene.scene.insert(item);
                scene
                    .objects
                    .insert(id, FootprintSceneObject::Zone(zone.uuid()));
            }
        }
        scene
    }

    /// The model object of an item.
    pub fn object(&self, item: ItemId) -> Option<FootprintSceneObject> {
        self.objects.get(&item).copied()
    }

    /// The items of a model object (e.g. to highlight the selection).
    pub fn items_of(&self, object: FootprintSceneObject) -> Vec<ItemId> {
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
    pub fn objects_at(&self, pos: Point, tolerance: Length) -> Vec<FootprintSceneObject> {
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
    pub fn objects_in_rect(&self, p1: Point, p2: Point) -> Vec<FootprintSceneObject> {
        let rect = Rect::from_points(convert::point(p1), convert::point(p2));
        let mut objects: Vec<FootprintSceneObject> = self
            .scene
            .items_in_rect(rect, SelectionMode::Intersects)
            .into_iter()
            .filter_map(|i| self.object(i))
            .collect();
        objects.sort();
        objects.dedup();
        objects
    }

    /// The canvas layer of a color role (e.g. `"board_legend_top"`), `None`
    /// if the role is not drawn.
    pub fn layer_id(&self, role: &str) -> Option<LayerId> {
        self.roles
            .iter()
            .position(|r| r == role)
            .map(|i| LayerId(i as u16))
    }

    /// The scene.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The scene for modification.
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// Bounding box of all visible items (world coordinates, mm).
    pub fn content_bounds(&self) -> Option<Rect> {
        crate::render::visible_bounds(&self.scene)
    }
}

struct Builder<'a> {
    scene: Scene,
    roles: &'a [String],
    scheme: &'a ColorScheme,
    objects: HashMap<ItemId, FootprintSceneObject>,
    /// The object of the items inserted next.
    current: Option<FootprintSceneObject>,
}

impl Builder<'_> {
    fn rank(&self, role: &str) -> Option<i32> {
        self.roles.iter().position(|r| r == role).map(|i| i as i32)
    }

    fn add_layers(&mut self) {
        for (rank, role) in self.roles.iter().enumerate() {
            let color = self.scheme.color(role);
            self.scene.set_layer(
                LayerId(rank as u16),
                CanvasLayer::new(color.unwrap_or(Color::TRANSPARENT))
                    .with_order(rank as i32)
                    .with_visible(color.is_some()),
            );
        }
        // Pad holes are drawn if holes are enabled, but pads not.
        let holes = self.scheme.color("board_holes");
        let visible = holes.is_some() && self.scheme.color("board_pads").is_none();
        let rank = self.rank("board_holes").unwrap_or_default();
        self.scene.set_layer(
            FootprintScene::PAD_HOLES,
            CanvasLayer::new(holes.unwrap_or(Color::TRANSPARENT))
                .with_order(rank)
                .with_visible(visible),
        );
    }

    fn insert(&mut self, role: &str, sub: i32, item: Option<Item>) {
        let Some(rank) = self.rank(role) else {
            return;
        };
        self.insert_on(LayerId(rank as u16), rank, sub, item);
    }

    fn insert_on(&mut self, layer: LayerId, rank: i32, sub: i32, item: Option<Item>) {
        if let Some(mut item) = item
            && !item.geometry.is_empty()
        {
            item.layer = layer;
            item.z = rank * 4 + sub;
            let id = self.scene.insert(item);
            if let Some(object) = self.current {
                self.objects.insert(id, object);
            }
        }
    }

    /// Upstream `getFillColor()`.
    fn fill(&self, layer: Layer, filled: bool, grab_area: bool) -> Fill {
        if filled {
            return Fill::Layer;
        }
        if grab_area {
            // Upstream `ColorRole::getGrabAreaRole()`.
            let role = if layer == Layer::TOP_LEGEND {
                "board_grab_areas_top"
            } else if layer == Layer::BOT_LEGEND {
                "board_grab_areas_bottom"
            } else {
                return Fill::None;
            };
            return self.scheme.color(role).map_or(Fill::None, Fill::Solid);
        }
        Fill::None
    }

    fn add_polygons(&mut self, footprint: &Footprint) {
        for polygon in footprint.polygons().iter() {
            self.current = Some(FootprintSceneObject::Polygon(polygon.uuid()));
            let fill = self.fill(polygon.layer(), polygon.is_filled(), polygon.is_grab_area());
            let item = shapes::polygon(
                LayerId::default(),
                polygon.path(),
                Affine::IDENTITY,
                *polygon.line_width(),
                fill,
            );
            self.insert(polygon.layer().color_role(), sub::POLYGONS, item);
        }
        for circle in footprint.circles().iter() {
            self.current = Some(FootprintSceneObject::Circle(circle.uuid()));
            let fill = self.fill(circle.layer(), circle.is_filled(), circle.is_grab_area());
            let item = shapes::circle(
                LayerId::default(),
                convert::point(circle.center()),
                *circle.diameter(),
                *circle.line_width(),
                fill,
            );
            self.insert(circle.layer().color_role(), sub::POLYGONS, Some(item));
        }
    }

    fn add_holes(&mut self, footprint: &Footprint) {
        for hole in footprint.holes().iter() {
            self.current = Some(FootprintSceneObject::Hole(hole.uuid()));
            let strokes = hole.path().get().to_outline_strokes(hole.diameter());
            let item = shapes::outline(LayerId::default(), &strokes, Affine::IDENTITY);
            self.insert("board_holes", sub::HOLES, item);
        }
    }

    fn add_pads(&mut self, footprint: &Footprint) {
        for fpt_pad in footprint.pads().iter() {
            self.current = Some(FootprintSceneObject::Pad(fpt_pad.uuid()));
            let pad = fpt_pad.pad();
            let xf = convert::transform(&Transform::new(pad.position(), pad.rotation(), false));
            let role = if pad.is_tht() {
                "board_pads"
            } else {
                pad.smt_layer().color_role()
            };
            let geometry = pad.geometry();
            if let Ok(outlines) = geometry.to_outlines() {
                let item = shapes::area_with_holes(
                    LayerId::default(),
                    &outlines,
                    &geometry.to_hole_outlines(),
                    xf,
                );
                self.insert(role, sub::AREAS, item);
            }
            // Holes of THT pads.
            let rank = self.rank("board_holes").unwrap_or_default();
            for hole in pad.holes().iter() {
                let strokes: Vec<Path> = hole.path().get().to_outline_strokes(hole.diameter());
                let item = shapes::outline(LayerId::default(), &strokes, xf);
                self.insert_on(FootprintScene::PAD_HOLES, rank, sub::PAD_HOLES, item);
            }
        }
    }

    fn add_texts(&mut self, footprint: &Footprint, font: &StrokeFont) {
        for text in footprint.stroke_texts().iter() {
            self.current = Some(FootprintSceneObject::StrokeText(text.uuid()));
            let xf = convert::transform(&Transform::new(
                text.position(),
                text.rotation(),
                text.mirrored(),
            ));
            let paths = text.generate_paths(font);
            if paths.is_empty() {
                continue;
            }
            let item = Item::new(
                LayerId::default(),
                xf * convert::paths(&paths),
                Style::stroke(shapes::pen(*text.stroke_width())),
            );
            self.insert(text.layer().color_role(), sub::POLYGONS, Some(item));
        }
    }
}
