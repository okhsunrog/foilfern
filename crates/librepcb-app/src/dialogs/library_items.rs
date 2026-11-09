//! Properties dialogs of the library element editors (symbol and package
//! tabs) and the "import pins" dialog.
//!
//! Ports of libs/librepcb/editor/library/sym/symbolpinpropertiesdialog.{ui,cpp},
//! libs/librepcb/editor/library/pkg/footprintpadpropertiesdialog.{ui,cpp}
//! and the library editor uses of
//! libs/librepcb/editor/dialogs/{polygon,circle,text,stroketext,hole}propertiesdialog.{ui,cpp}
//! and of `CircuitIdentifierImportDialog`. The dialogs return the modified
//! object ([`LibraryObject`]) for the tab, which applies it as one undo
//! step to its element editor.
//!
//! Differences to upstream: pad holes and custom pad outlines are not
//! editable (like the board pad dialog), vertices are edited in place.

use librepcb_core::geometry::{
    Circle, ComponentSide, Hole, PadFunction, PadShape, Polygon, StrokeText, Text, Zone, ZoneLayers,
};
use librepcb_core::library::pkg::FootprintPad;
use librepcb_core::library::sym::SymbolPin;
use librepcb_core::types::{
    CircuitIdentifier, Layer, Length, LengthUnit, PositiveLength, UnsignedLimitedRatio, Uuid,
};
use librepcb_editor::library_editor::commands::{FootprintObject, SymbolObject};
use librepcb_i18n::tr;

use super::board::{
    chosen_mask_config, chosen_zone_rules, mask_config_fields, positive, unsigned,
    update_mask_config_fields, zone_rule_fields,
};
use super::geometry::{
    alignment_fields, chosen_alignment, chosen_layer, chosen_path, chosen_position, layer_field,
    path_fields, position_fields,
};
use super::{Applied, DialogContext, DialogOptions, FieldEvent, Form, FormDialog, TabDialogResult};

/// A modified object of a library element (the result of the dialogs).
#[derive(Debug, Clone, PartialEq)]
pub enum LibraryObject {
    /// A symbol object.
    Symbol(SymbolObject),
    /// A footprint object (footprint UUID, object).
    Footprint(Option<Uuid>, FootprintObject),
}

/// Where a generic object (polygon, circle) belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectOwner {
    /// A symbol.
    Symbol,
    /// A footprint of a package.
    Footprint(Option<Uuid>),
}

fn applied(o: LibraryObject) -> Result<Applied, String> {
    Ok(Applied::Tab(TabDialogResult::LibraryObject(o)))
}

fn opts(width: f32) -> DialogOptions {
    DialogOptions {
        apply: true,
        width,
        label_width: 150.0,
        ..DialogOptions::default()
    }
}

// --- Pin -----------------------------------------------------------------------------------

const PIN: &str = "librepcb::editor::SymbolPinPropertiesDialog";

/// The properties dialog of a symbol pin (upstream
/// `SymbolPinPropertiesDialog`).
pub struct PinDialog {
    form: Form,
    pin: SymbolPin,
}

impl PinDialog {
    /// A dialog for a pin.
    pub fn new(pin: SymbolPin, unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        form.text("name", tr!(PIN, "Name:"), pin.name().as_str());
        position_fields(&mut form, &tr!(PIN, "Position:"), "", pin.position());
        form.angle("rotation", tr!(PIN, "Rotation:"), pin.rotation());
        form.length_with_steps(
            "length",
            tr!(PIN, "Length:"),
            *pin.length(),
            Length::ZERO,
            crate::length_edit::steps::PIN_LENGTH,
        );
        form.length(
            "name_x",
            tr!(PIN, "Text Position:"),
            pin.name_position().x,
            Length::MIN,
        );
        form.length("name_y", "", pin.name_position().y, Length::MIN);
        form.angle(
            "name_rotation",
            tr!(PIN, "Text Rotation:"),
            pin.name_rotation(),
        );
        form.length_with_steps(
            "name_height",
            tr!(PIN, "Text Height:"),
            *pin.name_height(),
            Length::new(1),
            crate::length_edit::steps::TEXT_HEIGHT,
        );
        alignment_fields(
            &mut form,
            &tr!(PIN, "Text Alignment:"),
            pin.name_alignment(),
        );
        Self { form, pin }
    }
}

impl FormDialog for PinDialog {
    fn title(&self) -> String {
        tr!(PIN, "Pin Properties")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        opts(450.0)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let mut pin = self.pin.clone();
        pin.set_name(
            CircuitIdentifier::new(CircuitIdentifier::clean(&f.get_text("name")))
                .map_err(|e| e.to_string())?,
        );
        pin.set_position(chosen_position(f));
        pin.set_rotation(f.get_angle("rotation"));
        pin.set_length(unsigned(f, "length")?);
        pin.set_name_position(librepcb_core::types::Point::new(
            f.get_length("name_x"),
            f.get_length("name_y"),
        ));
        pin.set_name_rotation(f.get_angle("name_rotation"));
        pin.set_name_height(positive(f, "name_height")?);
        pin.set_name_alignment(chosen_alignment(f));
        self.pin = pin.clone();
        applied(LibraryObject::Symbol(SymbolObject::Pin(pin)))
    }
}

// --- Footprint pad ---------------------------------------------------------------------------

const PAD: &str = "librepcb::editor::FootprintPadPropertiesDialog";

const PAD_SHAPES: [PadShape; 3] = [
    PadShape::RoundedRect,
    PadShape::RoundedOctagon,
    PadShape::Custom,
];

/// The properties dialog of a footprint pad (upstream
/// `FootprintPadPropertiesDialog`): package pad, side, function, shape,
/// size, radius, position, rotation, clearances.
pub struct FootprintPadDialog {
    form: Form,
    footprint: Option<Uuid>,
    pad: FootprintPad,
    package_pads: Vec<Option<Uuid>>,
}

impl FootprintPadDialog {
    /// A dialog for a pad; `package_pads` are the pads of the package
    /// (UUID, name).
    pub fn new(
        footprint: Option<Uuid>,
        pad: FootprintPad,
        package_pads: &[(Uuid, String)],
        unit: LengthUnit,
    ) -> Self {
        let mut form = Form::new(unit);
        form.page(tr!(PAD, "General"));
        let mut names = vec![format!(
            "({})",
            tr!(
                "librepcb::editor::ComponentSignalNameListModel",
                "unconnected"
            )
        )];
        let mut uuids = vec![None];
        for (uuid, name) in package_pads {
            names.push(name.clone());
            uuids.push(Some(*uuid));
        }
        form.choice(
            "package_pad",
            tr!(PAD, "Package Pad:"),
            &names,
            uuids.iter().position(|u| *u == pad.package_pad_uuid()),
        );
        let p = pad.pad();
        form.choice(
            "side",
            tr!(PAD, "Component Side:"),
            &[tr!(PAD, "Top"), tr!(PAD, "Bottom")],
            Some(usize::from(p.component_side() == ComponentSide::Bottom)),
        );
        let functions: Vec<String> = PadFunction::ALL
            .iter()
            .map(|f| f.description_tr())
            .collect();
        form.choice(
            "function",
            tr!(PAD, "Function:"),
            &functions,
            PadFunction::ALL.iter().position(|f| *f == p.function()),
        );
        form.choice(
            "shape",
            tr!(PAD, "Shape:"),
            &[
                tr!(PAD, "Rounded rectangle"),
                tr!(PAD, "Octagon"),
                tr!(PAD, "Custom"),
            ],
            PAD_SHAPES.iter().position(|s| *s == p.shape()),
        );
        form.length("width", tr!(PAD, "Size:"), *p.width(), Length::new(1));
        form.length("height", "", *p.height(), Length::new(1));
        form.ratio(
            "radius",
            tr!(PAD, "Corner Radius:"),
            *p.radius(),
            0,
            1_000_000,
        );
        position_fields(&mut form, &tr!(PAD, "Position:"), "", p.position());
        form.angle("rotation", tr!(PAD, "Rotation:"), p.rotation());
        form.label(
            "holes",
            tr!(PAD, "Plated Holes"),
            if p.holes().is_empty() {
                tr!(PAD, "Pad has no holes")
            } else {
                p.holes().len().to_string()
            },
        );
        form.page(tr!(PAD, "Clearances"));
        mask_config_fields(
            &mut form,
            "stop_mask",
            &tr!(PAD, "Stop Mask:"),
            PAD,
            p.stop_mask_config(),
        );
        mask_config_fields(
            &mut form,
            "solder_paste",
            &tr!(PAD, "Solder Paste:"),
            PAD,
            p.solder_paste_config(),
        );
        form.length(
            "copper_clearance",
            tr!(PAD, "Copper Keepout:"),
            *p.copper_clearance(),
            Length::ZERO,
        );
        Self {
            form,
            footprint,
            pad,
            package_pads: uuids,
        }
    }
}

impl FormDialog for FootprintPadDialog {
    fn title(&self) -> String {
        tr!(PAD, "Pad Properties")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        opts(500.0)
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, _id: &str, _event: FieldEvent) {
        update_mask_config_fields(&self.form, "stop_mask");
        update_mask_config_fields(&self.form, "solder_paste");
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let mut pad = self.pad.clone();
        if let Some(u) = f
            .get_index("package_pad")
            .and_then(|i| self.package_pads.get(i))
        {
            pad.set_package_pad_uuid(*u);
        }
        {
            let p = pad.pad_mut();
            p.set_component_side(if f.get_index("side") == Some(1) {
                ComponentSide::Bottom
            } else {
                ComponentSide::Top
            });
            if let Some(func) = f
                .get_index("function")
                .and_then(|i| PadFunction::ALL.get(i))
            {
                p.set_function(*func);
            }
            if let Some(s) = f.get_index("shape").and_then(|i| PAD_SHAPES.get(i)) {
                p.set_shape(*s);
            }
            p.set_width(positive(f, "width")?);
            p.set_height(positive(f, "height")?);
            p.set_radius(
                UnsignedLimitedRatio::new(f.get_ratio("radius")).map_err(|e| e.to_string())?,
            );
            p.set_position(chosen_position(f));
            p.set_rotation(f.get_angle("rotation"));
            p.set_stop_mask_config(chosen_mask_config(f, "stop_mask"));
            p.set_solder_paste_config(chosen_mask_config(f, "solder_paste"));
            p.set_copper_clearance(unsigned(f, "copper_clearance")?);
        }
        self.pad = pad.clone();
        applied(LibraryObject::Footprint(
            self.footprint,
            FootprintObject::Pad(pad),
        ))
    }
}

// --- Polygon, circle ---------------------------------------------------------------------------

const POLY: &str = "librepcb::editor::PolygonPropertiesDialog";
const CIRCLE: &str = "librepcb::editor::CirclePropertiesDialog";

/// The properties dialog of a polygon of a symbol or footprint (upstream
/// `PolygonPropertiesDialog`).
pub struct LibraryPolygonDialog {
    form: Form,
    owner: ObjectOwner,
    polygon: Polygon,
}

impl LibraryPolygonDialog {
    /// A dialog for a polygon with the allowed layers.
    pub fn new(owner: ObjectOwner, polygon: Polygon, layers: &[Layer], unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        layer_field(
            &mut form,
            "layer",
            &tr!(POLY, "Layer:"),
            layers,
            polygon.layer(),
        );
        form.length(
            "line_width",
            tr!(POLY, "Line Width:"),
            *polygon.line_width(),
            Length::ZERO,
        );
        form.checkbox(
            "fill",
            tr!(POLY, "Options:"),
            tr!(POLY, "Fill"),
            polygon.is_filled(),
        );
        form.checkbox(
            "grab_area",
            "",
            tr!(POLY, "Grab Area"),
            polygon.is_grab_area(),
        );
        path_fields(&mut form, polygon.path());
        Self {
            form,
            owner,
            polygon,
        }
    }
}

fn wrap_polygon(owner: ObjectOwner, p: Polygon) -> LibraryObject {
    match owner {
        ObjectOwner::Symbol => LibraryObject::Symbol(SymbolObject::Polygon(p)),
        ObjectOwner::Footprint(f) => LibraryObject::Footprint(f, FootprintObject::Polygon(p)),
    }
}

impl FormDialog for LibraryPolygonDialog {
    fn title(&self) -> String {
        tr!(POLY, "Polygon Properties")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        opts(450.0)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let mut p = self.polygon.clone();
        if let Some(layer) = chosen_layer(f, "layer") {
            p.set_layer(layer);
        }
        p.set_line_width(unsigned(f, "line_width")?);
        p.set_is_filled(f.get_checked("fill"));
        p.set_is_grab_area(f.get_checked("grab_area"));
        p.set_path(chosen_path(f, self.polygon.path()));
        self.polygon = p.clone();
        applied(wrap_polygon(self.owner, p))
    }
}

/// The properties dialog of a circle (upstream `CirclePropertiesDialog`).
pub struct LibraryCircleDialog {
    form: Form,
    owner: ObjectOwner,
    circle: Circle,
}

impl LibraryCircleDialog {
    /// A dialog for a circle with the allowed layers.
    pub fn new(owner: ObjectOwner, circle: Circle, layers: &[Layer], unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        layer_field(
            &mut form,
            "layer",
            &tr!(CIRCLE, "Layer:"),
            layers,
            circle.layer(),
        );
        form.length(
            "line_width",
            tr!(CIRCLE, "Line Width:"),
            *circle.line_width(),
            Length::ZERO,
        );
        form.checkbox(
            "fill",
            tr!(CIRCLE, "Area:"),
            tr!(CIRCLE, "Filled"),
            circle.is_filled(),
        );
        form.checkbox(
            "grab_area",
            "",
            tr!(CIRCLE, "Grab Area"),
            circle.is_grab_area(),
        );
        position_fields(&mut form, &tr!(CIRCLE, "Position:"), "", circle.center());
        form.length(
            "diameter",
            tr!(CIRCLE, "Diameter:"),
            *circle.diameter(),
            Length::new(1),
        );
        Self {
            form,
            owner,
            circle,
        }
    }
}

impl FormDialog for LibraryCircleDialog {
    fn title(&self) -> String {
        tr!(CIRCLE, "Circle Properties")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        opts(450.0)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let mut c = self.circle.clone();
        if let Some(layer) = chosen_layer(f, "layer") {
            c.set_layer(layer);
        }
        c.set_line_width(unsigned(f, "line_width")?);
        c.set_is_filled(f.get_checked("fill"));
        c.set_is_grab_area(f.get_checked("grab_area"));
        c.set_center(chosen_position(f));
        c.set_diameter(positive(f, "diameter")?);
        self.circle = c.clone();
        applied(match self.owner {
            ObjectOwner::Symbol => LibraryObject::Symbol(SymbolObject::Circle(c)),
            ObjectOwner::Footprint(fpt) => {
                LibraryObject::Footprint(fpt, FootprintObject::Circle(c))
            }
        })
    }
}

// --- Texts -----------------------------------------------------------------------------------

const TEXT: &str = "librepcb::editor::TextPropertiesDialog";
const STROKE: &str = "librepcb::editor::StrokeTextPropertiesDialog";

/// The properties dialog of a symbol text (upstream `TextPropertiesDialog`).
pub struct SymbolTextDialog {
    form: Form,
    text: Text,
}

impl SymbolTextDialog {
    /// A dialog for a text with the allowed layers.
    pub fn new(text: Text, layers: &[Layer], unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        layer_field(
            &mut form,
            "layer",
            &tr!(TEXT, "Layer:"),
            layers,
            text.layer(),
        );
        form.multiline("text", tr!(TEXT, "Text:"), text.text(), 3);
        form.length_with_steps(
            "height",
            tr!(TEXT, "Height:"),
            *text.height(),
            Length::new(1),
            crate::length_edit::steps::TEXT_HEIGHT,
        );
        alignment_fields(&mut form, &tr!(TEXT, "Alignment:"), text.align());
        position_fields(&mut form, &tr!(TEXT, "Position:"), "", text.position());
        form.angle("rotation", tr!(TEXT, "Rotation:"), text.rotation());
        Self { form, text }
    }
}

impl FormDialog for SymbolTextDialog {
    fn title(&self) -> String {
        tr!(TEXT, "Text Properties")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        opts(450.0)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let mut t = self.text.clone();
        if let Some(layer) = chosen_layer(f, "layer") {
            t.set_layer(layer);
        }
        t.set_text(f.get_text("text").to_string());
        t.set_height(positive(f, "height")?);
        t.set_align(chosen_alignment(f));
        t.set_position(chosen_position(f));
        t.set_rotation(f.get_angle("rotation"));
        self.text = t.clone();
        applied(LibraryObject::Symbol(SymbolObject::Text(t)))
    }
}

/// The properties dialog of a footprint stroke text (upstream
/// `StrokeTextPropertiesDialog`).
pub struct FootprintStrokeTextDialog {
    form: Form,
    footprint: Option<Uuid>,
    text: StrokeText,
}

impl FootprintStrokeTextDialog {
    /// A dialog for a stroke text with the allowed layers.
    pub fn new(
        footprint: Option<Uuid>,
        text: StrokeText,
        layers: &[Layer],
        unit: LengthUnit,
    ) -> Self {
        let mut form = Form::new(unit);
        layer_field(
            &mut form,
            "layer",
            &tr!(STROKE, "Layer:"),
            layers,
            text.layer(),
        );
        form.multiline("text", tr!(STROKE, "Text:"), text.text(), 3);
        form.length_with_steps(
            "height",
            tr!(STROKE, "Height:"),
            *text.height(),
            Length::new(1),
            crate::length_edit::steps::TEXT_HEIGHT,
        );
        form.length(
            "stroke_width",
            tr!(STROKE, "Stroke Width:"),
            *text.stroke_width(),
            Length::ZERO,
        );
        alignment_fields(&mut form, &tr!(STROKE, "Alignment:"), text.align());
        position_fields(&mut form, &tr!(STROKE, "Position:"), "", text.position());
        form.angle("rotation", tr!(STROKE, "Rotation:"), text.rotation());
        form.checkbox(
            "mirrored",
            tr!(STROKE, "Options:"),
            tr!(STROKE, "Mirror"),
            text.mirrored(),
        );
        form.checkbox(
            "auto_rotate",
            "",
            tr!(STROKE, "Auto-Rotate"),
            text.auto_rotate(),
        );
        Self {
            form,
            footprint,
            text,
        }
    }
}

impl FormDialog for FootprintStrokeTextDialog {
    fn title(&self) -> String {
        tr!("librepcb::editor::TextPropertiesDialog", "Text Properties")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        opts(450.0)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let mut t = self.text.clone();
        if let Some(layer) = chosen_layer(f, "layer") {
            t.set_layer(layer);
        }
        t.set_text(f.get_text("text").to_string());
        t.set_height(positive(f, "height")?);
        t.set_stroke_width(unsigned(f, "stroke_width")?);
        t.set_align(chosen_alignment(f));
        t.set_position(chosen_position(f));
        t.set_rotation(f.get_angle("rotation"));
        t.set_mirrored(f.get_checked("mirrored"));
        t.set_auto_rotate(f.get_checked("auto_rotate"));
        self.text = t.clone();
        applied(LibraryObject::Footprint(
            self.footprint,
            FootprintObject::StrokeText(t),
        ))
    }
}

// --- Hole ------------------------------------------------------------------------------------

const HOLE: &str = "librepcb::editor::HolePropertiesDialog";

/// The properties dialog of a footprint hole (upstream
/// `HolePropertiesDialog`).
pub struct FootprintHoleDialog {
    form: Form,
    footprint: Option<Uuid>,
    hole: Hole,
}

impl FootprintHoleDialog {
    /// A dialog for a hole.
    pub fn new(footprint: Option<Uuid>, hole: Hole, unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        form.length_with_steps(
            "diameter",
            tr!("librepcb::editor::HoleEditorWidget", "Diameter:"),
            *hole.diameter(),
            Length::new(1),
            crate::length_edit::steps::DRILL_DIAMETER,
        );
        position_fields(
            &mut form,
            &tr!("librepcb::editor::HoleEditorWidget", "Position:"),
            "",
            hole.path().vertices()[0].pos,
        );
        mask_config_fields(
            &mut form,
            "stop_mask",
            &tr!(HOLE, "Stop Mask:"),
            HOLE,
            hole.stop_mask_config(),
        );
        Self {
            form,
            footprint,
            hole,
        }
    }
}

impl FormDialog for FootprintHoleDialog {
    fn title(&self) -> String {
        tr!(HOLE, "Hole Properties")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        opts(450.0)
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, _id: &str, _event: FieldEvent) {
        update_mask_config_fields(&self.form, "stop_mask");
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let mut h = self.hole.clone();
        h.set_diameter(positive(f, "diameter")?);
        let pos = chosen_position(f);
        let offset = pos - h.path().vertices()[0].pos;
        let path = librepcb_core::geometry::NonEmptyPath::new(h.path().get().translated(offset))
            .map_err(|e| e.to_string())?;
        h.set_path(path);
        h.set_stop_mask_config(chosen_mask_config(f, "stop_mask"));
        self.hole = h.clone();
        applied(LibraryObject::Footprint(
            self.footprint,
            FootprintObject::Hole(h),
        ))
    }
}

// --- Zone ------------------------------------------------------------------------------------

const ZONE: &str = "librepcb::editor::ZonePropertiesDialog";

/// The properties dialog of a footprint keepout zone (upstream
/// `ZonePropertiesDialog` for library zones): layers (top, inner,
/// bottom), rules and vertices.
pub struct FootprintZoneDialog {
    form: Form,
    footprint: Option<Uuid>,
    zone: Zone,
}

const ZONE_LAYERS: [ZoneLayers; 3] = [ZoneLayers::TOP, ZoneLayers::INNER, ZoneLayers::BOTTOM];

impl FootprintZoneDialog {
    /// A dialog for a zone.
    pub fn new(footprint: Option<Uuid>, zone: Zone, unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        form.header(tr!(ZONE, "Layers"));
        let texts = [
            tr!(ZONE, "Top Side"),
            tr!(ZONE, "Inner Layers"),
            tr!(ZONE, "Bottom Side"),
        ];
        for (i, (layer, text)) in ZONE_LAYERS.iter().zip(texts).enumerate() {
            form.checkbox(
                &format!("layer_{i}"),
                "",
                text,
                zone.layers().contains(*layer),
            );
        }
        zone_rule_fields(&mut form, zone.rules());
        path_fields(&mut form, zone.outline());
        Self {
            form,
            footprint,
            zone,
        }
    }
}

impl FormDialog for FootprintZoneDialog {
    fn title(&self) -> String {
        tr!(ZONE, "Zone Properties")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        opts(450.0)
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let f = &self.form;
        let mut z = self.zone.clone();
        let mut layers = ZoneLayers::empty();
        for (i, layer) in ZONE_LAYERS.iter().enumerate() {
            layers.set(*layer, f.get_checked(&format!("layer_{i}")));
        }
        z.set_layers(layers);
        z.set_rules(chosen_zone_rules(f));
        z.set_outline(chosen_path(f, self.zone.outline()));
        self.zone = z.clone();
        applied(LibraryObject::Footprint(
            self.footprint,
            FootprintObject::Zone(z),
        ))
    }
}

// --- Courtyard excess ------------------------------------------------------------------------

const SELECT: &str = "librepcb::editor::PackageEditorState_Select";

/// The "Courtyard Excess" dialog of "generate courtyard" (upstream
/// `PackageEditorState_Select::processGenerateCourtyard()`), default
/// 0.2mm (IPC-7351C draft).
pub struct CourtyardOffsetDialog {
    form: Form,
}

impl CourtyardOffsetDialog {
    /// A dialog with the default excess.
    pub fn new(unit: LengthUnit) -> Self {
        let mut form = Form::new(unit);
        form.length(
            "offset",
            String::new(),
            Length::new(200_000),
            Length::new(1),
        );
        Self { form }
    }
}

impl FormDialog for CourtyardOffsetDialog {
    fn title(&self) -> String {
        tr!(SELECT, "Courtyard Excess")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            width: 300.0,
            label_width: 0.0,
            ..DialogOptions::default()
        }
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let offset: PositiveLength = positive(&self.form, "offset")?;
        Ok(Applied::Tab(TabDialogResult::CourtyardOffset(offset)))
    }
}

// --- Import pins -----------------------------------------------------------------------------

const IMPORT: &str = "librepcb::editor::CircuitIdentifierImportDialog";

/// The "import pins" dialog of the symbol editor (upstream
/// `CircuitIdentifierImportDialog`): one pin name per line.
pub struct ImportPinsDialog {
    form: Form,
}

impl Default for ImportPinsDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl ImportPinsDialog {
    /// An empty dialog.
    pub fn new() -> Self {
        let mut form = Form::new(LengthUnit::Millimeters);
        form.note(
            "note",
            tr!(
                IMPORT,
                "Specify the items for mass import in this text field, each item on a separate line."
            ),
        );
        form.multiline("names", "", "", 12);
        Self { form }
    }

    /// The valid names of the input (upstream: cleaned, empty lines
    /// skipped, duplicates removed).
    pub fn names(&self) -> Vec<CircuitIdentifier> {
        let mut names: Vec<CircuitIdentifier> = Vec::new();
        for line in self.form.get_text("names").lines() {
            if let Ok(id) = CircuitIdentifier::new(CircuitIdentifier::clean(line))
                && !names.contains(&id)
            {
                names.push(id);
            }
        }
        names
    }
}

impl FormDialog for ImportPinsDialog {
    fn title(&self) -> String {
        tr!("SymbolEditorTab", "Import Pins")
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            width: 450.0,
            label_width: 80.0,
            ..DialogOptions::default()
        }
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        Ok(Applied::Tab(TabDialogResult::ImportPins(self.names())))
    }
}
