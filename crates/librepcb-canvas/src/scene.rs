//! The retained scene: items, layers, render groups, the spatial index and
//! change tracking.
//!
//! Items are batched into *groups* by (z, layer, paint pass, world tile):
//! all items of a group are painted with a single fill or stroke call, so a
//! board with 100k traces needs only a few thousand draw calls, and a change
//! only re-encodes the groups of the changed items. The fixed world tile
//! grid keeps groups small enough for incremental updates and for culling.

use std::collections::HashMap;

use kurbo::{BezPath, Point, Rect};
use peniko::Color;
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{AABB, RTree};
use slotmap::{SecondaryMap, SlotMap};

use crate::geometry::Geometry;
use crate::style::{Brush, BrushKey, Pass, PassKey, StrokeKey, StrokeStyle, Style};

slotmap::new_key_type! {
    /// Identifies an item of a [`Scene`] (stable until the item is removed).
    pub struct ItemId;
    /// Identifies a render group of a [`Scene`] (see [`GroupView`]).
    pub struct GroupId;
}

/// Identifies a layer. The meaning (e.g. "top copper") is up to the scene
/// builder, which registers each layer with [`Scene::set_layer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct LayerId(pub u16);

/// Appearance of a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Color of items using [`Brush::Layer`].
    pub color: Color,
    /// Color of selected items.
    pub highlight_color: Color,
    /// Whether the layer is drawn and hit-testable.
    pub visible: bool,
    /// Draw order among items of the same z value (higher is drawn later).
    pub order: i32,
}

impl Layer {
    /// A visible layer with order 0 and a lighter highlight color.
    pub fn new(color: Color) -> Self {
        let [r, g, b, a] = color.components;
        let lighter = |c: f32| c + (1.0 - c) * 0.5;
        Self {
            color,
            highlight_color: Color::new([lighter(r), lighter(g), lighter(b), a.max(0.8)]),
            visible: true,
            order: 0,
        }
    }

    /// Returns the layer with another highlight color.
    pub fn with_highlight_color(mut self, color: Color) -> Self {
        self.highlight_color = color;
        self
    }

    /// Returns the layer with another draw order.
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// Returns the layer with another visibility.
    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }
}

impl Default for Layer {
    fn default() -> Self {
        Self::new(Color::from_rgba8(0xa0, 0xa0, 0xa0, 0xff))
    }
}

/// A drawable, hit-testable item.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Layer (color, visibility, draw order).
    pub layer: LayerId,
    /// Draw order: higher z is drawn later (on top); ties are ordered by
    /// [`Layer::order`], then by layer ID.
    pub z: i32,
    /// Fill and stroke.
    pub style: Style,
    /// Geometry in world coordinates (millimetres, Y up).
    pub geometry: Geometry,
}

impl Item {
    /// Creates an item with z = 0.
    pub fn new(layer: LayerId, geometry: impl Into<Geometry>, style: Style) -> Self {
        Self {
            layer,
            z: 0,
            style,
            geometry: geometry.into(),
        }
    }

    /// Returns the item with another z value.
    pub fn with_z(mut self, z: i32) -> Self {
        self.z = z;
        self
    }

    /// Bounding box of the painted area (geometry plus stroke).
    pub fn bounding_box(&self) -> Rect {
        let e = self.style.extent();
        self.geometry.bounding_box().inflate(e, e)
    }

    /// Exact hit test: whether the painted shape comes within `tolerance`
    /// of `p`. Items without fill and stroke are hit inside their area
    /// (grab areas).
    pub fn hit(&self, p: Point, tolerance: f64) -> bool {
        let filled = self.style.fill.is_some() || self.style.stroke.is_none();
        if filled && self.geometry.contains(p) {
            return true;
        }
        let half_width = self.style.stroke.as_ref().map_or(0.0, |s| s.width / 2.0);
        self.geometry.outline_within(p, half_width + tolerance)
    }

    /// Exact rectangle test for [`SelectionMode::Intersects`].
    pub fn intersects_rect(&self, rect: Rect) -> bool {
        let bbox = self.bounding_box();
        if !bbox.overlaps(rect) {
            return false;
        }
        if rect.contains_rect(bbox) {
            return true;
        }
        let filled = self.style.fill.is_some() || self.style.stroke.is_none();
        if filled && self.geometry.contains(rect.center()) {
            return true;
        }
        let half_width = self.style.stroke.as_ref().map_or(0.0, |s| s.width / 2.0);
        self.geometry.outline_touches_rect(rect, half_width)
    }
}

/// How [`Scene::items_in_rect`] selects items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// Items whose painted shape touches the rectangle.
    #[default]
    Intersects,
    /// Items completely inside the rectangle.
    Contains,
}

/// What changed in the base scene since a revision (see
/// [`Scene::damage_since`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Damage {
    /// Nothing.
    None,
    /// Everything (layer changes, cleared scene, too many changes).
    All,
    /// World rectangles to repaint.
    Rects(Vec<Rect>),
}

/// The resolved paint of a group.
#[derive(Debug, Clone, Copy)]
pub enum Paint<'a> {
    /// Non-zero fill.
    Fill(Color),
    /// Stroke.
    Stroke(Color, &'a StrokeStyle),
}

/// A render group as seen by a render backend.
#[derive(Clone, Copy)]
pub struct GroupView<'a> {
    scene: &'a Scene,
    id: GroupId,
    group: &'a Group,
}

impl<'a> GroupView<'a> {
    /// The group ID (stable while the group exists; key for encoding caches).
    pub fn id(&self) -> GroupId {
        self.id
    }

    /// Revision of the last change of the group's items; re-encode the group
    /// when it differs from the cached one.
    pub fn rev(&self) -> u64 {
        self.group.rev
    }

    /// The group's layer.
    pub fn layer(&self) -> LayerId {
        self.group.key.layer
    }

    /// Conservative bounding box of the painted area (world units, without
    /// hairline pixels).
    pub fn bounding_box(&self) -> Rect {
        self.group.bbox
    }

    /// The paint with the current layer colors.
    pub fn paint(&self) -> Paint<'a> {
        let pass = &self.scene.passes[self.group.key.pass as usize];
        let color = match pass.brush {
            Brush::Layer => self.scene.layer_or_default(self.group.key.layer).color,
            Brush::Solid(c) => c,
        };
        match &pass.stroke {
            None => Paint::Fill(color),
            Some(s) => Paint::Stroke(color, s),
        }
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.group.items.len()
    }

    /// Returns whether the group has no items (never true for live groups).
    pub fn is_empty(&self) -> bool {
        self.group.items.is_empty()
    }

    /// Appends the geometry of all items to `path` (the group's encoding).
    pub fn encode(&self, path: &mut BezPath) {
        let fill = self.scene.passes[self.group.key.pass as usize].is_fill();
        for id in &self.group.items {
            if let Some(e) = self.scene.items.get(*id) {
                e.item.geometry.append_to(path, fill);
            }
        }
    }
}

/// Selected items of one paint pass, ready to draw on top of the scene.
#[derive(Debug, Clone)]
pub struct OverlayGroup {
    /// The highlight color of the layer.
    pub color: Color,
    /// `None` for fills.
    pub stroke: Option<StrokeStyle>,
    /// The merged geometry.
    pub path: BezPath,
    /// Bounding box of the painted area (world units).
    pub bbox: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GroupKey {
    z: i32,
    layer: LayerId,
    pass: u32,
    tile: (i32, i32),
}

/// Draw order of groups.
type OrderKey = (i32, i32, LayerId, bool, u32, (i32, i32));

#[derive(Debug)]
struct Group {
    key: GroupKey,
    items: Vec<ItemId>,
    bbox: Rect,
    rev: u64,
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    group: GroupId,
    index: usize,
}

#[derive(Debug)]
struct Entry {
    item: Item,
    bbox: Rect,
    seq: u64,
    /// Fill and stroke group memberships.
    slots: [Option<Slot>; 2],
    indexed: bool,
}

type IndexEntry = GeomWithData<Rectangle<[f64; 2]>, ItemId>;

fn index_entry(bbox: Rect, id: ItemId) -> IndexEntry {
    GeomWithData::new(
        Rectangle::from_corners([bbox.x0, bbox.y0], [bbox.x1, bbox.y1]),
        id,
    )
}

/// Maximum number of damage rectangles kept before the oldest half is
/// dropped (renderers that are further behind repaint everything).
const DAMAGE_LOG_LIMIT: usize = 8192;

/// The retained scene. See the [crate documentation](crate).
#[derive(Debug)]
pub struct Scene {
    items: SlotMap<ItemId, Entry>,
    layers: HashMap<LayerId, Layer>,
    groups: SlotMap<GroupId, Group>,
    group_index: HashMap<GroupKey, GroupId>,
    order: Vec<(OrderKey, GroupId)>,
    passes: Vec<Pass>,
    pass_index: HashMap<PassKey, u32>,
    index: RTree<IndexEntry>,
    selection: SecondaryMap<ItemId, ()>,
    tile_size: f64,
    next_seq: u64,
    rev: u64,
    damage: Vec<(u64, Rect)>,
    damage_floor: u64,
    full_damage_rev: u64,
    selection_rev: u64,
}

// Scenes may be built on worker threads and moved to the UI thread.
static_assertions::assert_impl_all!(Scene: Send, Sync);

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    /// Default edge length of the world tiles used to batch items (mm).
    pub const DEFAULT_TILE_SIZE: f64 = 16.0;

    /// Creates an empty scene with the default tile size.
    pub fn new() -> Self {
        Self::with_tile_size(Self::DEFAULT_TILE_SIZE)
    }

    /// Creates an empty scene batching items into world tiles of the given
    /// edge length (mm). Smaller tiles mean cheaper incremental updates and
    /// finer culling, but more draw calls.
    pub fn with_tile_size(tile_size: f64) -> Self {
        Self {
            items: SlotMap::with_key(),
            layers: HashMap::new(),
            groups: SlotMap::with_key(),
            group_index: HashMap::new(),
            order: Vec::new(),
            passes: Vec::new(),
            pass_index: HashMap::new(),
            index: RTree::new(),
            selection: SecondaryMap::new(),
            tile_size: if tile_size.is_finite() && tile_size > 0.0 {
                tile_size
            } else {
                Self::DEFAULT_TILE_SIZE
            },
            next_seq: 0,
            rev: 1,
            damage: Vec::new(),
            damage_floor: 0,
            full_damage_rev: 1,
            selection_rev: 1,
        }
    }

    // ------------------------------------------------------------------
    // Layers
    // ------------------------------------------------------------------

    /// Registers or changes a layer. Only the affected parts are updated:
    /// color and visibility changes repaint without re-encoding geometry.
    pub fn set_layer(&mut self, id: LayerId, layer: Layer) {
        let old = self.layers.insert(id, layer.clone());
        let old = old.unwrap_or_default();
        if old == layer {
            return;
        }
        self.bump();
        if old.order != layer.order {
            self.resort();
        }
        if old.color != layer.color || old.visible != layer.visible || old.order != layer.order {
            self.full_damage_rev = self.rev;
        }
        self.selection_rev = self.rev;
    }

    /// Returns a registered layer.
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(&id)
    }

    /// Returns all registered layers.
    pub fn layers(&self) -> impl Iterator<Item = (LayerId, &Layer)> {
        self.layers.iter().map(|(id, l)| (*id, l))
    }

    /// Shows or hides a layer (registering it with defaults if needed).
    pub fn set_layer_visible(&mut self, id: LayerId, visible: bool) {
        let layer = self.layer_or_default(id).clone().with_visible(visible);
        self.set_layer(id, layer);
    }

    /// Changes the color of a layer (registering it with defaults if
    /// needed).
    pub fn set_layer_color(&mut self, id: LayerId, color: Color) {
        let mut layer = self.layer_or_default(id).clone();
        layer.color = color;
        self.set_layer(id, layer);
    }

    /// Returns whether a layer is visible (unregistered layers are).
    pub fn is_layer_visible(&self, id: LayerId) -> bool {
        self.layers.get(&id).is_none_or(|l| l.visible)
    }

    fn layer_or_default(&self, id: LayerId) -> &Layer {
        static DEFAULT: std::sync::OnceLock<Layer> = std::sync::OnceLock::new();
        self.layers
            .get(&id)
            .unwrap_or_else(|| DEFAULT.get_or_init(Layer::default))
    }

    // ------------------------------------------------------------------
    // Items
    // ------------------------------------------------------------------

    /// Adds an item. Filled paths are normalized so that every closed
    /// subpath adds to the filled area (like a union), except for subpaths
    /// nested in others (within their bounding box and covered by them),
    /// which become holes like with the odd-even rule (e.g. pad drills).
    pub fn insert(&mut self, item: Item) -> ItemId {
        self.bump();
        let id = self.insert_entry(item);
        self.index_item(id);
        id
    }

    /// Adds many items.
    ///
    /// The spatial index is not bulk-loaded: rstar 0.13.0
    /// `RTree::bulk_load()` builds trees which panic ("This is a bug in
    /// rstar.") on later insertions after removals, i.e. on item updates.
    pub fn extend(&mut self, items: impl IntoIterator<Item = Item>) -> Vec<ItemId> {
        self.bump();
        let ids: Vec<ItemId> = items.into_iter().map(|i| self.insert_entry(i)).collect();
        for id in &ids {
            self.index_item(*id);
        }
        ids
    }

    /// Replaces an item (keeping its ID, draw sequence and selection).
    /// Returns `false` if the item does not exist.
    pub fn update(&mut self, id: ItemId, item: Item) -> bool {
        if !self.items.contains_key(id) {
            return false;
        }
        self.bump();
        self.unlink(id);
        let mut item = item;
        if item.style.fill.is_some() {
            item.geometry.normalize_winding();
        }
        let bbox = item.bounding_box();
        if let Some(e) = self.items.get_mut(id) {
            e.item = item;
            e.bbox = bbox;
        }
        self.link(id);
        self.index_item(id);
        if self.selection.contains_key(id) {
            self.selection_rev = self.rev;
        }
        true
    }

    /// Removes an item.
    pub fn remove(&mut self, id: ItemId) -> Option<Item> {
        if !self.items.contains_key(id) {
            return None;
        }
        self.bump();
        self.unlink(id);
        if self.selection.remove(id).is_some() {
            self.selection_rev = self.rev;
        }
        self.items.remove(id).map(|e| e.item)
    }

    /// Removes all items (layers are kept).
    pub fn clear(&mut self) {
        self.bump();
        self.items.clear();
        self.groups.clear();
        self.group_index.clear();
        self.order.clear();
        self.index = RTree::new();
        self.selection.clear();
        self.damage.clear();
        self.full_damage_rev = self.rev;
        self.selection_rev = self.rev;
    }

    /// Returns an item.
    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.get(id).map(|e| &e.item)
    }

    /// Returns all items (in no particular order).
    pub fn items(&self) -> impl Iterator<Item = (ItemId, &Item)> {
        self.items.iter().map(|(id, e)| (id, &e.item))
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns whether the scene has no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Bounding box of all items (e.g. for "zoom to fit"), `None` if empty.
    pub fn bounding_box(&self) -> Option<Rect> {
        if self.index.size() == 0 {
            return None;
        }
        let env = self.index.root().envelope();
        let (lo, hi) = (env.lower(), env.upper());
        Some(Rect::new(lo[0], lo[1], hi[0], hi[1]))
    }

    // ------------------------------------------------------------------
    // Selection
    // ------------------------------------------------------------------

    /// Selects or deselects an item. Selected items are drawn on top in the
    /// highlight color of their layer, without repainting the scene.
    pub fn set_selected(&mut self, id: ItemId, selected: bool) {
        if !self.items.contains_key(id) || self.selection.contains_key(id) == selected {
            return;
        }
        if selected {
            self.selection.insert(id, ());
        } else {
            self.selection.remove(id);
        }
        self.bump();
        self.selection_rev = self.rev;
    }

    /// Deselects all items.
    pub fn clear_selection(&mut self) {
        if !self.selection.is_empty() {
            self.selection.clear();
            self.bump();
            self.selection_rev = self.rev;
        }
    }

    /// Returns whether an item is selected.
    pub fn is_selected(&self, id: ItemId) -> bool {
        self.selection.contains_key(id)
    }

    /// Returns the selected items.
    pub fn selection(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.selection.keys()
    }

    // ------------------------------------------------------------------
    // Hit testing
    // ------------------------------------------------------------------

    /// Returns the topmost visible item within `tolerance` (world units) of
    /// `p`.
    pub fn hit_test(&self, p: Point, tolerance: f64) -> Option<ItemId> {
        self.hit_test_with(p, tolerance, |_, _| true)
    }

    /// Like [`hit_test`](Self::hit_test), only considering items accepted by
    /// `filter`.
    pub fn hit_test_with(
        &self,
        p: Point,
        tolerance: f64,
        mut filter: impl FnMut(ItemId, &Item) -> bool,
    ) -> Option<ItemId> {
        self.candidates(Rect::from_center_size(
            p,
            (2.0 * tolerance, 2.0 * tolerance),
        ))
        .filter(|(id, e)| filter(*id, &e.item) && e.item.hit(p, tolerance))
        .max_by_key(|(_, e)| self.item_order(e))
        .map(|(id, _)| id)
    }

    /// Returns all visible items within `tolerance` of `p`, topmost first.
    pub fn items_at(&self, p: Point, tolerance: f64) -> Vec<ItemId> {
        let mut hits: Vec<_> = self
            .candidates(Rect::from_center_size(
                p,
                (2.0 * tolerance, 2.0 * tolerance),
            ))
            .filter(|(_, e)| e.item.hit(p, tolerance))
            .collect();
        hits.sort_by_key(|(_, e)| std::cmp::Reverse(self.item_order(e)));
        hits.into_iter().map(|(id, _)| id).collect()
    }

    /// Returns all visible items selected by a rectangle, topmost first.
    pub fn items_in_rect(&self, rect: Rect, mode: SelectionMode) -> Vec<ItemId> {
        let rect = rect.abs();
        let mut hits: Vec<_> = self
            .candidates(rect)
            .filter(|(_, e)| match mode {
                SelectionMode::Contains => rect.contains_rect(e.bbox),
                SelectionMode::Intersects => e.item.intersects_rect(rect),
            })
            .collect();
        hits.sort_by_key(|(_, e)| std::cmp::Reverse(self.item_order(e)));
        hits.into_iter().map(|(id, _)| id).collect()
    }

    /// Index candidates (bounding box intersects) on visible layers.
    fn candidates(&self, rect: Rect) -> impl Iterator<Item = (ItemId, &Entry)> {
        let env = AABB::from_corners([rect.x0, rect.y0], [rect.x1, rect.y1]);
        self.index
            .locate_in_envelope_intersecting(env)
            .filter_map(|g| self.items.get(g.data).map(|e| (g.data, e)))
            .filter(|(_, e)| self.is_layer_visible(e.item.layer))
    }

    fn item_order(&self, e: &Entry) -> (i32, i32, LayerId, u64) {
        let layer_order = self.layers.get(&e.item.layer).map_or(0, |l| l.order);
        (e.item.z, layer_order, e.item.layer, e.seq)
    }

    // ------------------------------------------------------------------
    // Render backend interface
    // ------------------------------------------------------------------

    /// The current revision (increases with every change).
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Revision of the last change of the selection overlay (selection,
    /// selected items, layers).
    pub fn selection_rev(&self) -> u64 {
        self.selection_rev
    }

    /// What changed in the base scene (without selection) after revision
    /// `rev` (a value of [`Scene::rev`] seen before).
    pub fn damage_since(&self, rev: u64) -> Damage {
        if rev >= self.rev {
            return Damage::None;
        }
        if self.full_damage_rev > rev || self.damage_floor > rev {
            return Damage::All;
        }
        let start = self.damage.partition_point(|(r, _)| *r <= rev);
        let rects: Vec<Rect> = self.damage[start..].iter().map(|(_, r)| *r).collect();
        if rects.is_empty() {
            Damage::None
        } else {
            Damage::Rects(rects)
        }
    }

    /// Groups of visible layers in draw order.
    pub fn draw_groups(&self) -> impl Iterator<Item = GroupView<'_>> {
        self.order.iter().filter_map(|(_, gid)| {
            let group = self.groups.get(*gid)?;
            self.is_layer_visible(group.key.layer).then_some(GroupView {
                scene: self,
                id: *gid,
                group,
            })
        })
    }

    /// Returns whether a group still exists (for pruning encoding caches).
    pub fn contains_group(&self, id: GroupId) -> bool {
        self.groups.contains_key(id)
    }

    /// Number of render groups.
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Builds the selection overlay: selected items on visible layers,
    /// merged per (z, layer, pass), in draw order, in highlight colors.
    pub fn selection_overlay(&self) -> Vec<OverlayGroup> {
        let mut map: HashMap<(i32, LayerId, u32), OverlayGroup> = HashMap::new();
        for id in self.selection.keys() {
            let Some(e) = self.items.get(id) else {
                continue;
            };
            if !self.is_layer_visible(e.item.layer) {
                continue;
            }
            let layer = self.layer_or_default(e.item.layer);
            for slot in e.slots.iter().flatten() {
                let Some(g) = self.groups.get(slot.group) else {
                    continue;
                };
                let pass = &self.passes[g.key.pass as usize];
                let entry = map
                    .entry((e.item.z, e.item.layer, g.key.pass))
                    .or_insert_with(|| OverlayGroup {
                        color: layer.highlight_color,
                        stroke: pass.stroke.clone(),
                        path: BezPath::new(),
                        bbox: e.bbox,
                    });
                e.item.geometry.append_to(&mut entry.path, pass.is_fill());
                entry.bbox = entry.bbox.union(e.bbox);
            }
        }
        let mut groups: Vec<_> = map
            .into_iter()
            .map(|((z, layer, pass), g)| {
                let order = self.layers.get(&layer).map_or(0, |l| l.order);
                let is_stroke = self.passes[pass as usize].stroke.is_some();
                ((z, order, layer, is_stroke, pass), g)
            })
            .collect();
        groups.sort_by_key(|(k, _)| *k);
        groups.into_iter().map(|(_, g)| g).collect()
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    fn bump(&mut self) {
        self.rev += 1;
    }

    fn add_damage(&mut self, rect: Rect) {
        if self.damage.len() >= DAMAGE_LOG_LIMIT {
            let drop = DAMAGE_LOG_LIMIT / 2;
            self.damage_floor = self.damage[drop - 1].0;
            self.damage.drain(..drop);
        }
        self.damage.push((self.rev, rect));
    }

    fn insert_entry(&mut self, mut item: Item) -> ItemId {
        if item.style.fill.is_some() {
            item.geometry.normalize_winding();
        }
        let bbox = item.bounding_box();
        let seq = self.next_seq;
        self.next_seq += 1;
        let id = self.items.insert(Entry {
            item,
            bbox,
            seq,
            slots: [None, None],
            indexed: false,
        });
        self.link(id);
        id
    }

    fn index_item(&mut self, id: ItemId) {
        let Some(e) = self.items.get_mut(id) else {
            return;
        };
        if e.item.geometry.is_empty() || e.indexed {
            return;
        }
        e.indexed = true;
        let entry = index_entry(e.bbox, id);
        self.index.insert(entry);
    }

    fn intern_pass(&mut self, key: PassKey, pass: Pass) -> u32 {
        if let Some(i) = self.pass_index.get(&key) {
            return *i;
        }
        let i = self.passes.len() as u32;
        self.passes.push(pass);
        self.pass_index.insert(key, i);
        i
    }

    fn tile_of(&self, bbox: Rect) -> (i32, i32) {
        let c = bbox.center();
        let t = |v: f64| {
            (v / self.tile_size)
                .floor()
                .clamp(i32::MIN as f64, i32::MAX as f64) as i32
        };
        (t(c.x), t(c.y))
    }

    fn order_key(&self, key: &GroupKey) -> OrderKey {
        let layer_order = self.layers.get(&key.layer).map_or(0, |l| l.order);
        let is_stroke = self.passes[key.pass as usize].stroke.is_some();
        (key.z, layer_order, key.layer, is_stroke, key.pass, key.tile)
    }

    fn resort(&mut self) {
        let mut order: Vec<_> = self
            .groups
            .iter()
            .map(|(gid, g)| (self.order_key(&g.key), gid))
            .collect();
        order.sort_unstable_by_key(|(k, _)| *k);
        self.order = order;
    }

    /// Adds the item to its groups and records damage.
    fn link(&mut self, id: ItemId) {
        let Some(e) = self.items.get(id) else {
            return;
        };
        if e.item.geometry.is_empty() {
            return;
        }
        let (layer, z, bbox) = (e.item.layer, e.item.z, e.bbox);
        let tile = self.tile_of(bbox);
        let mut passes: [Option<(PassKey, Pass)>; 2] = [None, None];
        if let Some(brush) = e.item.style.fill
            && !matches!(e.item.geometry, Geometry::Line(_))
        {
            passes[0] = Some((
                PassKey::Fill(BrushKey::new(brush)),
                Pass {
                    brush,
                    stroke: None,
                },
            ));
        }
        if let Some(stroke) = &e.item.style.stroke {
            passes[1] = Some((
                PassKey::Stroke(BrushKey::new(stroke.brush), StrokeKey::new(stroke)),
                Pass {
                    brush: stroke.brush,
                    stroke: Some(stroke.clone()),
                },
            ));
        }
        let mut slots = [None, None];
        for (k, pass) in passes.into_iter().enumerate() {
            let Some((pass_key, pass)) = pass else {
                continue;
            };
            let pass = self.intern_pass(pass_key, pass);
            let key = GroupKey {
                z,
                layer,
                pass,
                tile,
            };
            let gid = match self.group_index.get(&key) {
                Some(gid) => *gid,
                None => {
                    let gid = self.groups.insert(Group {
                        key,
                        items: Vec::new(),
                        bbox,
                        rev: self.rev,
                    });
                    self.group_index.insert(key, gid);
                    let ok = self.order_key(&key);
                    let pos = self.order.partition_point(|(k, _)| *k < ok);
                    self.order.insert(pos, (ok, gid));
                    gid
                }
            };
            let rev = self.rev;
            if let Some(g) = self.groups.get_mut(gid) {
                g.items.push(id);
                g.bbox = g.bbox.union(bbox);
                g.rev = rev;
                slots[k] = Some(Slot {
                    group: gid,
                    index: g.items.len() - 1,
                });
            }
        }
        if let Some(e) = self.items.get_mut(id) {
            e.slots = slots;
        }
        self.add_damage(bbox);
    }

    /// Removes the item from its groups and the index, and records damage.
    fn unlink(&mut self, id: ItemId) {
        let Some(e) = self.items.get_mut(id) else {
            return;
        };
        let slots = std::mem::take(&mut e.slots);
        let (bbox, indexed) = (e.bbox, e.indexed);
        e.indexed = false;
        if indexed {
            self.index.remove(&index_entry(bbox, id));
        }
        for slot in slots.into_iter().flatten() {
            self.remove_from_group(id, slot, bbox);
        }
        if !self.items[id].item.geometry.is_empty() {
            self.add_damage(bbox);
        }
    }

    fn remove_from_group(&mut self, id: ItemId, slot: Slot, bbox: Rect) {
        let rev = self.rev;
        let Some(g) = self.groups.get_mut(slot.group) else {
            return;
        };
        debug_assert_eq!(g.items.get(slot.index), Some(&id));
        g.items.swap_remove(slot.index);
        g.rev = rev;
        if let Some(moved) = g.items.get(slot.index).copied()
            && let Some(m) = self.items.get_mut(moved)
        {
            for s in m.slots.iter_mut().flatten() {
                if s.group == slot.group {
                    s.index = slot.index;
                }
            }
        }
        let g = &self.groups[slot.group];
        if g.items.is_empty() {
            let key = g.key;
            let ok = self.order_key(&key);
            let pos = self.order.partition_point(|(k, _)| *k < ok);
            if self
                .order
                .get(pos)
                .is_some_and(|(_, gid)| *gid == slot.group)
            {
                self.order.remove(pos);
            }
            self.group_index.remove(&key);
            self.groups.remove(slot.group);
        } else if touches_border(bbox, g.bbox) {
            // Shrink the bounding box again (it cannot shrink if the item
            // was strictly inside, which keeps removals from large groups
            // cheap).
            let bbox = g
                .items
                .iter()
                .filter_map(|i| self.items.get(*i).map(|e| e.bbox))
                .reduce(|a, b| a.union(b))
                .unwrap_or(Rect::ZERO);
            self.groups[slot.group].bbox = bbox;
        }
    }
}

/// Whether `inner` (contained in `outer`) reaches the border of `outer`.
fn touches_border(inner: Rect, outer: Rect) -> bool {
    inner.x0 <= outer.x0 || inner.y0 <= outer.y0 || inner.x1 >= outer.x1 || inner.y1 >= outer.y1
}
