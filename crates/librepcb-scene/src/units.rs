//! Bookkeeping for incremental scene updates (no upstream counterpart:
//! upstream's graphics items update themselves through Qt signals).
//!
//! A scene is built per *unit*: one model object whose canvas items are
//! always built together (a symbol, a device, a trace, a plane, ...).
//! [`Units`] maps units to their canvas items and back to the model
//! objects, and replaces the items of a unit in place when it is rebuilt,
//! reusing the item IDs (so the selection of unchanged units, and mostly
//! of rebuilt ones, survives). [`DepIndex`] records which units an item
//! of another unit depends on (e.g. a net segment on the symbols its lines
//! end at), so the dependents of a changed unit can be found even after
//! the model object is gone.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use librepcb_canvas::{Item, ItemId, Scene};

/// The items and problems produced by building one unit.
#[derive(Debug)]
pub(crate) struct Built<O> {
    pub items: Vec<(Item, O)>,
    pub warnings: Vec<String>,
}

impl<O> Default for Built<O> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// The canvas items of all units of a scene.
#[derive(Debug)]
pub(crate) struct Units<U, O> {
    items: HashMap<U, Vec<ItemId>>,
    objects: HashMap<ItemId, O>,
    warnings: HashMap<U, Vec<String>>,
}

impl<U: Copy + Eq + Hash, O: Copy> Units<U, O> {
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            objects: HashMap::new(),
            warnings: HashMap::new(),
        }
    }

    /// The model object of an item.
    pub fn object(&self, item: ItemId) -> Option<O> {
        self.objects.get(&item).copied()
    }

    /// The units which had problems when they were built last.
    pub fn with_warnings(&self) -> impl Iterator<Item = U> + '_ {
        self.warnings.keys().copied()
    }

    /// All problems, sorted.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = self.warnings.values().flatten().cloned().collect();
        warnings.sort();
        warnings
    }

    /// Replaces the items of units (`Some`, adding new units) or removes
    /// units (`None`). Existing item IDs are reused in order; new items
    /// are inserted in one batch.
    pub fn commit(&mut self, scene: &mut Scene, updates: Vec<(U, Option<Built<O>>)>) {
        let mut pending: Vec<(U, Item, O)> = Vec::new();
        for (unit, built) in updates {
            let old = self.items.remove(&unit).unwrap_or_default();
            self.warnings.remove(&unit);
            let Some(built) = built else {
                for id in old {
                    scene.remove(id);
                    self.objects.remove(&id);
                }
                continue;
            };
            if !built.warnings.is_empty() {
                self.warnings.insert(unit, built.warnings);
            }
            let mut ids = Vec::with_capacity(built.items.len());
            let mut old = old.into_iter();
            for (item, object) in built.items {
                match old.next() {
                    Some(id) => {
                        if scene.item(id) != Some(&item) {
                            scene.update(id, item);
                        }
                        self.objects.insert(id, object);
                        ids.push(id);
                    }
                    None => pending.push((unit, item, object)),
                }
            }
            for id in old {
                scene.remove(id);
                self.objects.remove(&id);
            }
            self.items.insert(unit, ids);
        }
        if pending.is_empty() {
            return;
        }
        let (units, (items, objects)): (Vec<U>, (Vec<Item>, Vec<O>)) =
            pending.into_iter().map(|(u, i, o)| (u, (i, o))).unzip();
        let ids = scene.extend(items);
        for ((unit, id), object) in units.into_iter().zip(ids).zip(objects) {
            self.items.entry(unit).or_default().push(id);
            self.objects.insert(id, object);
        }
    }
}

/// Dependencies of units on keys (e.g. model objects of other units),
/// with the reverse lookup.
#[derive(Debug)]
pub(crate) struct DepIndex<U, K> {
    forward: HashMap<U, Vec<K>>,
    reverse: HashMap<K, HashSet<U>>,
}

impl<U: Copy + Eq + Hash, K: Copy + Eq + Hash> DepIndex<U, K> {
    pub fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// Replaces the dependencies of a unit.
    pub fn set(&mut self, unit: U, keys: impl IntoIterator<Item = K>) {
        self.remove(unit);
        let mut keys: Vec<K> = keys.into_iter().collect();
        if keys.is_empty() {
            return;
        }
        let mut seen = HashSet::new();
        keys.retain(|k| seen.insert(*k));
        for key in &keys {
            self.reverse.entry(*key).or_default().insert(unit);
        }
        self.forward.insert(unit, keys);
    }

    /// Forgets the dependencies of a unit.
    pub fn remove(&mut self, unit: U) {
        for key in self.forward.remove(&unit).unwrap_or_default() {
            if let Some(set) = self.reverse.get_mut(&key) {
                set.remove(&unit);
                if set.is_empty() {
                    self.reverse.remove(&key);
                }
            }
        }
    }

    /// The keys a unit depends on.
    pub fn keys_of(&self, unit: &U) -> &[K] {
        self.forward.get(unit).map_or(&[], Vec::as_slice)
    }

    /// The units depending on a key.
    pub fn dependents(&self, key: &K) -> impl Iterator<Item = U> + '_ {
        self.reverse.get(key).into_iter().flatten().copied()
    }
}
