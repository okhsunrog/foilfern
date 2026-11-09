//! The package editor: [`PackageHost`] (the footprint specific part of the
//! library editor FSM, upstream `PackageEditorState::getAllowed*Layers()`
//! and the defaults of the states) and the package specific states (port
//! of libs/librepcb/editor/library/pkg/fsm/
//! {packageeditorstate_addpads,packageeditorstate_addholes,
//! packageeditorstate_drawzone,packageeditorstate_renumberpads}.{h,cpp}).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{
    CircleList, ComponentSide, Hole, NonEmptyPath, Pad, PadFunction, PadHole, PadHoleList,
    PadShape, Path, PolygonList, StrokeText, Vertex, Zone,
};
use librepcb_core::library::pkg::{Footprint, FootprintPad, Package};
use librepcb_core::types::{
    Alignment, Angle, HAlign, Layer, Length, MaskConfig, Point, PositiveLength, Ratio,
    StrokeTextSpacing, UnsignedLength, UnsignedLimitedRatio, Uuid, VAlign,
};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::draw_text::TextProps;
use super::{
    Cx, ElementHost, LibraryContext, LibraryEditorFsm, LibraryRequest, LibraryTool,
    LibraryToolData, PolygonMode, State, TextMode, format_angle, format_length, len, plen,
};
use crate::error::Result;
use crate::fsm::measure::{snap_candidates_from_circle, snap_candidates_from_path};
use crate::fsm::{
    Clipboard, CursorShape, Features, Key, KeyEvent, Modifiers, PointerEvent, SceneCursor,
};
use crate::library_editor::ElementCommand;
use crate::library_editor::commands::{
    AddFootprintObject, FootprintClipboardData, FootprintItem, FootprintObject, GenerateCourtyard,
    GeneratePackageOutline, ItemContainer, PasteFootprintItems, RemoveFootprintItems,
    UpdateFootprintObject, all_footprint_items, footprint_clipboard_mime_type,
};

/// The footprint specific part of the library editor FSM.
#[derive(Debug, Clone, Copy, Default)]
pub struct PackageHost;

/// The package editor FSM (upstream `PackageEditorFsm`).
pub type PackageEditorFsm = LibraryEditorFsm<PackageHost>;

/// The context of a package editor FSM call.
pub type PackageContext<'a> = LibraryContext<'a, PackageHost>;

/// The kind of pads added by the add pads tool (upstream
/// `PackageEditorState_AddPads::PadType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadType {
    /// Through-hole pads.
    Tht,
    /// Surface mount pads.
    Smt,
}

fn footprint(e: &Package, fpt: Option<Uuid>) -> Option<&Footprint> {
    e.footprints().by_uuid(&fpt?)
}

/// The single object of clipboard data whose geometry can be pasted onto
/// other objects (upstream `canPasteGeometry()`: exactly one object).
fn geometry_source(data: &FootprintClipboardData) -> Option<FootprintItem> {
    let count = data.pads.len()
        + data.polygons.len()
        + data.circles.len()
        + data.stroke_texts.len()
        + data.zones.len()
        + data.holes.len();
    if count != 1 {
        return None;
    }
    data.pads
        .iter()
        .map(|p| FootprintItem::Pad(p.uuid()))
        .chain(
            data.polygons
                .iter()
                .map(|p| FootprintItem::Polygon(p.uuid())),
        )
        .chain(data.circles.iter().map(|c| FootprintItem::Circle(c.uuid())))
        .chain(
            data.stroke_texts
                .iter()
                .map(|t| FootprintItem::StrokeText(t.uuid())),
        )
        .chain(data.zones.iter().map(|z| FootprintItem::Zone(z.uuid())))
        .chain(data.holes.iter().map(|h| FootprintItem::Hole(h.uuid())))
        .next()
}

fn footprint_mut(e: &mut Package, fpt: Option<Uuid>) -> Option<&mut Footprint> {
    e.footprints_mut().by_uuid_mut(&fpt?)
}

/// The package specific states.
#[derive(Debug)]
pub struct PackageStates {
    tht_pads: AddPadsState,
    smt_pads: BTreeMap<PadFunction, AddPadsState>,
    holes: AddHolesState,
    zone: DrawZoneState,
    renumber: RenumberPadsState,
}

impl Default for PackageStates {
    fn default() -> Self {
        let smt_pads = [
            PadFunction::StandardPad,
            PadFunction::ThermalPad,
            PadFunction::BgaPad,
            PadFunction::EdgeConnectorPad,
            PadFunction::TestPad,
            PadFunction::LocalFiducial,
            PadFunction::GlobalFiducial,
        ]
        .into_iter()
        .map(|f| (f, AddPadsState::new(PadType::Smt, f)))
        .collect();
        Self {
            tht_pads: AddPadsState::new(PadType::Tht, PadFunction::StandardPad),
            smt_pads,
            holes: AddHolesState::default(),
            zone: DrawZoneState::default(),
            renumber: RenumberPadsState::default(),
        }
    }
}

impl ElementHost for PackageHost {
    type Element = Package;
    type Item = FootprintItem;
    type Object = FootprintObject;
    type States = PackageStates;
    type ClipboardData = FootprintClipboardData;

    const IS_FOOTPRINT: bool = true;

    fn polygons(e: &Package, fpt: Option<Uuid>) -> Option<&PolygonList> {
        footprint(e, fpt).map(|f| f.polygons())
    }

    fn polygons_mut(e: &mut Package, fpt: Option<Uuid>) -> Option<&mut PolygonList> {
        footprint_mut(e, fpt).map(|f| f.polygons_mut())
    }

    fn circles(e: &Package, fpt: Option<Uuid>) -> Option<&CircleList> {
        footprint(e, fpt).map(|f| f.circles())
    }

    fn circles_mut(e: &mut Package, fpt: Option<Uuid>) -> Option<&mut CircleList> {
        footprint_mut(e, fpt).map(|f| f.circles_mut())
    }

    fn polygon_item(uuid: Uuid) -> FootprintItem {
        FootprintItem::Polygon(uuid)
    }

    fn circle_item(uuid: Uuid) -> FootprintItem {
        FootprintItem::Circle(uuid)
    }

    fn item_path(e: &Package, fpt: Option<Uuid>, item: FootprintItem) -> Option<Path> {
        let f = footprint(e, fpt)?;
        match item {
            FootprintItem::Polygon(u) => f.polygons().by_uuid(&u).map(|p| p.path().clone()),
            FootprintItem::Zone(u) => f.zones().by_uuid(&u).map(|z| z.outline().clone()),
            _ => None,
        }
    }

    fn set_item_path(e: &mut Package, fpt: Option<Uuid>, item: FootprintItem, path: Path) {
        let Some(f) = footprint_mut(e, fpt) else {
            return;
        };
        match item {
            FootprintItem::Polygon(u) => {
                if let Some(p) = f.polygons_mut().by_uuid_mut(&u) {
                    p.set_path(path);
                }
            }
            FootprintItem::Zone(u) => {
                if let Some(z) = f.zones_mut().by_uuid_mut(&u) {
                    z.set_outline(path);
                }
            }
            _ => {}
        }
    }

    fn item_exists(e: &Package, fpt: Option<Uuid>, item: FootprintItem) -> bool {
        footprint(e, fpt).is_some_and(|f| item.exists_in(f))
    }

    fn all_items(e: &Package, fpt: Option<Uuid>) -> BTreeSet<FootprintItem> {
        footprint(e, fpt)
            .map(all_footprint_items)
            .unwrap_or_default()
    }

    fn container(e: &Package, fpt: Option<Uuid>) -> Option<&dyn ItemContainer<FootprintItem>> {
        footprint(e, fpt).map(|f| f as &dyn ItemContainer<FootprintItem>)
    }

    fn container_mut(
        e: &mut Package,
        fpt: Option<Uuid>,
    ) -> Option<&mut dyn ItemContainer<FootprintItem>> {
        footprint_mut(e, fpt).map(|f| f as &mut dyn ItemContainer<FootprintItem>)
    }

    fn remove_items(e: &mut Package, fpt: Option<Uuid>, items: &BTreeSet<FootprintItem>) {
        if let Some(footprint) = fpt {
            // Removing never fails for existing footprints.
            let _ = RemoveFootprintItems {
                footprint,
                items: items.clone(),
            }
            .execute(e);
        }
    }

    fn copy_items(
        e: &Package,
        fpt: Option<Uuid>,
        items: &BTreeSet<FootprintItem>,
        cursor_pos: Point,
        app_version: &str,
    ) -> Result<Option<(String, Vec<u8>)>> {
        let Some(fpt) = fpt else {
            return Ok(None);
        };
        let data = FootprintClipboardData::from_items(e, fpt, items, cursor_pos)?;
        if data.item_count() == 0 {
            return Ok(None);
        }
        Ok(Some((
            footprint_clipboard_mime_type(app_version),
            data.to_bytes()?,
        )))
    }

    fn clipboard_data(
        clipboard: &dyn Clipboard,
        app_version: &str,
    ) -> Result<Option<(FootprintClipboardData, Point)>> {
        match clipboard.get(&footprint_clipboard_mime_type(app_version)) {
            Some(bytes) => {
                let data = FootprintClipboardData::from_bytes(&bytes)?;
                let pos = data.cursor_pos;
                Ok(Some((data, pos)))
            }
            None => Ok(None),
        }
    }

    fn paste(
        e: &mut Package,
        fpt: Option<Uuid>,
        data: FootprintClipboardData,
        offset: Point,
    ) -> Result<BTreeSet<FootprintItem>> {
        let Some(footprint) = fpt else {
            return Ok(BTreeSet::new());
        };
        PasteFootprintItems {
            footprint,
            data,
            offset,
        }
        .execute(e)
    }

    fn can_paste_geometry(
        _e: &Package,
        fpt: Option<Uuid>,
        data: &FootprintClipboardData,
        selection: &BTreeSet<FootprintItem>,
    ) -> bool {
        let Some(copied) = geometry_source(data) else {
            return false;
        };
        // Objects of the same kind other than the copied one must be
        // selected (so that a single object can still be copied & pasted).
        fpt.is_some()
            && selection.iter().any(|item| {
                std::mem::discriminant(item) == std::mem::discriminant(&copied)
                    && item.uuid() != copied.uuid()
            })
    }

    fn paste_geometry(
        e: &mut Package,
        fpt: Option<Uuid>,
        data: &FootprintClipboardData,
        selection: &BTreeSet<FootprintItem>,
    ) {
        let Some(f) = footprint_mut(e, fpt) else {
            return;
        };
        for item in selection {
            match *item {
                FootprintItem::Pad(u) => {
                    if let (Some(src), Some(dst)) =
                        (data.pads.iter().next(), f.pads_mut().by_uuid_mut(&u))
                    {
                        let (src, dst) = (src.pad(), dst.pad_mut());
                        dst.set_component_side(src.component_side());
                        dst.set_function(src.function());
                        dst.set_shape(src.shape());
                        dst.set_width(src.width());
                        dst.set_height(src.height());
                        dst.set_radius(src.radius());
                        dst.set_custom_shape_outline(src.custom_shape_outline().clone());
                        dst.set_stop_mask_config(src.stop_mask_config());
                        dst.set_solder_paste_config(src.solder_paste_config());
                        dst.set_copper_clearance(src.copper_clearance());
                        dst.set_holes(src.holes().clone());
                    }
                }
                FootprintItem::Polygon(u) => {
                    if let (Some(src), Some(dst)) = (
                        data.polygons.iter().next(),
                        f.polygons_mut().by_uuid_mut(&u),
                    ) {
                        dst.set_layer(src.layer());
                        dst.set_line_width(src.line_width());
                        dst.set_is_filled(src.is_filled());
                        dst.set_is_grab_area(src.is_grab_area());
                    }
                }
                FootprintItem::Circle(u) => {
                    if let (Some(src), Some(dst)) =
                        (data.circles.iter().next(), f.circles_mut().by_uuid_mut(&u))
                    {
                        dst.set_layer(src.layer());
                        dst.set_line_width(src.line_width());
                        dst.set_is_filled(src.is_filled());
                        dst.set_is_grab_area(src.is_grab_area());
                        dst.set_diameter(src.diameter());
                    }
                }
                FootprintItem::StrokeText(u) => {
                    if let (Some(src), Some(dst)) = (
                        data.stroke_texts.iter().next(),
                        f.stroke_texts_mut().by_uuid_mut(&u),
                    ) {
                        dst.set_layer(src.layer());
                        dst.set_height(src.height());
                        dst.set_stroke_width(src.stroke_width());
                        dst.set_letter_spacing(src.letter_spacing());
                        dst.set_line_spacing(src.line_spacing());
                    }
                }
                FootprintItem::Zone(u) => {
                    if let (Some(src), Some(dst)) =
                        (data.zones.iter().next(), f.zones_mut().by_uuid_mut(&u))
                    {
                        dst.set_layers(src.layers());
                        dst.set_rules(src.rules());
                    }
                }
                FootprintItem::Hole(u) => {
                    if let (Some(src), Some(dst)) =
                        (data.holes.iter().next(), f.holes_mut().by_uuid_mut(&u))
                    {
                        dst.set_diameter(src.diameter());
                        dst.set_stop_mask_config(src.stop_mask_config());
                    }
                }
            }
        }
    }

    fn object(e: &Package, fpt: Option<Uuid>, item: FootprintItem) -> Option<FootprintObject> {
        FootprintObject::from_footprint(footprint(e, fpt)?, item)
    }

    fn add_object(
        e: &mut Package,
        fpt: Option<Uuid>,
        obj: FootprintObject,
    ) -> Result<FootprintItem> {
        let footprint = fpt.ok_or_else(|| crate::Error::InvalidArgument("No footprint.".into()))?;
        AddFootprintObject {
            footprint,
            object: obj,
        }
        .execute(e)
    }

    fn update_object(e: &mut Package, fpt: Option<Uuid>, obj: FootprintObject) -> Result<()> {
        let footprint = fpt.ok_or_else(|| crate::Error::InvalidArgument("No footprint.".into()))?;
        UpdateFootprintObject {
            footprint,
            object: obj,
        }
        .execute(e)
    }

    fn new_text(p: &TextProps, pos: Point) -> FootprintObject {
        FootprintObject::StrokeText(StrokeText::new(
            Uuid::new_random(),
            p.layer,
            p.text.clone(),
            pos,
            p.rotation,
            p.height,
            p.stroke_width,
            StrokeTextSpacing::default(),
            StrokeTextSpacing::default(),
            p.align,
            p.mirrored,
            true,
            false,
        ))
    }

    fn set_object_uuid(obj: &mut FootprintObject, from: &FootprintObject) {
        if let (FootprintObject::StrokeText(t), FootprintObject::StrokeText(f)) = (&*obj, from) {
            *obj = FootprintObject::StrokeText(t.with_uuid(f.uuid()));
        }
    }

    fn text_properties(obj: &FootprintObject) -> (Angle, Alignment, bool) {
        match obj {
            FootprintObject::StrokeText(t) => (t.rotation(), t.align(), t.mirrored()),
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
            Layer::BOARD_SHEET_FRAMES,
            Layer::BOARD_OUTLINES,
            Layer::BOARD_CUTOUTS,
            Layer::BOARD_PLATED_CUTOUTS,
            Layer::BOARD_MEASURES,
            Layer::BOARD_ALIGNMENT,
            Layer::BOARD_DOCUMENTATION,
            Layer::BOARD_COMMENTS,
            Layer::BOARD_GUIDE,
            Layer::TOP_LEGEND,
            Layer::TOP_HIDDEN_GRAB_AREAS,
            Layer::TOP_DOCUMENTATION,
            Layer::TOP_PACKAGE_OUTLINES,
            Layer::TOP_NAMES,
            Layer::TOP_VALUES,
            Layer::TOP_COPPER,
            Layer::TOP_COURTYARD,
            Layer::TOP_GLUE,
            Layer::TOP_SOLDER_PASTE,
            Layer::TOP_STOP_MASK,
            Layer::BOT_LEGEND,
            Layer::BOT_HIDDEN_GRAB_AREAS,
            Layer::BOT_DOCUMENTATION,
            Layer::BOT_PACKAGE_OUTLINES,
            Layer::BOT_NAMES,
            Layer::BOT_VALUES,
            Layer::BOT_COPPER,
            Layer::BOT_COURTYARD,
            Layer::BOT_GLUE,
            Layer::BOT_SOLDER_PASTE,
            Layer::BOT_STOP_MASK,
        ]
    }

    fn text_layers() -> Vec<Layer> {
        // Upstream getAllowedTextLayers().
        vec![
            Layer::BOARD_SHEET_FRAMES,
            Layer::BOARD_OUTLINES,
            Layer::BOARD_CUTOUTS,
            Layer::BOARD_PLATED_CUTOUTS,
            Layer::BOARD_MEASURES,
            Layer::BOARD_ALIGNMENT,
            Layer::BOARD_DOCUMENTATION,
            Layer::BOARD_COMMENTS,
            Layer::BOARD_GUIDE,
            Layer::TOP_LEGEND,
            Layer::TOP_DOCUMENTATION,
            Layer::TOP_NAMES,
            Layer::TOP_VALUES,
            Layer::TOP_COPPER,
            Layer::TOP_COURTYARD,
            Layer::TOP_GLUE,
            Layer::TOP_SOLDER_PASTE,
            Layer::TOP_STOP_MASK,
            Layer::BOT_LEGEND,
            Layer::BOT_DOCUMENTATION,
            Layer::BOT_NAMES,
            Layer::BOT_VALUES,
            Layer::BOT_COPPER,
            Layer::BOT_COURTYARD,
            Layer::BOT_GLUE,
            Layer::BOT_SOLDER_PASTE,
            Layer::BOT_STOP_MASK,
        ]
    }

    fn polygon_defaults(_mode: PolygonMode) -> (Layer, UnsignedLength, bool, bool) {
        (Layer::TOP_LEGEND, len(200_000), false, false)
    }

    fn circle_defaults() -> (Layer, UnsignedLength, bool, bool) {
        (Layer::TOP_LEGEND, len(200_000), false, false)
    }

    fn text_defaults(mode: TextMode) -> TextProps {
        let (layer, text, height, h, v) = match mode {
            TextMode::Name => (
                Layer::TOP_NAMES,
                "{{NAME}}",
                1_000_000,
                HAlign::Center,
                VAlign::Bottom,
            ),
            TextMode::Value => (
                Layer::TOP_VALUES,
                "{{VALUE}}",
                1_000_000,
                HAlign::Center,
                VAlign::Top,
            ),
            // Non-empty to avoid an invisible item.
            TextMode::Text => (
                Layer::TOP_LEGEND,
                "Text",
                2_000_000,
                HAlign::Left,
                VAlign::Bottom,
            ),
        };
        TextProps {
            layer,
            text: text.to_owned(),
            rotation: Angle::DEG0,
            height: plen(height),
            stroke_width: len(200_000),
            align: Alignment::new(h, v),
            locked: false,
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
            "{{BOARD}}",
            "{{PROJECT}}",
            "{{AUTHOR}}",
            "{{VERSION}}",
            "{{DATE}}",
            "{{TIME}}",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn measure_snap_candidates(e: &Package, fpt: Option<Uuid>) -> BTreeSet<Point> {
        // Upstream MeasureTool::snapCandidatesFromFootprint().
        let mut c = BTreeSet::new();
        let Some(f) = footprint(e, fpt) else {
            return c;
        };
        for fp in f.pads().iter() {
            let p = fp.pad();
            c.insert(p.position());
            for outline in p.geometry().to_outlines().unwrap_or_default() {
                c.extend(snap_candidates_from_path(
                    &outline
                        .rotated(p.rotation(), Point::ORIGIN)
                        .translated(p.position()),
                ));
            }
            for h in p.holes().iter() {
                for v in h.path().get().vertices() {
                    let pos = v.pos.rotated(p.rotation(), Point::ORIGIN) + p.position();
                    c.extend(snap_candidates_from_circle(pos, h.diameter()));
                }
            }
        }
        for p in f.polygons().iter() {
            c.extend(snap_candidates_from_path(p.path()));
        }
        for circle in f.circles().iter() {
            c.extend(snap_candidates_from_circle(
                circle.center(),
                circle.diameter(),
            ));
        }
        for t in f.stroke_texts().iter() {
            c.insert(t.position());
        }
        for h in f.holes().iter() {
            for v in h.path().get().vertices() {
                c.extend(snap_candidates_from_circle(v.pos, h.diameter()));
            }
        }
        c
    }

    fn add_text(kind: &str) -> String {
        match kind {
            "polygon" => tr!(
                "librepcb::editor::PackageEditorState_DrawPolygonBase",
                "Add Footprint Polygon"
            ),
            "circle" => tr!(
                "librepcb::editor::PackageEditorState_DrawCircle",
                "Add Footprint Circle"
            ),
            _ => tr!(
                "librepcb::editor::PackageEditorState_DrawTextBase",
                "Add Footprint Text"
            ),
        }
    }

    fn paste_text() -> String {
        tr!(
            "librepcb::editor::PackageEditorState_Select",
            "Paste Footprint Elements"
        )
    }

    fn state(states: &mut PackageStates, tool: LibraryTool) -> Option<&mut dyn State<Self>> {
        Some(match tool {
            LibraryTool::AddThtPads => &mut states.tht_pads,
            LibraryTool::AddSmtPads(f) => {
                let f = if states.smt_pads.contains_key(&f) {
                    f
                } else {
                    PadFunction::StandardPad
                };
                states.smt_pads.get_mut(&f)?
            }
            LibraryTool::AddHoles => &mut states.holes,
            LibraryTool::DrawZone => &mut states.zone,
            LibraryTool::RenumberPads => &mut states.renumber,
            _ => return None,
        })
    }
}

// --- Add pads ---

/// The add pads state (upstream `PackageEditorState_AddPads`).
#[derive(Debug)]
struct AddPadsState {
    pad_type: PadType,
    /// Properties of the next pad (upstream `mCurrentProperties`).
    props: FootprintPad,
    /// The pad being placed; an undo group is open.
    current: Option<Uuid>,
}

impl AddPadsState {
    fn new(pad_type: PadType, function: PadFunction) -> Self {
        // 0..100% are valid limited ratios.
        let percent = |p| {
            UnsignedLimitedRatio::new(Ratio::from_percent(p)).unwrap_or_else(|_| unreachable!())
        };
        let mut pad = Pad::new(
            Uuid::new_random(),
            Point::ORIGIN,
            Angle::DEG0,
            PadShape::RoundedRect,
            plen(2_500_000),
            plen(1_300_000),
            percent(100),
            Path::new(Vec::new()),
            MaskConfig::Automatic,
            MaskConfig::Off,
            UnsignedLength::default(),
            ComponentSide::Top,
            function,
            PadHoleList::new(),
        );
        if pad_type == PadType::Smt {
            pad.set_radius(percent(50));
            pad.set_width(plen(1_500_000));
            pad.set_height(plen(700_000));
            pad.set_solder_paste_config(MaskConfig::Automatic);
            match function {
                PadFunction::ThermalPad => {
                    pad.set_radius(percent(0));
                    pad.set_width(plen(2_000_000));
                    pad.set_height(plen(2_000_000));
                }
                PadFunction::BgaPad => {
                    pad.set_radius(percent(100));
                    pad.set_width(plen(300_000));
                    pad.set_height(plen(300_000));
                }
                PadFunction::EdgeConnectorPad => {
                    pad.set_radius(percent(0));
                    pad.set_solder_paste_config(MaskConfig::Off);
                }
                PadFunction::TestPad => {
                    pad.set_radius(percent(100));
                    pad.set_width(plen(700_000));
                    pad.set_height(plen(700_000));
                    pad.set_solder_paste_config(MaskConfig::Off);
                }
                PadFunction::LocalFiducial | PadFunction::GlobalFiducial => {
                    pad.set_radius(percent(100));
                    pad.set_width(plen(1_000_000));
                    pad.set_height(plen(1_000_000));
                    pad.set_copper_clearance(len(500_000));
                    pad.set_stop_mask_config(MaskConfig::Manual(Length::new(500_000)));
                    pad.set_solder_paste_config(MaskConfig::Off);
                }
                _ => {}
            }
        } else {
            let mut holes = PadHoleList::new();
            holes.push(PadHole::new(
                Uuid::new_random(),
                plen(800_000), // Commonly used drill diameter.
                NonEmptyPath::from_point(Point::ORIGIN),
            ));
            pad.set_holes(holes);
        }
        let mut this = Self {
            pad_type,
            props: FootprintPad::new(pad, None),
            current: None,
        };
        this.apply_recommended_radius();
        this
    }

    /// Upstream `applyRecommendedRoundedRectRadius()`.
    fn apply_recommended_radius(&mut self) {
        let pad = self.props.pad_mut();
        let r = pad.radius().get();
        if r > Ratio::from_percent(0) && r < Ratio::from_percent(100) {
            pad.set_radius(Pad::recommended_radius(pad.width(), pad.height()));
        }
    }

    fn drill(&self) -> Option<PositiveLength> {
        self.props.pad().holes().first().map(|h| h.diameter())
    }

    fn tool(&self) -> LibraryTool {
        match self.pad_type {
            PadType::Tht => LibraryTool::AddThtPads,
            PadType::Smt => LibraryTool::AddSmtPads(self.props.pad().function()),
        }
    }

    fn write_tool_data(&self, cx: &mut Cx<'_, '_, PackageHost>) {
        let pad = self.props.pad();
        let d = &mut cx.out.tool_data;
        d.package_pad = self.props.package_pad_uuid();
        d.component_side = pad.component_side();
        d.pad_shape = pad.shape();
        d.pad_width = pad.width();
        d.pad_height = pad.height();
        d.pad_radius = pad.radius();
        if let Some(drill) = self.drill() {
            d.drill = drill;
        }
        d.copper_clearance = pad.copper_clearance();
        d.stop_mask = pad.stop_mask_config();
        d.pad_function = pad.function();
    }

    /// Upstream `selectNextFreePackagePad()`.
    fn select_next_free_package_pad(&mut self, cx: &Cx<'_, '_, PackageHost>) {
        let Some(fpt) = footprint(cx.element(), cx.fpt()) else {
            return;
        };
        let pad = cx
            .element()
            .pads()
            .iter()
            .find(|p| {
                !fpt.pads()
                    .iter()
                    .any(|fp| fp.package_pad_uuid() == Some(p.uuid()))
            })
            .map(|p| p.uuid());
        self.props.set_package_pad_uuid(pad);
    }

    /// Upstream `startAddPad()`.
    fn start(&mut self, cx: &mut Cx<'_, '_, PackageHost>, pos: Point) -> bool {
        if !cx.begin(tr!(
            "librepcb::editor::PackageEditorState_AddPads",
            "Add footprint pad"
        )) {
            return false;
        }
        let mut pad = self.props.with_uuid(Uuid::new_random());
        pad.pad_mut().set_position(pos);
        let holes: PadHoleList = self
            .props
            .pad()
            .holes()
            .iter()
            .map(|h| h.with_uuid(Uuid::new_random()))
            .collect();
        pad.pad_mut().set_holes(holes);
        let uuid = pad.uuid();
        match cx.modify(|e, fpt| PackageHost::add_object(e, fpt, FootprintObject::Pad(pad))) {
            Some(Ok(_)) => {
                self.current = Some(uuid);
                cx.out.selection = [FootprintItem::Pad(uuid)].into_iter().collect();
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

    /// Modifies the pad being placed.
    fn edit(&mut self, cx: &mut Cx<'_, '_, PackageHost>, f: impl FnOnce(&mut FootprintPad)) {
        let Some(uuid) = self.current else {
            return;
        };
        cx.modify(|e, fpt| {
            if let Some(p) = footprint_mut(e, fpt).and_then(|f| f.pads_mut().by_uuid_mut(&uuid)) {
                f(p);
            }
        });
    }

    fn current_pad(&self, cx: &Cx<'_, '_, PackageHost>) -> Option<FootprintPad> {
        footprint(cx.element(), cx.fpt())?
            .pads()
            .by_uuid(&self.current?)
            .cloned()
    }

    /// Upstream `finishAddPad()`.
    fn finish(&mut self, cx: &mut Cx<'_, '_, PackageHost>, pos: Point) -> bool {
        self.edit(cx, |p| {
            p.pad_mut().set_position(pos);
        });
        if let Some(pad) = self.current_pad(cx) {
            self.props = pad;
        }
        if let Some(uuid) = self.current.take() {
            cx.out.selection.remove(&FootprintItem::Pad(uuid));
        }
        let ok = cx.commit();
        self.select_next_free_package_pad(cx);
        self.write_tool_data(cx);
        ok
    }

    /// Upstream `abortAddPad()`.
    fn abort_command(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if let Some(pad) = self.current_pad(cx) {
            self.props = pad;
        }
        if let Some(uuid) = self.current.take() {
            cx.out.selection.remove(&FootprintItem::Pad(uuid));
        }
        cx.abort_group()
    }
}

impl State<PackageHost> for AddPadsState {
    fn entry(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if !self.props.pad().function_is_fiducial() {
            self.select_next_free_package_pad(cx);
        }
        let pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        if !self.start(cx, pos) {
            return false;
        }
        cx.out.tool = self.tool();
        self.write_tool_data(cx);
        cx.out.view.features = Features {
            rotate: true,
            ..Features::default()
        };
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if self.current.is_some() && !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        cx.out.view.features = Features::default();
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        if self.current.is_none() {
            return false;
        }
        let pos = e.pos.mapped_to_grid(cx.grid());
        self.edit(cx, |p| {
            p.pad_mut().set_position(pos);
        });
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        let pos = e.pos.mapped_to_grid(cx.grid());
        if self.current.is_some() {
            self.finish(cx, pos);
        }
        self.start(cx, pos)
    }

    fn right_released(&mut self, cx: &mut Cx<'_, '_, PackageHost>, _e: PointerEvent) -> bool {
        State::<PackageHost>::rotate(self, cx, Angle::DEG90)
    }

    fn rotate(&mut self, cx: &mut Cx<'_, '_, PackageHost>, angle: Angle) -> bool {
        use crate::library_editor::commands::Transformable;
        if self.current.is_none() {
            return false;
        }
        self.edit(cx, |p| {
            let center = p.pad().position();
            p.rotate(angle, center);
        });
        true
    }

    fn tool_data_changed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, old: &LibraryToolData) {
        let d = cx.out.tool_data.clone();
        self.props.set_package_pad_uuid(d.package_pad);
        let pad = self.props.pad_mut();
        pad.set_component_side(d.component_side);
        pad.set_function(d.pad_function);
        pad.set_copper_clearance(d.copper_clearance);
        pad.set_stop_mask_config(d.stop_mask);
        pad.set_radius(d.pad_radius);
        if d.pad_shape != old.pad_shape {
            pad.set_shape(d.pad_shape);
            self.apply_recommended_radius();
        }
        if d.pad_width != old.pad_width {
            self.props.pad_mut().set_width(d.pad_width);
            self.apply_recommended_radius();
            // Avoid creating pads with a drill larger than width or height.
            if self.drill().is_some_and(|drill| drill > d.pad_width) {
                self.set_drill(d.pad_width);
            }
        }
        if d.pad_height != old.pad_height {
            self.props.pad_mut().set_height(d.pad_height);
            self.apply_recommended_radius();
            if self.drill().is_some_and(|drill| drill > d.pad_height) {
                self.set_drill(d.pad_height);
            }
        }
        if d.drill != old.drill && self.drill().is_some() {
            self.set_drill(d.drill);
        }
        // Apply the properties to the pad being placed.
        let props = self.props.clone();
        self.edit(cx, |p| {
            p.set_package_pad_uuid(props.package_pad_uuid());
            let (src, dst) = (props.pad(), p.pad_mut());
            dst.set_component_side(src.component_side());
            dst.set_function(src.function());
            dst.set_shape(src.shape());
            dst.set_width(src.width());
            dst.set_height(src.height());
            dst.set_radius(src.radius());
            dst.set_copper_clearance(src.copper_clearance());
            dst.set_stop_mask_config(src.stop_mask_config());
            let holes: PadHoleList = dst
                .holes()
                .iter()
                .zip(src.holes().iter())
                .map(|(d, s)| {
                    let mut h = d.clone();
                    h.set_diameter(s.diameter());
                    h
                })
                .collect();
            dst.set_holes(holes);
        });
        self.write_tool_data(cx);
    }
}

impl AddPadsState {
    /// Upstream `setDrillDiameter()`: also grows the pad if needed.
    fn set_drill(&mut self, diameter: PositiveLength) {
        let pad = self.props.pad_mut();
        let holes: PadHoleList = pad
            .holes()
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let mut h = h.clone();
                if i == 0 {
                    h.set_diameter(diameter);
                }
                h
            })
            .collect();
        pad.set_holes(holes);
        if diameter > pad.width() {
            pad.set_width(diameter);
        }
        if diameter > pad.height() {
            pad.set_height(diameter);
        }
    }
}

// --- Add holes ---

/// The add holes state (upstream `PackageEditorState_AddHoles`).
#[derive(Debug)]
struct AddHolesState {
    diameter: PositiveLength,
    current: Option<Uuid>,
}

impl Default for AddHolesState {
    fn default() -> Self {
        Self {
            diameter: plen(1_000_000), // Commonly used drill diameter.
            current: None,
        }
    }
}

impl AddHolesState {
    fn start(&mut self, cx: &mut Cx<'_, '_, PackageHost>, pos: Point) -> bool {
        if !cx.begin(tr!(
            "librepcb::editor::PackageEditorState_AddHoles",
            "Add Footprint Hole"
        )) {
            return false;
        }
        let hole = Hole::new(
            Uuid::new_random(),
            self.diameter,
            NonEmptyPath::from_point(pos),
            MaskConfig::Automatic,
        );
        let uuid = hole.uuid();
        match cx.modify(|e, fpt| PackageHost::add_object(e, fpt, FootprintObject::Hole(hole))) {
            Some(Ok(_)) => {
                self.current = Some(uuid);
                cx.out.selection = [FootprintItem::Hole(uuid)].into_iter().collect();
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

    fn set_position(&self, cx: &mut Cx<'_, '_, PackageHost>, pos: Point) {
        let Some(uuid) = self.current else {
            return;
        };
        cx.modify(|e, fpt| {
            if let Some(h) = footprint_mut(e, fpt).and_then(|f| f.holes_mut().by_uuid_mut(&uuid)) {
                h.set_path(NonEmptyPath::from_point(pos));
            }
        });
    }

    fn abort_command(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if let Some(uuid) = self.current.take() {
            cx.out.selection.remove(&FootprintItem::Hole(uuid));
        }
        cx.abort_group()
    }
}

impl State<PackageHost> for AddHolesState {
    fn entry(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        let pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        if !self.start(cx, pos) {
            return false;
        }
        cx.out.tool = LibraryTool::AddHoles;
        cx.out.tool_data.drill = self.diameter;
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if self.current.is_some() && !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        true
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        if self.current.is_none() {
            return false;
        }
        let pos = e.pos.mapped_to_grid(cx.grid());
        self.set_position(cx, pos);
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        let pos = e.pos.mapped_to_grid(cx.grid());
        if let Some(uuid) = self.current {
            self.set_position(cx, pos);
            self.current = None;
            cx.out.selection.remove(&FootprintItem::Hole(uuid));
            cx.commit();
        }
        self.start(cx, pos)
    }

    fn tool_data_changed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, _old: &LibraryToolData) {
        self.diameter = cx.out.tool_data.drill;
        if let Some(uuid) = self.current {
            let diameter = self.diameter;
            cx.modify(|e, fpt| {
                if let Some(h) =
                    footprint_mut(e, fpt).and_then(|f| f.holes_mut().by_uuid_mut(&uuid))
                {
                    h.set_diameter(diameter);
                }
            });
        }
    }
}

// --- Draw zone ---

/// The draw zone state (upstream `PackageEditorState_DrawZone`).
#[derive(Debug)]
struct DrawZoneState {
    layers: librepcb_core::geometry::ZoneLayers,
    rules: librepcb_core::geometry::ZoneRules,
    last_angle: Angle,
    current: Option<Uuid>,
    last_scene_pos: Point,
    cursor_pos: Point,
}

impl Default for DrawZoneState {
    fn default() -> Self {
        Self {
            layers: librepcb_core::geometry::ZoneLayers::TOP,
            rules: librepcb_core::geometry::ZoneRules::all(),
            last_angle: Angle::DEG0,
            current: None,
            last_scene_pos: Point::ORIGIN,
            cursor_pos: Point::ORIGIN,
        }
    }
}

impl DrawZoneState {
    fn outline(&self, cx: &Cx<'_, '_, PackageHost>) -> Option<Path> {
        footprint(cx.element(), cx.fpt())?
            .zones()
            .by_uuid(&self.current?)
            .map(|z| z.outline().clone())
    }

    fn set_outline(&self, cx: &mut Cx<'_, '_, PackageHost>, path: Path) {
        let Some(uuid) = self.current else {
            return;
        };
        cx.modify(|e, fpt| {
            if let Some(z) = footprint_mut(e, fpt).and_then(|f| f.zones_mut().by_uuid_mut(&uuid)) {
                z.set_outline(path);
            }
        });
    }

    fn begin(cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        cx.begin(tr!(
            "librepcb::editor::PackageEditorState_DrawZone",
            "Add Footprint Zone"
        ))
    }

    /// Upstream `start()`.
    fn start(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        let path = Path::new(vec![
            Vertex::new(self.cursor_pos, self.last_angle),
            Vertex::new(self.cursor_pos, Angle::DEG0),
        ]);
        if !Self::begin(cx) {
            return false;
        }
        let zone = Zone::new(Uuid::new_random(), self.layers, self.rules, path);
        let uuid = zone.uuid();
        match cx.modify(|e, fpt| PackageHost::add_object(e, fpt, FootprintObject::Zone(zone))) {
            Some(Ok(_)) => {
                self.current = Some(uuid);
                cx.out.selection = [FootprintItem::Zone(uuid)].into_iter().collect();
                self.update_overlay_text(cx);
                self.update_status_bar_message(cx);
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

    /// Upstream `abort()`.
    fn abort_command(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if let Some(uuid) = self.current.take() {
            cx.out.selection.remove(&FootprintItem::Zone(uuid));
        }
        let ok = cx.abort_group();
        self.update_overlay_text(cx);
        self.update_status_bar_message(cx);
        ok
    }

    /// Upstream `addNextSegment()`.
    fn add_next_segment(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        let Some(mut path) = self.outline(cx) else {
            return self.abort_command(cx);
        };
        // If no valid zone has been drawn (area = 0), abort.
        let mut closed = path.clone();
        closed.clean();
        closed.close();
        let has_area = !closed.is_zero_area();
        let n = path.vertices().len();
        let finish = path.is_closed() || path.vertices()[n - 1].pos == path.vertices()[n - 2].pos;
        if finish && !has_area {
            return self.abort_command(cx);
        }
        if has_area {
            path.clean();
            path.open();
            self.set_outline(cx, path.clone());
            if !cx.commit() || !Self::begin(cx) {
                self.current = None;
                return false;
            }
        }
        if finish {
            return self.abort_command(cx);
        }
        let mut v = path.into_vertices();
        if let Some(last) = v.last_mut() {
            last.angle = self.last_angle;
        }
        v.push(Vertex::new(self.cursor_pos, Angle::DEG0));
        self.set_outline(cx, Path::new(v));
        self.update_overlay_text(cx);
        self.update_status_bar_message(cx);
        true
    }

    /// Upstream `updateCursorPosition()`.
    fn update_cursor_position(&mut self, cx: &mut Cx<'_, '_, PackageHost>, m: Modifiers) {
        self.cursor_pos = self.last_scene_pos;
        if !m.shift {
            self.cursor_pos = self.cursor_pos.mapped_to_grid(cx.grid());
        }
        cx.out.view.scene_cursor = Some(SceneCursor {
            pos: self.cursor_pos,
            cross: true,
            circle: false,
        });
        if let Some(path) = self.outline(cx) {
            let mut v = path.into_vertices();
            if let Some(last) = v.last_mut() {
                last.pos = self.cursor_pos;
            }
            self.set_outline(cx, Path::new(v));
        }
        self.update_overlay_text(cx);
    }

    fn update_overlay_text(&self, cx: &mut Cx<'_, '_, PackageHost>) {
        let unit = cx.unit();
        let v = self
            .outline(cx)
            .map(Path::into_vertices)
            .unwrap_or_default();
        let n = v.len();
        let p0 = if n >= 2 {
            v[n - 2].pos
        } else {
            self.cursor_pos
        };
        let p1 = if n >= 2 {
            v[n - 1].pos
        } else {
            self.cursor_pos
        };
        let diff = p1 - p0;
        let (dx, dy) = diff.to_mm();
        let angle = Angle::from_rad(libm::atan2(dy, dx)).unwrap_or(Angle::DEG0);
        cx.out.view.info_box = [
            format_length(unit, "X0", p0.x),
            format_length(unit, "Y0", p0.y),
            format_length(unit, "X1", p1.x),
            format_length(unit, "Y1", p1.y),
            String::new(),
            format_length(unit, "Δ", *diff.length()),
            format_angle(unit, "∠", angle),
        ]
        .join("\n");
    }

    fn update_status_bar_message(&self, cx: &mut Cx<'_, '_, PackageHost>) {
        let ctx = "librepcb::editor::PackageEditorState_DrawZone";
        let note = format!(
            " {}",
            tr!(
                ctx,
                "(press {0} to disable snap, {1} to abort)",
                "Shift",
                tr!(ctx, "right click")
            )
        );
        let msg = if self.current.is_none() {
            tr!(ctx, "Click to specify the first point")
        } else {
            tr!(ctx, "Click to specify the next point")
        };
        cx.out.view.set_status(msg + &note, None);
    }

    fn write_tool_data(&self, cx: &mut Cx<'_, '_, PackageHost>) {
        cx.out.tool_data.zone_layers = self.layers;
        cx.out.tool_data.zone_rules = self.rules;
        cx.out.tool_data.angle = self.last_angle;
    }
}

impl State<PackageHost> for DrawZoneState {
    fn entry(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        self.last_scene_pos = cx.cursor_pos().mapped_to_grid(cx.grid());
        self.update_cursor_position(cx, Modifiers::NONE);
        self.update_status_bar_message(cx);
        cx.out.tool = LibraryTool::DrawZone;
        self.write_tool_data(cx);
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if !self.abort_command(cx) {
            return false;
        }
        cx.set_cursor(None);
        cx.out.view.scene_cursor = None;
        cx.out.view.info_box.clear();
        cx.out.view.set_status(String::new(), None);
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        self.current.is_some() && self.abort_command(cx)
    }

    fn key_pressed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: KeyEvent) -> bool {
        if e.key == Key::Shift {
            self.update_cursor_position(cx, e.modifiers);
            return true;
        }
        false
    }

    fn key_released(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: KeyEvent) -> bool {
        State::<PackageHost>::key_pressed(self, cx, e)
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        self.last_scene_pos = e.pos;
        self.update_cursor_position(cx, e.modifiers);
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        self.last_scene_pos = e.pos;
        self.update_cursor_position(cx, e.modifiers);
        if self.current.is_some() {
            self.add_next_segment(cx)
        } else {
            self.start(cx)
        }
    }

    fn left_double_clicked(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        State::<PackageHost>::left_pressed(self, cx, e)
    }

    fn tool_data_changed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, old: &LibraryToolData) {
        let d = cx.out.tool_data.clone();
        self.layers = d.zone_layers;
        self.rules = d.zone_rules;
        if let Some(uuid) = self.current {
            let (layers, rules) = (self.layers, self.rules);
            cx.modify(|e, fpt| {
                if let Some(z) =
                    footprint_mut(e, fpt).and_then(|f| f.zones_mut().by_uuid_mut(&uuid))
                {
                    z.set_layers(layers);
                    z.set_rules(rules);
                }
            });
        }
        if d.angle != old.angle {
            self.last_angle = d.angle;
            if let Some(path) = self.outline(cx) {
                let mut v = path.into_vertices();
                let n = v.len();
                if n >= 2 {
                    v[n - 2].angle = self.last_angle;
                    self.set_outline(cx, Path::new(v));
                }
            }
        }
        self.write_tool_data(cx);
    }
}

// --- Re-number pads ---

/// The re-number pads state (upstream `PackageEditorState_ReNumberPads`).
#[derive(Debug, Default)]
struct RenumberPadsState {
    /// Package pads sorted by name.
    package_pads: Vec<Uuid>,
    active: bool,
    assigned_count: usize,
    previous_pad: Option<Uuid>,
    current_pad: Option<Uuid>,
    /// Number of pads of the temporary assignment.
    tmp_count: usize,
    /// Content before applying the temporary assignment of the current
    /// pad (upstream `mTmpCmd`).
    tmp_snapshot: Option<crate::library_editor::PackageContent>,
    current_pos: Point,
    modifiers: Modifiers,
}

impl RenumberPadsState {
    /// Upstream `start()`.
    fn start(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        let Some(fpt) = footprint(cx.element(), cx.fpt()) else {
            return false;
        };
        let pad_uuids: Vec<Uuid> = fpt.pads().iter().map(|p| p.uuid()).collect();
        let mut pads: Vec<_> = cx.element().pads().iter().collect();
        pads.sort_by(|a, b| toolbox::compare_numeric(a.name().as_str(), b.name().as_str()));
        self.package_pads = pads.into_iter().map(|p| p.uuid()).collect();
        self.assigned_count = 0;
        self.previous_pad = None;
        self.current_pad = None;
        self.tmp_snapshot = None;
        self.current_pos = cx.cursor_pos();
        self.modifiers = Modifiers::NONE;
        if !cx.begin(tr!(
            "librepcb::editor::PackageEditorState_ReNumberPads",
            "Re-number pads"
        )) {
            return false;
        }
        self.active = true;
        // Clear all pad numbers.
        cx.modify(|e, fpt| {
            if let Some(f) = footprint_mut(e, fpt) {
                for uuid in &pad_uuids {
                    if let Some(p) = f.pads_mut().by_uuid_mut(uuid) {
                        p.set_package_pad_uuid(None);
                    }
                }
            }
        });
        true
    }

    /// Discards the temporary assignment of the current pad.
    fn discard_tmp(&mut self, cx: &mut Cx<'_, '_, PackageHost>) {
        if let Some(snapshot) = self.tmp_snapshot.take()
            && let Err(e) = cx.ctx.editor.restore(snapshot)
        {
            cx.error(e);
        }
    }

    /// Upstream `updateCurrentPad()`.
    fn update_current_pad(&mut self, cx: &mut Cx<'_, '_, PackageHost>, force: bool) {
        if !self.active {
            return;
        }
        let Some(fpt) = footprint(cx.element(), cx.fpt()) else {
            return;
        };
        // Find the pad under the cursor.
        let pad = cx
            .ctx
            .view
            .items_at(self.current_pos, cx.ctx.view.tolerance())
            .into_iter()
            .find_map(|i| match i {
                FootprintItem::Pad(u) => Some(u),
                _ => None,
            });
        if pad == self.current_pad && !force {
            return;
        }
        let _ = fpt;
        // Discard temporary changes.
        self.discard_tmp(cx);
        self.current_pad = pad;
        cx.out.selection.clear();
        let Some(fpt) = footprint(cx.element(), cx.fpt()) else {
            return;
        };
        let Some(pad) = pad.and_then(|u| fpt.pads().by_uuid(&u)) else {
            return;
        };
        if pad.package_pad_uuid().is_some() {
            return;
        }
        // Determine the area between the previous and the current pad.
        let cur_pos = pad.pad().position();
        let prev = self.previous_pad.and_then(|u| fpt.pads().by_uuid(&u));
        let prev_pos = prev.map_or(cur_pos, |p| p.pad().position());
        let (min_x, max_x) = (prev_pos.x.min(cur_pos.x), prev_pos.x.max(cur_pos.x));
        let (min_y, max_y) = (prev_pos.y.min(cur_pos.y), prev_pos.y.max(cur_pos.y));
        let control = self.modifiers.control;
        let shift = self.modifiers.shift;
        // Find all unconnected pads in the area, sorted by position.
        let mut pads: Vec<&FootprintPad> = fpt
            .pads()
            .iter()
            .filter(|p| {
                let pos = p.pad().position();
                if p.package_pad_uuid().is_some() {
                    false
                } else if prev.is_none() || (control && p.uuid() != pad.uuid()) {
                    p.uuid() == pad.uuid()
                } else {
                    pos.x >= min_x && pos.x <= max_x && pos.y >= min_y && pos.y <= max_y
                }
            })
            .collect();
        let inv_x = prev_pos.x > cur_pos.x;
        let inv_y = prev_pos.y < cur_pos.y;
        pads.sort_by(|a, b| {
            let (pa, pb) = (a.pad().position(), b.pad().position());
            let by_x = |pa: Point, pb: Point| (pa.x < pb.x) != inv_x;
            let by_y = |pa: Point, pb: Point| (pa.y > pb.y) != inv_y;
            let less = if shift {
                if pa.y != pb.y {
                    by_y(pa, pb)
                } else {
                    by_x(pa, pb)
                }
            } else if pa.x != pb.x {
                by_x(pa, pb)
            } else {
                by_y(pa, pb)
            };
            let greater = if shift {
                if pa.y != pb.y {
                    by_y(pb, pa)
                } else {
                    by_x(pb, pa)
                }
            } else if pa.x != pb.x {
                by_x(pb, pa)
            } else {
                by_y(pb, pa)
            };
            match (less, greater) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
        });
        // Determine the next unused pad number.
        let mut index = 0usize;
        if let Some(prev_pkg) = prev.and_then(|p| p.package_pad_uuid()) {
            index = self
                .package_pads
                .iter()
                .position(|u| *u == prev_pkg)
                .unwrap_or(usize::MAX);
            let keep_last_index = shift;
            if pads.len() > 1 || !keep_last_index {
                index = index.wrapping_add(1);
            }
        }
        // Assign new pad numbers (temporarily).
        let mut assignments = Vec::new();
        let pads: Vec<Uuid> = pads.iter().map(|p| p.uuid()).collect();
        for p in pads {
            if let Some(pkg) = self.package_pads.get(index) {
                assignments.push((p, *pkg));
            }
            cx.out.selection.insert(FootprintItem::Pad(p));
            index = index.wrapping_add(1);
        }
        self.tmp_snapshot = Some(cx.ctx.editor.snapshot());
        cx.modify(|e, fpt| {
            if let Some(f) = footprint_mut(e, fpt) {
                for (pad, pkg) in &assignments {
                    if let Some(p) = f.pads_mut().by_uuid_mut(pad) {
                        p.set_package_pad_uuid(Some(*pkg));
                    }
                }
            }
        });
        self.tmp_count = assignments.len();
    }

    /// Upstream `commitCurrentPad()`.
    fn commit_current_pad(&mut self, cx: &mut Cx<'_, '_, PackageHost>) {
        if self.current_pad.is_some() && self.tmp_snapshot.is_some() {
            cx.out.selection.clear();
            self.tmp_snapshot = None;
            self.previous_pad = self.current_pad.take();
            self.assigned_count += self.tmp_count;
        }
    }
}

impl State<PackageHost> for RenumberPadsState {
    fn entry(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if !self.start(cx) {
            return false;
        }
        cx.out.selection.clear();
        cx.out.tool = LibraryTool::RenumberPads;
        let ctx = "librepcb::editor::PackageEditorState_ReNumberPads";
        let note = format!(
            " {}",
            tr!(
                ctx,
                "(press {0} for single-selection, {1} to change numbering mode, {2} to finish)",
                "Ctrl",
                "Shift",
                "Return"
            )
        );
        cx.out
            .view
            .set_status(tr!(ctx, "Click on the next pad") + &note, None);
        cx.set_cursor(Some(CursorShape::PointingHand));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        self.previous_pad = None;
        self.current_pad = None;
        self.tmp_snapshot = None;
        if self.active {
            cx.abort_group();
            self.active = false;
        }
        self.package_pads.clear();
        cx.out.selection.clear();
        cx.set_cursor(None);
        cx.out.view.set_status(String::new(), None);
        true
    }

    fn key_pressed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: KeyEvent) -> bool {
        if self.modifiers != e.modifiers {
            self.modifiers = e.modifiers;
            self.update_current_pad(cx, true);
            return true;
        }
        false
    }

    fn key_released(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: KeyEvent) -> bool {
        State::<PackageHost>::key_pressed(self, cx, e)
    }

    fn pointer_moved(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        self.current_pos = e.pos;
        let force = self.modifiers != e.modifiers;
        self.modifiers = e.modifiers;
        self.update_current_pad(cx, force);
        true
    }

    fn left_pressed(&mut self, cx: &mut Cx<'_, '_, PackageHost>, e: PointerEvent) -> bool {
        self.current_pos = e.pos;
        let force = self.modifiers != e.modifiers;
        self.modifiers = e.modifiers;
        // Make sure the pad under the cursor is the current one.
        self.update_current_pad(cx, force);
        self.commit_current_pad(cx);
        let count = footprint(cx.element(), cx.fpt()).map_or(0, |f| f.pads().len());
        if self.assigned_count == count {
            State::<PackageHost>::accept(self, cx);
        } else {
            self.update_current_pad(cx, force);
        }
        true
    }

    fn accept(&mut self, cx: &mut Cx<'_, '_, PackageHost>) -> bool {
        if self.active {
            // Keep the temporary assignment of the pad under the cursor out.
            self.discard_tmp(cx);
            cx.commit();
            self.active = false;
            cx.out.leave_requested = true;
        }
        true
    }
}

impl LibraryEditorFsm<PackageHost> {
    /// The footprint being edited.
    pub fn footprint(&self) -> Option<Uuid> {
        self.current_footprint()
    }

    /// Changes the footprint being edited (upstream
    /// `processChangeCurrentFootprint()`).
    pub fn set_footprint(&mut self, ctx: &mut PackageContext<'_>, footprint: Option<Uuid>) -> bool {
        self.change_footprint(ctx, footprint)
    }

    /// Flips the selected items (mirror and move to the other side,
    /// upstream `processFlip()`).
    pub fn flip(
        &mut self,
        ctx: &mut PackageContext<'_>,
        orientation: librepcb_core::types::Orientation,
    ) -> bool {
        self.with_select_state(ctx, |s, cx| State::<PackageHost>::flip(s, cx, orientation))
            .unwrap_or(false)
    }

    /// Imports a DXF file into the current footprint (upstream
    /// `processImportDxf()`, select tool only): its polygons become
    /// polygons on the chosen layer, its circles polygons or non-plated
    /// holes, which are pasted like clipboard data (following the cursor
    /// unless a placement position is given).
    pub fn import_dxf(
        &mut self,
        ctx: &mut PackageContext<'_>,
        settings: &crate::fsm::board::DxfImportSettings,
    ) -> bool {
        let Some(footprint) = self.footprint() else {
            return false;
        };
        self.with_select_state(ctx, |s, cx| {
            let result = crate::fsm::read_dxf_import(settings).map(|(paths, circles)| {
                let pads = cx.element().pads().clone();
                let mut data = FootprintClipboardData::new(footprint, pads, Point::ORIGIN);
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
                    if settings.circles_as_drills {
                        data.holes.push(librepcb_core::geometry::Hole::new(
                            Uuid::new_random(),
                            circle.diameter,
                            librepcb_core::geometry::NonEmptyPath::from_point(circle.position),
                            MaskConfig::Automatic,
                        ));
                    } else {
                        data.polygons.push(polygon(
                            Path::circle(circle.diameter).translated(circle.position),
                        ));
                    }
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

    /// Generates the package outline of the current footprint (upstream
    /// `processGenerateOutline()`).
    pub fn generate_outline(&mut self, ctx: &mut PackageContext<'_>) -> bool {
        let Some(footprint) = self.footprint() else {
            return false;
        };
        self.with_select_state(ctx, |_, cx| {
            cx.out.selection.clear();
            match cx.execute(GeneratePackageOutline { footprint }) {
                Some(true) => true,
                Some(false) => {
                    let ctx = "librepcb::editor::PackageEditorState_Select";
                    cx.out.requests.push(LibraryRequest::ShowInfo {
                        title: tr!(ctx, "No Content"),
                        text: tr!(
                            ctx,
                            "No content (e.g. pads or documentation polygons) found to generate the package outline from. Please add at least the pads before invoking this command."
                        ),
                    });
                    false
                }
                None => false,
            }
        })
        .unwrap_or(false)
    }

    /// Generates the courtyard of the current footprint with the given
    /// excess (upstream `processGenerateCourtyard()`; without offset, the
    /// FSM requests [`LibraryRequest::CourtyardOffsetDialog`]).
    pub fn generate_courtyard(
        &mut self,
        ctx: &mut PackageContext<'_>,
        offset: Option<PositiveLength>,
    ) -> bool {
        let Some(footprint) = self.footprint() else {
            return false;
        };
        self.with_select_state(ctx, |_, cx| {
            let Some(offset) = offset else {
                cx.out.requests.push(LibraryRequest::CourtyardOffsetDialog);
                return false;
            };
            cx.out.selection.clear();
            match cx.execute(GenerateCourtyard { footprint, offset }) {
                Some(true) => true,
                Some(false) => {
                    let ctx = "librepcb::editor::PackageEditorState_Select";
                    cx.out.requests.push(LibraryRequest::ShowInfo {
                        title: tr!(ctx, "No Outline"),
                        text: tr!(
                            ctx,
                            "The courtyard can only be generated if there's a package outline polygon or circle, so that needs to be added first."
                        ),
                    });
                    false
                }
                None => false,
            }
        })
        .unwrap_or(false)
    }
}
