//! The symbol editor: [`SymbolHost`] (the symbol specific part of the
//! library editor FSM, upstream `SymbolEditorState::getAllowed*Layers()`
//! and the defaults of the states) and the add pins state (port of
//! libs/librepcb/editor/library/sym/fsm/symboleditorstate_addpins.{h,cpp}).

use std::collections::BTreeSet;

use librepcb_core::geometry::{CircleList, Path, PolygonList, Text};
use librepcb_core::library::sym::{Symbol, SymbolPin};
use librepcb_core::types::{
    Alignment, Angle, CircuitIdentifier, HAlign, Layer, Length, Orientation, Point, UnsignedLength,
    Uuid, VAlign,
};
use librepcb_i18n::tr;

use super::draw_text::TextProps;
use super::{
    Cx, ElementHost, LibraryContext, LibraryEditorFsm, LibraryRequest, LibraryTool,
    LibraryToolData, PolygonMode, State, TextMode, len, plen,
};
use crate::error::Result;
use crate::fsm::measure::{snap_candidates_from_circle, snap_candidates_from_path};
use crate::fsm::{Clipboard, CursorShape, Features, PointerEvent};
use crate::library_editor::ElementCommand;
use crate::library_editor::commands::{
    AddSymbolObject, ItemContainer, PasteSymbolItems, RemoveSymbolItems, SymbolClipboardData,
    SymbolItem, SymbolObject, UpdateSymbolObject, all_symbol_items, import_pins_data,
    symbol_clipboard_mime_type,
};

/// The symbol specific part of the library editor FSM.
#[derive(Debug, Clone, Copy, Default)]
pub struct SymbolHost;

/// The symbol editor FSM (upstream `SymbolEditorFsm`).
pub type SymbolEditorFsm = LibraryEditorFsm<SymbolHost>;

/// The context of a symbol editor FSM call.
pub type SymbolContext<'a> = LibraryContext<'a, SymbolHost>;

/// The symbol specific states.
#[derive(Debug, Default)]
pub struct SymbolStates {
    add_pins: AddPinsState,
}

impl ElementHost for SymbolHost {
    type Element = Symbol;
    type Item = SymbolItem;
    type Object = SymbolObject;
    type States = SymbolStates;
    type ClipboardData = SymbolClipboardData;

    const IS_FOOTPRINT: bool = false;

    fn polygons(e: &Symbol, _fpt: Option<Uuid>) -> Option<&PolygonList> {
        Some(e.polygons())
    }

    fn polygons_mut(e: &mut Symbol, _fpt: Option<Uuid>) -> Option<&mut PolygonList> {
        Some(e.polygons_mut())
    }

    fn circles(e: &Symbol, _fpt: Option<Uuid>) -> Option<&CircleList> {
        Some(e.circles())
    }

    fn circles_mut(e: &mut Symbol, _fpt: Option<Uuid>) -> Option<&mut CircleList> {
        Some(e.circles_mut())
    }

    fn polygon_item(uuid: Uuid) -> SymbolItem {
        SymbolItem::Polygon(uuid)
    }

    fn circle_item(uuid: Uuid) -> SymbolItem {
        SymbolItem::Circle(uuid)
    }

    fn item_path(e: &Symbol, _fpt: Option<Uuid>, item: SymbolItem) -> Option<Path> {
        match item {
            SymbolItem::Polygon(u) => e.polygons().by_uuid(&u).map(|p| p.path().clone()),
            _ => None,
        }
    }

    fn set_item_path(e: &mut Symbol, _fpt: Option<Uuid>, item: SymbolItem, path: Path) {
        if let SymbolItem::Polygon(u) = item
            && let Some(p) = e.polygons_mut().by_uuid_mut(&u)
        {
            p.set_path(path);
        }
    }

    fn item_exists(e: &Symbol, _fpt: Option<Uuid>, item: SymbolItem) -> bool {
        item.exists_in(e)
    }

    fn all_items(e: &Symbol, _fpt: Option<Uuid>) -> BTreeSet<SymbolItem> {
        all_symbol_items(e)
    }

    fn container(e: &Symbol, _fpt: Option<Uuid>) -> Option<&dyn ItemContainer<SymbolItem>> {
        Some(e)
    }

    fn container_mut(
        e: &mut Symbol,
        _fpt: Option<Uuid>,
    ) -> Option<&mut dyn ItemContainer<SymbolItem>> {
        Some(e)
    }

    fn remove_items(e: &mut Symbol, _fpt: Option<Uuid>, items: &BTreeSet<SymbolItem>) {
        // Removing never fails (missing items are ignored).
        let _ = RemoveSymbolItems {
            items: items.clone(),
        }
        .execute(e);
    }

    fn copy_items(
        e: &Symbol,
        _fpt: Option<Uuid>,
        items: &BTreeSet<SymbolItem>,
        cursor_pos: Point,
        app_version: &str,
    ) -> Result<Option<(String, Vec<u8>)>> {
        let data = SymbolClipboardData::from_items(e, items, cursor_pos)?;
        if data.item_count() == 0 {
            return Ok(None);
        }
        Ok(Some((
            symbol_clipboard_mime_type(app_version),
            data.to_zip()?,
        )))
    }

    fn clipboard_data(
        clipboard: &dyn Clipboard,
        app_version: &str,
    ) -> Result<Option<(SymbolClipboardData, Point)>> {
        match clipboard.get(&symbol_clipboard_mime_type(app_version)) {
            Some(zip) => {
                let data = SymbolClipboardData::from_zip(&zip)?;
                let pos = data.cursor_pos;
                Ok(Some((data, pos)))
            }
            None => Ok(None),
        }
    }

    fn paste(
        e: &mut Symbol,
        _fpt: Option<Uuid>,
        data: SymbolClipboardData,
        offset: Point,
    ) -> Result<BTreeSet<SymbolItem>> {
        PasteSymbolItems { data, offset }.execute(e)
    }

    fn image(
        e: &Symbol,
        _fpt: Option<Uuid>,
        item: SymbolItem,
    ) -> Option<librepcb_core::geometry::Image> {
        match item {
            SymbolItem::Image(u) => e.images().by_uuid(&u).cloned(),
            _ => None,
        }
    }

    fn set_image(e: &mut Symbol, _fpt: Option<Uuid>, image: librepcb_core::geometry::Image) {
        if let Some(i) = e.images_mut().by_uuid_mut(&image.uuid()) {
            *i = image;
        }
    }

    fn object(e: &Symbol, _fpt: Option<Uuid>, item: SymbolItem) -> Option<SymbolObject> {
        SymbolObject::from_symbol(e, item)
    }

    fn add_object(e: &mut Symbol, _fpt: Option<Uuid>, obj: SymbolObject) -> Result<SymbolItem> {
        AddSymbolObject(obj).execute(e)
    }

    fn update_object(e: &mut Symbol, _fpt: Option<Uuid>, obj: SymbolObject) -> Result<()> {
        UpdateSymbolObject(obj).execute(e)
    }

    fn new_text(p: &TextProps, pos: Point) -> SymbolObject {
        SymbolObject::Text(Text::new(
            Uuid::new_random(),
            p.layer,
            p.text.clone(),
            pos,
            p.rotation,
            p.height,
            p.align,
            p.locked,
        ))
    }

    fn set_object_uuid(obj: &mut SymbolObject, from: &SymbolObject) {
        if let (SymbolObject::Text(t), SymbolObject::Text(f)) = (&*obj, from) {
            *obj = SymbolObject::Text(t.with_uuid(f.uuid()));
        }
    }

    fn text_properties(obj: &SymbolObject) -> (Angle, Alignment, bool) {
        match obj {
            SymbolObject::Text(t) => (t.rotation(), t.align(), false),
            _ => (
                Angle::DEG0,
                Alignment::new(HAlign::Left, VAlign::Bottom),
                false,
            ),
        }
    }

    fn polygon_layers() -> Vec<Layer> {
        // Upstream getAllowedCircleAndPolygonLayers().
        vec![
            Layer::SYMBOL_OUTLINES,
            Layer::SYMBOL_HIDDEN_GRAB_AREAS,
            Layer::SYMBOL_NAMES,
            Layer::SYMBOL_VALUES,
            Layer::SCHEMATIC_SHEET_FRAMES,
            Layer::SCHEMATIC_DOCUMENTATION,
            Layer::SCHEMATIC_COMMENTS,
            Layer::SCHEMATIC_GUIDE,
        ]
    }

    fn text_layers() -> Vec<Layer> {
        // Upstream getAllowedTextLayers().
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

    fn polygon_defaults(mode: PolygonMode) -> (Layer, UnsignedLength, bool, bool) {
        let grab_area = !matches!(mode, PolygonMode::Line | PolygonMode::Arc);
        (Layer::SYMBOL_OUTLINES, len(200_000), false, grab_area)
    }

    fn circle_defaults() -> (Layer, UnsignedLength, bool, bool) {
        (Layer::SYMBOL_OUTLINES, len(200_000), false, true)
    }

    fn text_defaults(mode: TextMode) -> TextProps {
        let (layer, text, v, locked) = match mode {
            TextMode::Name => (Layer::SYMBOL_NAMES, "{{NAME}}", VAlign::Bottom, false),
            TextMode::Value => (Layer::SYMBOL_VALUES, "{{VALUE}}", VAlign::Top, false),
            // Non-empty to avoid an invisible item.
            TextMode::Text => (Layer::SYMBOL_OUTLINES, "Text", VAlign::Bottom, true),
        };
        TextProps {
            layer,
            text: text.to_owned(),
            rotation: Angle::DEG0,
            height: plen(2_500_000),
            stroke_width: UnsignedLength::default(),
            align: Alignment::new(HAlign::Left, v),
            locked,
            mirrored: false,
        }
    }

    fn text_suggestions(mode: TextMode) -> Vec<String> {
        if mode != TextMode::Text {
            return Vec::new();
        }
        [
            "{{NAME}}",
            "{{VALUE}}",
            "{{SHEET}}",
            "{{PROJECT}}",
            "{{DATE}}",
            "{{TIME}}",
            "{{AUTHOR}}",
            "{{VERSION}}",
            "{{PAGE_X_OF_Y}}",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn measure_snap_candidates(e: &Symbol, _fpt: Option<Uuid>) -> BTreeSet<Point> {
        // Upstream MeasureTool::snapCandidatesFromSymbol().
        let mut c = BTreeSet::new();
        for p in e.pins().iter() {
            c.insert(p.position());
            c.insert(
                p.position()
                    + Point::new(*p.length(), Length::ZERO).rotated(p.rotation(), Point::ORIGIN),
            );
        }
        for p in e.polygons().iter() {
            c.extend(snap_candidates_from_path(p.path()));
        }
        for circle in e.circles().iter() {
            c.extend(snap_candidates_from_circle(
                circle.center(),
                circle.diameter(),
            ));
        }
        for t in e.texts().iter() {
            c.insert(t.position());
        }
        c
    }

    fn add_text(kind: &str) -> String {
        match kind {
            "polygon" => tr!(
                "librepcb::editor::SymbolEditorState_DrawPolygonBase",
                "Add symbol polygon"
            ),
            "circle" => tr!(
                "librepcb::editor::SymbolEditorState_DrawCircle",
                "Add symbol circle"
            ),
            _ => tr!(
                "librepcb::editor::SymbolEditorState_DrawTextBase",
                "Add symbol text"
            ),
        }
    }

    fn paste_text() -> String {
        tr!(
            "librepcb::editor::SymbolEditorState_Select",
            "Paste Symbol Elements"
        )
    }

    fn state(states: &mut SymbolStates, tool: LibraryTool) -> Option<&mut dyn State<Self>> {
        match tool {
            LibraryTool::AddPins => Some(&mut states.add_pins),
            _ => None,
        }
    }
}

/// The add pins state (upstream `SymbolEditorState_AddPins`).
#[derive(Debug)]
struct AddPinsState {
    /// Properties of the next pin (upstream `mCurrentProperties`).
    props: SymbolPin,
    /// The pin being placed; an undo group is open.
    current: Option<Uuid>,
}

impl Default for AddPinsState {
    fn default() -> Self {
        let length = len(2_540_000); // Default length according library conventions.
        Self {
            props: SymbolPin::new(
                Uuid::new_random(),
                CircuitIdentifier::new("1").unwrap_or_else(|_| unreachable!()),
                Point::ORIGIN,
                length,
                Angle::DEG0,
                SymbolPin::default_name_position(length),
                Angle::DEG0,
                SymbolPin::default_name_height(),
                SymbolPin::default_name_alignment(),
            ),
            current: None,
        }
    }
}

impl AddPinsState {
    fn write_tool_data(&self, cx: &mut Cx<'_, '_, SymbolHost>) {
        cx.out.tool_data.pin_name = self.props.name().as_str().to_owned();
        cx.out.tool_data.pin_length = self.props.length();
    }

    /// Upstream `determineNextPinName()`.
    fn next_pin_name(symbol: &Symbol) -> CircuitIdentifier {
        let mut i = 1;
        while symbol.pins().contains_name(&i.to_string()) {
            i += 1;
        }
        CircuitIdentifier::new(i.to_string()).unwrap_or_else(|_| unreachable!())
    }

    /// Upstream `addNextPin()`.
    fn add_next_pin(&mut self, cx: &mut Cx<'_, '_, SymbolHost>, pos: Point) -> bool {
        if !cx.begin(tr!(
            "librepcb::editor::SymbolEditorState_AddPins",
            "Add symbol pin"
        )) {
            return false;
        }
        self.props.set_name(Self::next_pin_name(cx.element()));
        self.props.set_position(pos);
        let pin = self.props.with_uuid(Uuid::new_random());
        let uuid = pin.uuid();
        match cx.modify(|e, _| AddSymbolObject(SymbolObject::Pin(pin)).execute(e)) {
            Some(Ok(_)) => {
                self.current = Some(uuid);
                cx.out.selection = [SymbolItem::Pin(uuid)].into_iter().collect();
                self.write_tool_data(cx);
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

    /// Modifies the pin being placed.
    fn edit(&mut self, cx: &mut Cx<'_, '_, SymbolHost>, f: impl FnOnce(&mut SymbolPin)) {
        let Some(uuid) = self.current else {
            return;
        };
        let pin = cx.modify(|e, _| {
            let pin = e.pins_mut().by_uuid_mut(&uuid)?;
            f(pin);
            Some(pin.clone())
        });
        if let Some(Some(pin)) = pin {
            self.props.set_rotation(pin.rotation());
        }
    }
}

impl State<SymbolHost> for AddPinsState {
    fn entry(&mut self, cx: &mut Cx<'_, '_, SymbolHost>) -> bool {
        let pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        if !self.add_next_pin(cx, pos) {
            return false;
        }
        cx.out.tool = LibraryTool::AddPins;
        cx.out.view.features = Features {
            rotate: true,
            mirror: true,
            ..Features::default()
        };
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, SymbolHost>) -> bool {
        if let Some(uuid) = self.current.take() {
            cx.out.selection.remove(&SymbolItem::Pin(uuid));
        }
        if !cx.abort_group() {
            return false;
        }
        cx.set_cursor(None);
        cx.out.view.features = Features::default();
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, SymbolHost>, e: PointerEvent) -> bool {
        let pos = e.pos.mapped_to_grid(cx.grid());
        self.edit(cx, |p| {
            p.set_position(pos);
        });
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, SymbolHost>, e: PointerEvent) -> bool {
        let pos = e.pos.mapped_to_grid(cx.grid());
        self.edit(cx, |p| {
            p.set_position(pos);
        });
        if let Some(uuid) = self.current.take() {
            cx.out.selection.remove(&SymbolItem::Pin(uuid));
        }
        if !cx.commit() {
            return false;
        }
        self.add_next_pin(cx, pos)
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_, SymbolHost>, _e: PointerEvent) -> bool {
        State::<SymbolHost>::rotate(self, cx, Angle::DEG90)
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_, SymbolHost>, angle: Angle) -> bool {
        use crate::library_editor::commands::Transformable;
        self.edit(cx, |p| {
            let center = p.position();
            p.rotate(angle, center);
        });
        true
    }

    fn mirror(&mut self, cx: &mut Cx<'_, '_, SymbolHost>, orientation: Orientation) -> bool {
        use crate::library_editor::commands::Transformable;
        self.edit(cx, |p| {
            let center = p.position();
            p.mirror_geometry(orientation, center);
        });
        true
    }

    fn tool_data_changed(&mut self, cx: &mut Cx<'_, '_, SymbolHost>, old: &LibraryToolData) {
        let d = cx.out.tool_data.clone();
        if d.pin_name != old.pin_name {
            match CircuitIdentifier::new(d.pin_name.clone()) {
                Ok(name) => {
                    self.props.set_name(name.clone());
                    self.edit(cx, |p| {
                        p.set_name(name);
                    });
                }
                Err(e) => cx.error(e),
            }
        }
        if d.pin_length != old.pin_length {
            let length = d.pin_length;
            self.props.set_length(length);
            self.props
                .set_name_position(SymbolPin::default_name_position(length));
            self.edit(cx, |p| {
                p.set_length(length);
                p.set_name_position(SymbolPin::default_name_position(length));
            });
        }
        self.write_tool_data(cx);
    }
}

impl LibraryEditorFsm<SymbolHost> {
    /// Starts adding pins (upstream `processStartAddingSymbolPins()`); with
    /// `import`, requests the "import pins" dialog
    /// ([`LibraryRequest::ImportPinsDialog`]), the application then calls
    /// [`import_pins()`](Self::import_pins).
    pub fn start_adding_pins(&mut self, ctx: &mut SymbolContext<'_>, import: bool) -> bool {
        if !self.set_tool(ctx, LibraryTool::AddPins) {
            return false;
        }
        if import {
            self.with_host_state(ctx, |_, _, cx| {
                cx.out.requests.push(LibraryRequest::ImportPinsDialog);
            });
        }
        true
    }

    /// Adds pins with the given names below each other (upstream
    /// `processImportPins()`): they are pasted in the select tool and
    /// follow the cursor until the next click.
    pub fn import_pins(
        &mut self,
        ctx: &mut SymbolContext<'_>,
        names: &[CircuitIdentifier],
    ) -> bool {
        if names.is_empty() {
            return true;
        }
        let template = self.with_host_state(ctx, |states, _, _| states.add_pins.props.clone());
        let data = import_pins_data(ctx.editor.element().metadata_uuid(), names, &template);
        if !self.select_tool(ctx) {
            return false;
        }
        self.with_select_state(ctx, |s, cx| {
            s.start_paste_data(cx, (data, Point::ORIGIN), None)
        })
        .unwrap_or(false)
    }

    /// Adds an image (upstream `SymbolEditorState_AddImage` with the data
    /// of the image chooser dialog or the clipboard): it follows the cursor
    /// (10 mm on its longer side) until a click places it, then its size
    /// follows the cursor until the next click. The file is stored in the
    /// symbol directory, reusing an existing file with the same content;
    /// its name is derived from `data.basename` (upstream asks for it).
    pub fn add_image(
        &mut self,
        ctx: &mut SymbolContext<'_>,
        data: crate::fsm::schematic::ImageData,
    ) -> bool {
        if !self.select_tool(ctx) {
            return false;
        }
        self.with_select_state(ctx, |s, cx| {
            match image_clipboard_data(cx.element().metadata_uuid(), data) {
                Ok(data) => s.start_adding_image(
                    cx,
                    tr!(
                        "librepcb::editor::SymbolEditorState_AddImage",
                        "Add Symbol Image"
                    ),
                    (data, Point::ORIGIN),
                ),
                Err(e) => {
                    cx.error(e);
                    false
                }
            }
        })
        .unwrap_or(false)
    }

    /// Imports a DXF file (upstream `processImportDxf()`, select tool
    /// only): its polygons and circles become polygons on the chosen layer
    /// (`circles_as_drills` does not apply to symbols), which are pasted
    /// like clipboard data (following the cursor unless a placement
    /// position is given).
    pub fn import_dxf(
        &mut self,
        ctx: &mut SymbolContext<'_>,
        settings: &crate::fsm::board::DxfImportSettings,
    ) -> bool {
        self.with_select_state(ctx, |s, cx| {
            let result = crate::fsm::read_dxf_import(settings).map(|(paths, circles)| {
                let mut data =
                    SymbolClipboardData::new(cx.element().metadata_uuid(), Point::ORIGIN);
                let polygon = |path| {
                    librepcb_core::geometry::Polygon::new(
                        Uuid::new_random(),
                        settings.layer,
                        settings.line_width,
                        false,
                        false,
                        path,
                    )
                };
                for path in paths {
                    data.polygons.push(polygon(path));
                }
                for circle in &circles {
                    data.polygons.push(polygon(
                        Path::circle(circle.diameter).translated(circle.position),
                    ));
                }
                data
            });
            match result {
                Ok(data) => s.start_paste_data(cx, (data, Point::ORIGIN), settings.placement),
                Err(e) => {
                    cx.error(e);
                    false
                }
            }
        })
        .unwrap_or(false)
    }
}

/// Clipboard data with one image of `data`, 10 mm on its longer side, at
/// the origin (upstream `SymbolEditorState_AddImage::start()`).
fn image_clipboard_data(
    symbol: Uuid,
    data: crate::fsm::schematic::ImageData,
) -> Result<SymbolClipboardData> {
    use librepcb_core::geometry::Image;
    use librepcb_core::types::{FileProofName, PositiveLength};
    let format = data.format.to_lowercase();
    let (px_w, px_h) =
        Image::try_load(&data.data, &format).map_err(crate::Error::InvalidArgument)?;
    let range = |e: librepcb_core::types::Error| crate::Error::InvalidArgument(e.to_string());
    let mut width = Length::from_px(px_w).map_err(range)?;
    let mut height = Length::from_px(px_h).map_err(range)?;
    let initial = Length::new(10_000_000);
    if width > height {
        height =
            Length::from_mm(height.to_mm() * initial.to_mm() / width.to_mm()).map_err(range)?;
        width = initial;
    } else {
        width = Length::from_mm(width.to_mm() * initial.to_mm() / height.to_mm()).map_err(range)?;
        height = initial;
    }
    // The final file name is determined when pasting (existing file with
    // the same content, or an unused name).
    let mut base = FileProofName::clean(data.basename.trim());
    if base.is_empty() {
        base = "image".to_owned();
    }
    let file_name = FileProofName::new(format!("{base}.{format}"))
        .or_else(|_| FileProofName::new(format!("image.{format}")))
        .map_err(|e| crate::Error::InvalidArgument(e.to_string()))?;
    let image = Image::new(
        Uuid::new_random(),
        file_name.clone(),
        Point::ORIGIN,
        Angle::DEG0,
        PositiveLength::new(width).map_err(range)?,
        PositiveLength::new(height).map_err(range)?,
        None,
    );
    let mut clipboard = SymbolClipboardData::new(symbol, Point::ORIGIN);
    clipboard.images.push(image);
    clipboard.files.insert(file_name.to_string(), data.data);
    Ok(clipboard)
}

/// UUID access without importing the element traits.
trait MetadataUuid {
    fn metadata_uuid(&self) -> Uuid;
}

impl MetadataUuid for Symbol {
    fn metadata_uuid(&self) -> Uuid {
        use librepcb_core::library::LibraryBaseElement;
        self.metadata().uuid()
    }
}
