//! The "find" feature of the schematic editor: port of the find handling
//! of libs/librepcb/editor/project/schematic/schematictab.{h,cpp}
//! (`FindRefreshSuggestions`, `FindNext`, `FindPrevious`,
//! `goToObjects()`), see [`crate::fsm::find`].
//!
//! Differences to upstream: component candidates are the symbol names
//! (with gate suffix, like upstream) and are resolved through the symbols
//! of the page, so symbols of multi-gate components are found too
//! (upstream looks the symbol names up as component names).

use std::collections::BTreeSet;

use librepcb_core::project::ComponentInstanceId;
use librepcb_core::types::Point;

use super::{SchematicContext, SchematicEditorFsm, SchematicItem, StateKind};
use crate::fsm::find::{
    FindCandidate, FindKind, FindResult, SearchContext, net_candidates, resolve_candidates,
    zoom_rect,
};

impl SchematicEditorFsm {
    /// The search state (term and suggestions) of the "find" field.
    pub fn search(&self) -> &SearchContext {
        &self.search
    }

    /// Sets the search term of the "find" field.
    pub fn set_find_term(&mut self, term: &str) {
        self.search.set_term(term);
    }

    /// Refreshes the suggestions: the symbols of the page (except pure
    /// schematic-only components like frames) and the named nets (upstream
    /// `FindRefreshSuggestions`).
    pub fn refresh_find_suggestions(&mut self, ctx: &SchematicContext<'_>) {
        let p = ctx.editor.project();
        let mut candidates = Vec::new();
        if let Some(s) = p.schematic(self.schematic) {
            for symbol in s.symbols().values() {
                let Ok(resolved) = symbol.resolve(p.view()) else {
                    continue;
                };
                if resolved
                    .component
                    .is_pure_schematic_only(resolved.lib_component)
                {
                    continue;
                }
                if let Ok(name) = symbol.name(p.view()) {
                    candidates.push(FindCandidate::component(name));
                }
            }
        }
        candidates.extend(net_candidates(p));
        self.search.set_candidates(candidates);
    }

    /// Goes to the next match (upstream `FindNext`): selects it and returns
    /// the rectangle to zoom to.
    pub fn find_next(&mut self, ctx: &mut SchematicContext<'_>) -> FindResult {
        let objects = self.search.find_next();
        self.go_to_objects(ctx, &objects)
    }

    /// Goes to the previous match (upstream `FindPrevious`).
    pub fn find_previous(&mut self, ctx: &mut SchematicContext<'_>) -> FindResult {
        let objects = self.search.find_previous();
        self.go_to_objects(ctx, &objects)
    }

    /// Selects the symbols of the found components and the pins, net lines
    /// and net labels of the found nets, and returns the zoom rectangle
    /// (upstream `SchematicTab::goToObjects()`). Only has an effect in the
    /// select tool.
    pub fn go_to_objects(
        &mut self,
        ctx: &mut SchematicContext<'_>,
        objects: &[FindCandidate],
    ) -> FindResult {
        let p = ctx.editor.project();
        let (mut components, nets) = resolve_candidates(p, objects);
        let mut items: BTreeSet<SchematicItem> = BTreeSet::new();
        let mut points: Vec<Point> = Vec::new();
        if let Some(s) = p.schematic(self.schematic) {
            // Symbol names (with gate suffix) of this page.
            let names: BTreeSet<&str> = objects
                .iter()
                .filter(|o| o.kind == FindKind::Component)
                .map(|o| o.name.as_str())
                .collect();
            for symbol in s.symbols().values() {
                if symbol
                    .name(p.view())
                    .is_ok_and(|n| names.contains(n.as_str()))
                {
                    components.insert(symbol.component());
                }
            }
            for (id, symbol) in s.symbols() {
                if components.contains(&symbol.component()) {
                    items.insert(SchematicItem::Symbol(*id));
                    points.push(symbol.position());
                }
                for pin in symbol.pins(p.view()).unwrap_or_default() {
                    if pin.net().is_some_and(|n| nets.contains(&n)) {
                        items.insert(SchematicItem::SymbolPin(*id, pin.uuid()));
                        points.push(pin.position());
                    }
                }
            }
            for (id, seg) in s.net_segments() {
                if !nets.contains(&seg.net()) {
                    continue;
                }
                for line in seg.lines().values() {
                    items.insert(SchematicItem::NetLine(*id, line.uuid()));
                    for anchor in [line.p1(), line.p2()] {
                        if let Some(pos) = s.net_line_anchor_position(*id, anchor, p.view()) {
                            points.push(pos);
                        }
                    }
                }
                for label in seg.labels().values() {
                    items.insert(SchematicItem::NetLabel(*id, label.uuid()));
                    points.push(label.position());
                }
            }
        }
        let components: Vec<ComponentInstanceId> = components.into_iter().collect();
        let result = FindResult {
            objects: objects.to_vec(),
            components,
            nets: nets.into_iter().collect(),
            zoom_rect: if items.is_empty() {
                None
            } else {
                zoom_rect(points)
            },
        };
        if self.current == StateKind::Select {
            self.set_selection(items);
            // Update the info box and features for the new selection
            // (upstream `processChangedSelection()`).
            self.ensure_entered(ctx);
            self.after_event(ctx);
        }
        result
    }
}
