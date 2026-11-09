//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_addlabel.{h,cpp}
//! (net labels on net lines, bus labels on bus lines).

use librepcb_core::geometry::NetLabel;
use librepcb_core::types::{Angle, Orientation, Point};
use librepcb_i18n::tr;

use super::drag::DragSelection;
use super::hit_test::{FindFlags, find_items_at};
use super::{Cx, SchematicItem, SchematicTool, State};
use crate::commands::{AddBusLabel, AddNetLabel, ApplyMutations};
use crate::fsm::{CursorShape, Features, PointerEvent};

/// The add net label state.
#[derive(Debug, Default)]
pub(crate) struct AddLabelState {
    /// The label being placed (as a single item drag) and the group length
    /// after adding it.
    placing: Option<(DragSelection, usize)>,
}

impl AddLabelState {
    fn apply(&self, cx: &mut Cx<'_, '_>) {
        let Some((drag, base)) = &self.placing else {
            return;
        };
        cx.ctx.editor.rollback_group_to(*base);
        let mutations = drag.mutations(cx.project());
        if let Err(e) = cx.ctx.editor.execute(ApplyMutations {
            text: None,
            mutations,
        }) {
            log::warn!("Failed to move the net label: {e}");
        }
    }

    /// Starts placing a bus label on the bus line under the cursor.
    fn add_bus_label(
        &mut self,
        cx: &mut Cx<'_, '_>,
        segment: librepcb_core::project::BusSegmentId,
        pos: Point,
    ) -> bool {
        let pos = pos.mapped_to_grid(cx.grid());
        if let Err(e) = cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_AddLabel",
            "Add Bus Label to Schematic"
        )) {
            cx.error(e);
            return false;
        }
        match cx.ctx.editor.execute(AddBusLabel {
            segment,
            position: pos,
            rotation: Angle::DEG0,
            mirrored: false,
        }) {
            Ok(uuid) => {
                let base = cx.ctx.editor.active_group_len().unwrap_or(0);
                let label = NetLabel::new(uuid, pos, Angle::DEG0, false);
                self.placing = Some((
                    DragSelection::for_bus_label(cx.schematic, segment, label),
                    base,
                ));
                cx.out.view.features = Features {
                    rotate: true,
                    mirror: true,
                    ..Features::default()
                };
                true
            }
            Err(e) => {
                cx.error(e);
                self.abort_command(cx);
                false
            }
        }
    }

    /// Starts placing a label on the bus or net line under the cursor
    /// (upstream `addLabel()`).
    fn add_label(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        let bus_flags = FindFlags {
            bus_lines: true,
            ..FindFlags::NONE
        }
        .within_grid();
        if let Some(SchematicItem::BusLine(segment, _)) =
            find_items_at(cx, pos, bus_flags, &[]).into_iter().next()
        {
            return self.add_bus_label(cx, segment, pos);
        }
        let flags = FindFlags {
            net_lines: true,
            ..FindFlags::NONE
        }
        .within_grid();
        let Some(SchematicItem::NetLine(segment, _)) =
            find_items_at(cx, pos, flags, &[]).into_iter().next()
        else {
            return false;
        };
        let pos = pos.mapped_to_grid(cx.grid());
        if let Err(e) = cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_AddLabel",
            "Add Net Label to Schematic"
        )) {
            cx.error(e);
            return false;
        }
        match cx.ctx.editor.execute(AddNetLabel {
            segment,
            position: pos,
            rotation: Angle::DEG0,
            mirrored: false,
        }) {
            Ok(uuid) => {
                let base = cx.ctx.editor.active_group_len().unwrap_or(0);
                let label = NetLabel::new(uuid, pos, Angle::DEG0, false);
                self.placing = Some((DragSelection::for_label(cx.schematic, segment, label), base));
                cx.out.view.features = Features {
                    rotate: true,
                    mirror: true,
                    ..Features::default()
                };
                true
            }
            Err(e) => {
                cx.error(e);
                self.abort_command(cx);
                false
            }
        }
    }

    /// Places the label (upstream `fixLabel()`).
    fn fix_label(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        let grid = cx.grid();
        if let Some((drag, _)) = &mut self.placing {
            drag.set_current_position(pos, Some(grid));
        }
        self.apply(cx);
        self.placing = None;
        cx.out.view.features = Features::default();
        match cx.ctx.editor.commit_group() {
            Ok(_) => true,
            Err(e) => {
                cx.error(e);
                false
            }
        }
    }

    fn abort_command(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.out.view.features = Features::default();
        self.placing = None;
        if cx.ctx.editor.undo_stack().is_group_active()
            && let Err(e) = cx.ctx.editor.abort_group()
        {
            cx.error(e);
            return false;
        }
        true
    }
}

impl State for AddLabelState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.out.tool = SchematicTool::Label;
        cx.out.view.features = Features::default();
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        true
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        let grid = cx.grid();
        let Some((drag, _)) = &mut self.placing else {
            return false;
        };
        drag.rotate(angle, false, grid);
        self.apply(cx);
        true
    }

    fn mirror(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        let grid = cx.grid();
        let Some((drag, _)) = &mut self.placing else {
            return false;
        };
        drag.mirror(orientation, false, grid);
        self.apply(cx);
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        let grid = cx.grid();
        let Some((drag, _)) = &mut self.placing else {
            return false;
        };
        let old = drag.delta();
        drag.set_current_position(e.pos, Some(grid));
        if drag.delta() != old {
            self.apply(cx);
        }
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        if self.placing.is_some() {
            self.fix_label(cx, e.pos)
        } else {
            self.add_label(cx, e.pos)
        }
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.left_pressed(cx, e)
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_>, _e: PointerEvent) -> bool {
        // Always accept the event while placing a label.
        self.rotate(cx, Angle::DEG90)
    }
}
