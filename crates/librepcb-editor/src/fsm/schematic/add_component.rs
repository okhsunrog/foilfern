//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_addcomponent.{h,cpp}.
//!
//! Differences to upstream: the "add component" dialog is requested from
//! the application ([`SchematicRequest::AddComponentDialog`]), which
//! passes the choice to [`SchematicEditorFsm::add_component()`]; the value
//! attribute editing of the tool bar (attribute value and unit) is not
//! ported, only the value.
//!
//! [`SchematicRequest::AddComponentDialog`]: super::SchematicRequest::AddComponentDialog
//! [`SchematicEditorFsm::add_component()`]: super::SchematicEditorFsm::add_component

use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::{ComponentInstanceId, SymbolId};
use librepcb_core::types::{Angle, Orientation, Point, Uuid};
use librepcb_i18n::tr;

use super::drag::DragSelection;
use super::{ComponentChoice, Cx, SchematicRequest, SchematicTool, State};
use crate::commands::{
    AddComponent, ApplyMutations, ComponentRef, EditComponent, MoveSymbol, PlaceSymbol,
};
use crate::fsm::{CursorShape, Features, PointerEvent};

/// A gate being placed.
#[derive(Debug)]
struct Placing {
    symbol: SymbolId,
    drag: DragSelection,
    /// Group length after adding the symbol.
    base: usize,
}

/// The add component state.
#[derive(Debug, Default)]
pub(crate) struct AddComponentState {
    component: Option<ComponentInstanceId>,
    choice: Option<ComponentChoice>,
    placing: Option<Placing>,
    angle: Angle,
    mirrored: bool,
    abort_after_current_gate: bool,
    abort_after_last_gate: bool,
}

impl AddComponentState {
    /// Starts adding a component (upstream `processAddComponent(cmp,
    /// symbVar)`).
    pub fn start(&mut self, cx: &mut Cx<'_, '_>, choice: ComponentChoice) -> bool {
        if !self.abort_command(cx) {
            return false;
        }
        self.angle = Angle::DEG0;
        self.mirrored = false;
        cx.out.tool_data.value.clear();
        self.abort_after_current_gate = false;
        self.abort_after_last_gate = false;
        match self.start_adding_component(cx, choice, false) {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    /// Starts placing the remaining gates of a component (upstream
    /// `processAddRemainingGates()`).
    pub fn start_remaining_gates(
        &mut self,
        cx: &mut Cx<'_, '_>,
        component: ComponentInstanceId,
        gate: Option<Uuid>,
    ) -> bool {
        if !self.abort_command(cx) {
            return false;
        }
        let Some(value) = cx
            .project()
            .circuit()
            .component_instance(component)
            .map(|c| c.value().clone())
        else {
            return false;
        };
        if let Err(e) = cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_AddComponent",
            "Add Component Gate to Schematic"
        )) {
            cx.error(e);
            return false;
        }
        self.component = Some(component);
        self.choice = None;
        self.angle = Angle::DEG0;
        self.mirrored = false;
        cx.out.tool_data.value = value;
        self.abort_after_current_gate = gate.is_some();
        self.abort_after_last_gate = true;
        let pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        match self.start_adding_next_gate(cx, pos, gate) {
            Ok(true) => true,
            Ok(false) => {
                self.abort_command(cx);
                false
            }
            Err(e) => {
                cx.error(e);
                self.abort_command(cx);
                false
            }
        }
    }

    /// Upstream `startAddingComponent()`.
    fn start_adding_component(
        &mut self,
        cx: &mut Cx<'_, '_>,
        choice: ComponentChoice,
        keep_value: bool,
    ) -> crate::Result<()> {
        cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_AddComponent",
            "Add Component to Schematic"
        ))?;
        let result = (|| -> crate::Result<()> {
            let added = cx.ctx.editor.execute(AddComponent {
                component: choice.component,
                symbol_variant: choice.symbol_variant,
                name: None,
                value: None,
                device: choice.device,
                place: None,
            })?;
            self.component = Some(added.component);
            self.choice = Some(choice.clone());
            if !keep_value {
                cx.out.tool_data.value = cx
                    .project()
                    .circuit()
                    .component_instance(added.component)
                    .map(|c| c.value().clone())
                    .unwrap_or_default();
            }
            self.update_value_suggestions(cx);
            if !keep_value {
                self.update_value_attribute(cx);
            }
            let pos = cx.cursor_pos().mapped_to_grid(cx.grid());
            if !self.start_adding_next_gate(cx, pos, None)? {
                return Err(crate::Error::ComponentWithoutSymbols(choice.component));
            }
            // A schematic frame added as the first symbol is placed at
            // (0, 0) and the tool is left.
            let p = cx.project();
            let is_frame = cx.sch().is_some_and(|s| s.symbols().len() == 1)
                && p.library().component(&choice.component).is_some_and(|c| {
                    c.schematic_only()
                        && c.metadata()
                            .name()
                            .as_str()
                            .to_lowercase()
                            .contains("frame")
                        && c.symbol_variants()
                            .iter()
                            .find(|v| {
                                p.circuit()
                                    .component_instance(added.component)
                                    .is_some_and(|ci| ci.lib_variant() == v.uuid())
                            })
                            .is_some_and(|v| v.symbol_items().len() == 1)
                });
            if is_frame && let Some(placing) = self.placing.take() {
                cx.ctx.editor.rollback_group_to(placing.base);
                cx.ctx.editor.execute(MoveSymbol {
                    symbol: placing.symbol,
                    position: Some(Point::ORIGIN),
                    rotation: Some(Angle::DEG0),
                    mirrored: Some(false),
                })?;
                cx.ctx.editor.commit_group()?;
                self.component = None;
                cx.out.requests.push(SchematicRequest::ZoomAll);
                cx.out.leave_requested = true;
            }
            Ok(())
        })();
        if result.is_err() && cx.ctx.editor.undo_stack().is_group_active() {
            let _ = cx.ctx.editor.abort_group();
            self.placing = None;
            self.component = None;
        }
        result
    }

    /// Adds the next unplaced gate at `pos` (upstream
    /// `startAddingNextGate()`); returns `false` if all gates are placed.
    fn start_adding_next_gate(
        &mut self,
        cx: &mut Cx<'_, '_>,
        pos: Point,
        gate: Option<Uuid>,
    ) -> crate::Result<bool> {
        let Some(component) = self.component else {
            return Ok(false);
        };
        let p = cx.project();
        let Some(instance) = p.circuit().component_instance(component) else {
            return Ok(false);
        };
        let placed = p
            .component_uses(component)
            .map(|u| u.symbols.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let items: Vec<Uuid> = p
            .library()
            .component(&instance.lib_component())
            .and_then(|c| c.symbol_variants().by_uuid(&instance.lib_variant()))
            .map(|v| v.symbol_items().uuids())
            .unwrap_or_default();
        let next = match gate {
            Some(g) if !placed.contains(&g) && items.contains(&g) => Some(g),
            _ => items.into_iter().find(|g| !placed.contains(g)),
        };
        let Some(gate) = next else {
            return Ok(false);
        };
        let placed = cx.ctx.editor.execute(PlaceSymbol {
            component: ComponentRef::Id(component),
            gate: Some(gate),
            schematic: Some(cx.schematic),
            position: pos,
            rotation: Angle::DEG0,
            mirrored: false,
        })?;
        // Upstream `setRotation()` / `setMirrored()` of the edit command.
        cx.ctx.editor.execute(MoveSymbol {
            symbol: placed.symbol,
            position: None,
            rotation: Some(self.angle),
            mirrored: Some(self.mirrored),
        })?;
        let base = cx.ctx.editor.active_group_len().unwrap_or(0);
        let symbol = cx
            .sch()
            .and_then(|s| s.symbols().get(&placed.symbol))
            .cloned()
            .ok_or_else(|| crate::Error::not_found("Symbol", placed.symbol))?;
        self.placing = Some(Placing {
            symbol: placed.symbol,
            drag: DragSelection::for_symbol(cx.schematic, symbol),
            base,
        });
        self.apply(cx);
        Ok(true)
    }

    /// Replaces the preview of the placed symbol (and the value).
    fn apply(&self, cx: &mut Cx<'_, '_>) {
        let Some(placing) = &self.placing else {
            return;
        };
        cx.ctx.editor.rollback_group_to(placing.base);
        let mutations = placing.drag.mutations(cx.project());
        if let Err(e) = cx.ctx.editor.execute(ApplyMutations {
            text: None,
            mutations,
        }) {
            log::warn!("Failed to move the symbol: {e}");
        }
        self.apply_value(cx);
    }

    /// Upstream `applyValueAndAttributeToComponent()`.
    fn apply_value(&self, cx: &mut Cx<'_, '_>) {
        let Some(component) = self.component else {
            return;
        };
        let value = cx.out.tool_data.value.clone();
        let Some(current) = cx.project().circuit().component_instance(component) else {
            return;
        };
        let mut attributes = current.attributes().clone();
        if let Some(attr) = &cx.out.tool_data.value_attribute
            && let Some(index) = attributes.index_of_name(attr.key().as_str(), true)
            && let Some(existing) = attributes.get_mut(index)
        {
            *existing = attr.clone();
        }
        let value_changed = current.value() != &value;
        let attributes_changed = current.attributes() != &attributes;
        if (value_changed || attributes_changed)
            && let Err(e) = cx.ctx.editor.execute(EditComponent {
                component: ComponentRef::Id(component),
                name: None,
                value: value_changed.then_some(value),
                attributes: attributes_changed.then_some(attributes),
                assembly_options: None,
                lock_assembly: None,
            })
        {
            log::warn!("Failed to set the component value: {e}");
        }
    }

    /// The first attribute of the component referenced by the value
    /// (upstream `setValue()`: only the first one, see
    /// LibrePCB-Libraries/LibrePCB_Base.lplib#138).
    fn update_value_attribute(&self, cx: &mut Cx<'_, '_>) {
        let attribute = self
            .component
            .and_then(|c| cx.project().circuit().component_instance(c))
            .and_then(|c| {
                let mut first = None;
                librepcb_core::attribute::substitute(
                    &cx.out.tool_data.value,
                    |key| {
                        if first.is_none() {
                            first = c.attributes().by_name(key, true).cloned();
                        }
                        None
                    },
                    None,
                );
                first
            });
        cx.out.tool_data.value_attribute = attribute;
    }

    /// The value of the value attribute was changed in the tool bar
    /// (upstream `setValueAttributeValue()`; invalid values are ignored).
    pub fn value_attribute_value_changed(&mut self, cx: &mut Cx<'_, '_>, value: &str) {
        if let Some(attr) = &mut cx.out.tool_data.value_attribute
            && attr.value() != value
            && attr.attribute_type().is_value_valid(value)
        {
            let (t, unit) = (attr.attribute_type(), attr.unit());
            let _ = attr.set_type_value_unit(t, value, unit);
        }
        self.apply(cx);
    }

    /// The unit of the value attribute was changed in the tool bar
    /// (upstream `setValueAttributeUnit()`).
    pub fn value_attribute_unit_changed(
        &mut self,
        cx: &mut Cx<'_, '_>,
        unit: Option<&'static librepcb_core::attribute::AttributeUnit>,
    ) {
        if let Some(attr) = &mut cx.out.tool_data.value_attribute
            && attr.unit() != unit
            && attr.attribute_type().is_unit_available(unit)
        {
            let (t, value) = (attr.attribute_type(), attr.value().to_owned());
            let _ = attr.set_type_value_unit(t, value, unit);
        }
        self.apply(cx);
    }

    fn update_value_suggestions(&self, cx: &mut Cx<'_, '_>) {
        let suggestions = self
            .component
            .and_then(|c| cx.project().circuit().component_instance(c))
            .map(|c| {
                c.attributes()
                    .iter()
                    .map(|a| format!("{{{{{}}}}}", a.key().as_str()))
                    .collect()
            })
            .unwrap_or_default();
        cx.out.tool_data.value_suggestions = suggestions;
    }

    /// The value was changed in the tool bar.
    pub fn value_changed(&mut self, cx: &mut Cx<'_, '_>) {
        self.update_value_suggestions(cx);
        self.update_value_attribute(cx);
        self.apply(cx);
    }

    /// Remembers the orientation of the placed symbol for the next gates.
    fn sync_orientation(&mut self, cx: &Cx<'_, '_>) {
        if let Some(symbol) = self
            .placing
            .as_ref()
            .and_then(|p| cx.sch()?.symbols().get(&p.symbol))
        {
            self.angle = symbol.rotation();
            self.mirrored = symbol.mirrored();
        }
    }

    /// Upstream `abortCommand()`.
    fn abort_command(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.placing = None;
        self.component = None;
        if cx.ctx.editor.undo_stack().is_group_active()
            && let Err(e) = cx.ctx.editor.abort_group()
        {
            cx.error(e);
            return false;
        }
        true
    }

    /// Places the current gate (upstream left click).
    fn place(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        let grid = cx.grid();
        let Some(placing) = &mut self.placing else {
            return false;
        };
        let pos = pos.mapped_to_grid(grid);
        placing.drag.set_current_position(pos, Some(grid));
        self.apply(cx);
        self.placing = None;
        let result = (|| -> crate::Result<()> {
            cx.ctx.editor.commit_group()?;
            cx.ctx.editor.begin_group(tr!(
                "librepcb::editor::SchematicEditorState_AddComponent",
                "Add Symbol to Schematic"
            ))?;
            if self.abort_after_current_gate {
                self.abort_command(cx);
                cx.out.leave_requested = true;
            } else if !self.start_adding_next_gate(cx, pos, None)? {
                // All gates placed: add the next component of this kind.
                let choice = self.choice.clone();
                self.abort_command(cx);
                match choice {
                    Some(choice) if !self.abort_after_last_gate => {
                        self.start_adding_component(cx, choice, true)?;
                    }
                    _ => cx.out.leave_requested = true,
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            cx.error(e);
            self.abort_command(cx);
        }
        true
    }
}

impl State for AddComponentState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.angle = Angle::DEG0;
        self.mirrored = false;
        cx.out.tool_data.value.clear();
        cx.out.tool = SchematicTool::Component;
        cx.out.view.features = Features {
            rotate: true,
            mirror: true,
            ..Features::default()
        };
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        cx.out.view.features = Features::default();
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort_command(cx);
        // The FSM leaves this state.
        false
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        let grid = cx.grid();
        let Some(placing) = &mut self.placing else {
            return false;
        };
        placing.drag.rotate(angle, false, grid);
        self.apply(cx);
        self.sync_orientation(cx);
        true
    }

    fn mirror(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        let grid = cx.grid();
        let Some(placing) = &mut self.placing else {
            return false;
        };
        placing.drag.mirror(orientation, false, grid);
        self.apply(cx);
        self.sync_orientation(cx);
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        let grid = cx.grid();
        let Some(placing) = &mut self.placing else {
            return false;
        };
        let old = placing.drag.delta();
        placing
            .drag
            .set_current_position(e.pos.mapped_to_grid(grid), Some(grid));
        if placing.drag.delta() != old {
            self.apply(cx);
        }
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.place(cx, e.pos)
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.place(cx, e.pos)
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_>, _e: PointerEvent) -> bool {
        // Always accept the event while placing.
        self.rotate(cx, Angle::DEG90)
    }
}
