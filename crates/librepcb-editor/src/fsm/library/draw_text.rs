//! Port of libs/librepcb/editor/library/sym/fsm/symboleditorstate_drawtextbase.{h,cpp}
//! and libs/librepcb/editor/library/pkg/fsm/packageeditorstate_drawtextbase.{h,cpp}
//! (with the derived add names, add values and draw text states).

use librepcb_core::types::{
    Alignment, Angle, Layer, Orientation, Point, PositiveLength, UnsignedLength,
};

use super::{Cx, ElementHost, LibraryTool, LibraryToolData, State, TextMode};
use crate::fsm::{CursorShape, Features, PointerEvent};
use crate::library_editor::commands::Transformable;

/// The properties of a text being placed (upstream `mCurrentProperties`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextProps {
    /// Layer.
    pub layer: Layer,
    /// Text.
    pub text: String,
    /// Rotation.
    pub rotation: Angle,
    /// Height.
    pub height: PositiveLength,
    /// Stroke width (stroke texts).
    pub stroke_width: UnsignedLength,
    /// Alignment.
    pub align: Alignment,
    /// Locked (symbol texts).
    pub locked: bool,
    /// Mirrored (stroke texts).
    pub mirrored: bool,
}

/// The text placing state.
#[derive(Debug)]
pub(crate) struct DrawTextState<H: ElementHost> {
    mode: TextMode,
    props: TextProps,
    /// The text being placed and its start position; an undo group is
    /// open.
    current: Option<(H::Item, Point)>,
}

impl<H: ElementHost> DrawTextState<H> {
    pub(crate) fn new(mode: TextMode) -> Self {
        Self {
            mode,
            props: H::text_defaults(mode),
            current: None,
        }
    }

    fn tool(&self) -> LibraryTool {
        match self.mode {
            TextMode::Name => LibraryTool::AddNames,
            TextMode::Value => LibraryTool::AddValues,
            TextMode::Text => LibraryTool::DrawText,
        }
    }

    fn write_tool_data(&self, cx: &mut Cx<'_, '_, H>) {
        let d = &mut cx.out.tool_data;
        d.layer = self.props.layer;
        d.available_layers = H::text_layers();
        d.text = self.props.text.clone();
        d.text_suggestions = H::text_suggestions(self.mode);
        d.height = self.props.height;
        d.stroke_width = self.props.stroke_width;
        d.h_align = self.props.align.h;
        d.v_align = self.props.align.v;
    }

    /// Upstream `startAddText()`.
    fn start(&mut self, cx: &mut Cx<'_, '_, H>, pos: Point) -> bool {
        if !cx.begin(H::add_text("text")) {
            return false;
        }
        let obj = H::new_text(&self.props, pos);
        let item = cx.modify(|e, fpt| H::add_object(e, fpt, obj));
        match item {
            Some(Ok(item)) => {
                self.current = Some((item, pos));
                cx.out.selection = [item].into_iter().collect();
                true
            }
            Some(Err(e)) => {
                cx.error(e);
                cx.abort_group();
                false
            }
            None => {
                cx.abort_group();
                false
            }
        }
    }

    /// Applies `f` to the text being placed.
    fn edit(&mut self, cx: &mut Cx<'_, '_, H>, f: impl FnOnce(&mut H::Object)) -> bool {
        let Some((item, _)) = self.current else {
            return false;
        };
        let Some(mut obj) = H::object(cx.element(), cx.fpt(), item) else {
            return false;
        };
        f(&mut obj);
        let (rotation, align, _) = H::text_properties(&obj);
        self.props.rotation = rotation;
        self.props.align = align;
        cx.modify(|e, fpt| H::update_object(e, fpt, obj));
        true
    }

    fn set_position(&mut self, cx: &mut Cx<'_, '_, H>, pos: Point) {
        self.edit(cx, |obj| {
            let old = obj.positions().first().copied().unwrap_or_default();
            obj.translate(pos - old);
        });
    }

    /// Upstream `finishAddText()`.
    fn finish(&mut self, cx: &mut Cx<'_, '_, H>, pos: Point) -> bool {
        let Some((item, start)) = self.current else {
            return false;
        };
        if pos == start {
            return self.abort_command(cx);
        }
        self.set_position(cx, pos);
        self.current = None;
        cx.out.selection.remove(&item);
        cx.commit()
    }

    /// Upstream `abortAddText()`.
    fn abort_command(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if let Some((item, _)) = self.current.take() {
            cx.out.selection.remove(&item);
        }
        cx.abort_group()
    }
}

impl<H: ElementHost> State<H> for DrawTextState<H> {
    fn entry(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if self.mode != TextMode::Text {
            self.props = H::text_defaults(self.mode);
        }
        let pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        if !self.start(cx, pos) {
            return false;
        }
        cx.out.tool = self.tool();
        self.write_tool_data(cx);
        cx.out.view.features = Features {
            rotate: true,
            mirror: true,
            ..Features::default()
        };
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, H>) -> bool {
        if self.current.is_some() && !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        cx.out.view.features = Features::default();
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        if self.current.is_some() {
            let pos = e.pos.mapped_to_grid(cx.grid());
            self.set_position(cx, pos);
            true
        } else {
            false
        }
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, H>, e: PointerEvent) -> bool {
        let pos = e.pos.mapped_to_grid(cx.grid());
        if self.current.is_some() {
            self.finish(cx, pos);
        }
        self.start(cx, pos)
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_, H>, _e: PointerEvent) -> bool {
        self.rotate(cx, Angle::DEG90)
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_, H>, angle: Angle) -> bool {
        let done = self.edit(cx, |obj| {
            let center = obj.positions().first().copied().unwrap_or_default();
            obj.rotate(angle, center);
        });
        self.write_tool_data(cx);
        done
    }

    fn mirror(&mut self, cx: &mut Cx<'_, '_, H>, orientation: Orientation) -> bool {
        let done = self.edit(cx, |obj| {
            let center = obj.positions().first().copied().unwrap_or_default();
            obj.mirror_geometry(orientation, center);
        });
        self.write_tool_data(cx);
        done
    }

    fn tool_data_changed(&mut self, cx: &mut Cx<'_, '_, H>, _old: &LibraryToolData) {
        let d = cx.out.tool_data.clone();
        self.props.layer = d.layer;
        self.props.text = d.text.clone();
        self.props.height = d.height;
        self.props.stroke_width = d.stroke_width;
        self.props.align = d.alignment();
        if let Some((item, _)) = self.current {
            let props = self.props.clone();
            if let Some(obj) = H::object(cx.element(), cx.fpt(), item) {
                let pos = obj.positions().first().copied().unwrap_or_default();
                let mut new = H::new_text(&props, pos);
                H::set_object_uuid(&mut new, &obj);
                cx.modify(|e, fpt| H::update_object(e, fpt, new));
            }
        }
        self.write_tool_data(cx);
    }
}
