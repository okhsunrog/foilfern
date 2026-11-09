//! Port of libs/librepcb/editor/project/schematic/fsm/schematiceditorstate_addimage.{h,cpp}.
//!
//! The image is added when the tool starts (with its file, in the group
//! "Add Schematic Image") and follows the cursor; the first click fixes
//! its position, then the size follows the cursor (keeping the aspect
//! ratio) until the second click, which finishes the tool.
//!
//! Differences to upstream: the image chooser dialog is requested from the
//! application ([`SchematicRequest::ChooseImageFile`]), which passes the
//! chosen data to [`SchematicEditorFsm::add_image()`](super::SchematicEditorFsm::add_image);
//! the file name is not asked for (see [`crate::commands::image`]).

use librepcb_core::geometry::Image;
use librepcb_core::project::{Mutation, SchematicMutation};
use librepcb_core::types::{Angle, Length, Point, PositiveLength, Uuid};
use librepcb_i18n::tr;

use super::{Cx, ImageData, SchematicTool, State};
use crate::commands::image::{find_existing_image_file, unused_image_file_name};
use crate::commands::{AddSchematicImage, ApplyMutations};
use crate::fsm::{CursorShape, Features, PointerEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Positioning,
    Resizing,
}

/// The image being added.
#[derive(Debug, Clone)]
struct Placing {
    /// The original image (as added).
    image: Image,
    /// The current state.
    current: Image,
    aspect_ratio: f64,
    /// Group length after adding the image.
    base: usize,
    phase: Phase,
}

/// The add image state.
#[derive(Debug, Default)]
pub(crate) struct AddImageState {
    placing: Option<Placing>,
}

impl AddImageState {
    /// Adds the image at `pos` (upstream `start()`).
    pub fn start(&mut self, cx: &mut Cx<'_, '_>, data: ImageData) -> bool {
        self.abort_command(cx);
        let pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        match self.try_start(cx, data, pos) {
            Ok(()) => {
                cx.out.view.features = Features {
                    rotate: true,
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

    fn try_start(&mut self, cx: &mut Cx<'_, '_>, data: ImageData, pos: Point) -> crate::Result<()> {
        let format = data.format.to_lowercase();
        let (px_w, px_h) =
            Image::try_load(&data.data, &format).map_err(crate::Error::InvalidArgument)?;
        let existing = find_existing_image_file(cx.project(), cx.schematic, &data.data)?;
        let file_name = match &existing {
            Some(name) => name.clone(),
            None => unused_image_file_name(cx.project(), cx.schematic, &data.basename, &format)?,
        };
        let range = |e: librepcb_core::types::Error| crate::Error::InvalidArgument(e.to_string());
        let mut width = Length::from_px(px_w).map_err(range)?;
        let mut height = Length::from_px(px_h).map_err(range)?;
        let initial = Length::new(10_000_000);
        if width > height {
            height =
                Length::from_mm(height.to_mm() * initial.to_mm() / width.to_mm()).map_err(range)?;
            width = initial;
        } else {
            width =
                Length::from_mm(width.to_mm() * initial.to_mm() / height.to_mm()).map_err(range)?;
            height = initial;
        }
        let width = PositiveLength::new(width).map_err(range)?;
        let height = PositiveLength::new(height).map_err(range)?;
        let image = Image::new(
            Uuid::new_random(),
            file_name,
            pos,
            Angle::DEG0,
            width,
            height,
            None,
        );
        cx.ctx.editor.begin_group(tr!(
            "librepcb::editor::SchematicEditorState_AddImage",
            "Add Schematic Image"
        ))?;
        cx.ctx.editor.execute(AddSchematicImage {
            schematic: cx.schematic,
            image: image.clone(),
            data: existing.is_none().then_some(data.data),
        })?;
        let base = cx.ctx.editor.active_group_len().unwrap_or(0);
        cx.out.selection.clear();
        cx.out
            .selection
            .insert(super::SchematicItem::Image(image.uuid()));
        self.placing = Some(Placing {
            aspect_ratio: width.to_mm() / height.to_mm(),
            current: image.clone(),
            image,
            base,
            phase: Phase::Positioning,
        });
        Ok(())
    }

    /// Replaces the preview by the current state of the image.
    fn apply(&self, cx: &mut Cx<'_, '_>) {
        let Some(p) = &self.placing else {
            return;
        };
        cx.ctx.editor.rollback_group_to(p.base);
        if p.current != p.image
            && let Err(e) = cx.ctx.editor.execute(ApplyMutations {
                text: None,
                mutations: vec![Mutation::Schematic(SchematicMutation::UpdateImage {
                    schematic: cx.schematic,
                    image: p.current.clone(),
                })],
            })
        {
            log::warn!("Failed to update the image: {e}");
        }
    }

    /// Upstream `updateSize()`.
    fn update_size(&mut self, pos: Point) {
        let Some(p) = &mut self.placing else {
            return;
        };
        let img = &p.current;
        let rel = pos.rotated(-img.rotation(), img.position()) - img.position();
        let width = rel.x;
        let Ok(height) = Length::from_mm(width.to_mm() / p.aspect_ratio) else {
            return;
        };
        if let (Ok(w), Ok(h)) = (PositiveLength::new(width), PositiveLength::new(height)) {
            p.current.set_width(w);
            p.current.set_height(h);
        }
    }

    /// Upstream `finish()`.
    fn finish(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        if self
            .placing
            .as_ref()
            .is_some_and(|p| p.current.position() == pos)
        {
            cx.out.leave_requested = true;
            return;
        }
        self.update_size(pos);
        self.apply(cx);
        self.placing = None;
        cx.out.view.features = Features::default();
        if let Err(e) = cx.ctx.editor.commit_group() {
            cx.error(e);
        }
        // Usually only one image is added.
        cx.out.leave_requested = true;
    }

    fn rotate_image(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        let Some(p) = &mut self.placing else {
            return false;
        };
        let rotation = p.current.rotation() + angle;
        p.current.set_rotation(rotation);
        self.apply(cx);
        true
    }

    fn abort_command(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.out.view.features = Features::default();
        if self.placing.take().is_some()
            && cx.ctx.editor.undo_stack().is_group_active()
            && let Err(e) = cx.ctx.editor.abort_group()
        {
            cx.error(e);
            return false;
        }
        true
    }

    fn snapped(cx: &Cx<'_, '_>, e: PointerEvent) -> Point {
        if e.modifiers.shift {
            e.pos
        } else {
            e.pos.mapped_to_grid(cx.grid())
        }
    }
}

impl State for AddImageState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        cx.out.tool = SchematicTool::Image;
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
        if self
            .placing
            .as_ref()
            .is_some_and(|p| p.phase == Phase::Positioning)
        {
            return self.rotate_image(cx, angle);
        }
        false
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        let pos = Self::snapped(cx, e);
        let Some(p) = &mut self.placing else {
            return false;
        };
        match p.phase {
            Phase::Positioning => {
                p.current.set_position(pos);
            }
            Phase::Resizing => {
                if pos == p.current.position() {
                    return true;
                }
                self.update_size(pos);
            }
        }
        self.apply(cx);
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_>, e: PointerEvent) -> bool {
        let pos = Self::snapped(cx, e);
        let Some(p) = &mut self.placing else {
            return false;
        };
        match p.phase {
            Phase::Positioning => {
                p.current.set_position(pos);
                p.phase = Phase::Resizing;
                self.apply(cx);
            }
            Phase::Resizing => self.finish(cx, pos),
        }
        true
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_>, _e: PointerEvent) -> bool {
        // Always accept the event while placing an image.
        self.rotate_image(cx, Angle::DEG90)
    }
}
