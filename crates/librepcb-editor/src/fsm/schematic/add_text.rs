//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_addtext.{h,cpp}.

use librepcb_core::geometry::Text;
use librepcb_core::project::SchematicMutation;
use librepcb_core::types::{Alignment, Angle, HAlign, Layer, Orientation, Point, Uuid, VAlign};
use librepcb_i18n::tr;

use super::drag::DragSelection;
use super::{Cx, SchematicTool, State};
use crate::commands::ApplyMutations;
use crate::fsm::{CursorShape, Features, PointerEvent};

/// Layers allowed for schematic geometry (upstream
/// `getAllowedGeometryLayers()`).
pub(crate) fn allowed_geometry_layers() -> Vec<Layer> {
    vec![
        Layer::SYMBOL_OUTLINES,
        Layer::SYMBOL_NAMES,
        Layer::SYMBOL_VALUES,
        Layer::SCHEMATIC_SHEET_FRAMES,
        Layer::SCHEMATIC_DOCUMENTATION,
        Layer::SCHEMATIC_COMMENTS,
        Layer::SCHEMATIC_GUIDE,
    ]
}

/// The add text state.
#[derive(Debug)]
pub(crate) struct AddTextState {
    /// Rotation and alignment of the next text (upstream
    /// `mCurrentProperties`; layer, height and text are the tool data).
    rotation: Angle,
    align: Alignment,
    initialized: bool,
    /// The text being placed and the group length after adding it.
    placing: Option<(DragSelection, usize, Uuid)>,
}

impl Default for AddTextState {
    fn default() -> Self {
        Self {
            rotation: Angle::DEG0,
            align: Alignment::new(HAlign::Left, VAlign::Bottom),
            initialized: false,
            placing: None,
        }
    }
}

impl AddTextState {
    fn apply(&self, cx: &mut Cx<'_, '_>) {
        let Some((drag, base, _)) = &self.placing else {
            return;
        };
        cx.ctx.editor.rollback_group_to(*base);
        let mutations = drag.mutations(cx.project());
        if let Err(e) = cx.ctx.editor.execute(ApplyMutations {
            text: None,
            mutations,
        }) {
            log::warn!("Failed to move the text: {e}");
        }
    }

    /// Upstream `addText()`.
    fn add_text(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        let data = &cx.out.tool_data;
        let text = Text::new(
            Uuid::new_random(),
            data.layer,
            data.value.clone(),
            pos,
            self.rotation,
            data.size,
            self.align,
            false,
        );
        let uuid = text.uuid();
        let result = cx
            .ctx
            .editor
            .begin_group(tr!(
                "librepcb::editor::SchematicEditorState_AddText",
                "Add text to schematic"
            ))
            .and_then(|()| {
                cx.ctx.editor.execute(ApplyMutations {
                    text: None,
                    mutations: vec![librepcb_core::project::Mutation::Schematic(
                        SchematicMutation::AddText {
                            schematic: cx.schematic,
                            text: text.clone(),
                        },
                    )],
                })
            });
        match result {
            Ok(()) => {
                let base = cx.ctx.editor.active_group_len().unwrap_or(0);
                self.placing = Some((DragSelection::for_text(cx.schematic, text), base, uuid));
                true
            }
            Err(e) => {
                cx.error(e);
                self.abort_command(cx);
                false
            }
        }
    }

    /// Upstream `fixPosition()`.
    fn fix_position(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        if let Some((drag, _, _)) = &mut self.placing {
            drag.set_current_position(pos, None);
        }
        self.apply(cx);
        self.placing = None;
        match cx.ctx.editor.commit_group() {
            Ok(_) => true,
            Err(e) => {
                cx.error(e);
                self.abort_command(cx);
                false
            }
        }
    }

    fn abort_command(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.placing = None;
        if cx.ctx.editor.undo_stack().is_group_active()
            && let Err(e) = cx.ctx.editor.abort_group()
        {
            cx.error(e);
            return false;
        }
        true
    }

    /// Remembers rotation and alignment of the placed text for the next
    /// one (upstream `mCurrentProperties = ...getTextObj()`).
    fn sync_properties(&mut self, cx: &Cx<'_, '_>) {
        if let Some((_, _, uuid)) = &self.placing
            && let Some(text) = cx.sch().and_then(|s| s.texts().get(uuid))
        {
            self.rotation = text.rotation();
            self.align = text.align();
        }
    }

    /// Layer, height or text changed in the tool bar: the text being
    /// placed is replaced with the new properties.
    pub fn properties_changed(&mut self, cx: &mut Cx<'_, '_>) {
        let Some((drag, base, uuid)) = self.placing.take() else {
            return;
        };
        let pos = drag.current_pos();
        cx.ctx.editor.rollback_group_to(0);
        let _ = base;
        let data = &cx.out.tool_data;
        let text = Text::new(
            uuid,
            data.layer,
            data.value.clone(),
            pos,
            self.rotation,
            data.size,
            self.align,
            false,
        );
        match cx.ctx.editor.execute(ApplyMutations {
            text: None,
            mutations: vec![librepcb_core::project::Mutation::Schematic(
                SchematicMutation::AddText {
                    schematic: cx.schematic,
                    text: text.clone(),
                },
            )],
        }) {
            Ok(()) => {
                let base = cx.ctx.editor.active_group_len().unwrap_or(0);
                self.placing = Some((DragSelection::for_text(cx.schematic, text), base, uuid));
            }
            Err(e) => cx.error(e),
        }
    }
}

impl State for AddTextState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        if !self.initialized {
            // Upstream defaults: comments layer, 2.5 mm, "{{PROJECT}}".
            self.initialized = true;
            cx.out.tool_data.layer = Layer::SCHEMATIC_COMMENTS;
            cx.out.tool_data.size = super::default_text_height();
            cx.out.tool_data.value = "{{PROJECT}}".to_owned();
        }
        cx.out.tool_data.available_layers = allowed_geometry_layers();
        cx.out.tool_data.value_suggestions = [
            "{{SHEET}}",
            "{{PAGE_X_OF_Y}}",
            "{{PROJECT}}",
            "{{AUTHOR}}",
            "{{VERSION}}",
            "{{DATE}}",
            "{{TIME}}",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        if !self.add_text(cx, pos) {
            return false;
        }
        cx.out.tool = SchematicTool::Text;
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

    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        let grid = cx.grid();
        let Some((drag, _, _)) = &mut self.placing else {
            return false;
        };
        drag.rotate(angle, false, grid);
        self.apply(cx);
        self.sync_properties(cx);
        true
    }

    fn mirror(&mut self, cx: &mut Cx<'_, '_>, orientation: Orientation) -> bool {
        let grid = cx.grid();
        let Some((drag, _, _)) = &mut self.placing else {
            return false;
        };
        drag.mirror(orientation, false, grid);
        self.apply(cx);
        self.sync_properties(cx);
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        let grid = cx.grid();
        let Some((drag, _, _)) = &mut self.placing else {
            return false;
        };
        let old = drag.delta();
        drag.set_current_position(e.pos.mapped_to_grid(grid), None);
        if drag.delta() != old {
            self.apply(cx);
        }
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        let pos = e.pos.mapped_to_grid(cx.grid());
        self.fix_position(cx, pos);
        self.add_text(cx, pos);
        true
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        self.left_pressed(cx, e)
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_>, _e: PointerEvent) -> bool {
        self.rotate(cx, Angle::DEG90);
        self.placing.is_some()
    }
}
