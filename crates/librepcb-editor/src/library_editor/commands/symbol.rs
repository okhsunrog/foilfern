//! Symbol commands: port of libs/librepcb/editor/library/cmd/
//! {cmdsymbolpinedit,cmdremoveselectedsymbolitems,cmdpastesymbolitems,
//! cmddragselectedsymbolitems}.{h,cpp}, the list insert/edit commands of
//! the symbol objects (`CmdSymbolPinInsert`, `CmdPolygonInsert`,
//! `CmdPolygonEdit`, ...) and libs/librepcb/editor/library/sym/
//! symbolclipboarddata.{h,cpp}.
//!
//! Differences to upstream: objects are identified by [`SymbolItem`]s
//! instead of graphics items; edits of polygons, circles, texts and images
//! replace the whole object ([`UpdateSymbolObject`]) instead of setting
//! single properties.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::fileio::{FileSystem, TransactionalDirectory};
use librepcb_core::geometry::{
    Circle, CircleList, Image, ImageList, Path, Polygon, PolygonList, Text, TextList,
};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::sym::{Symbol, SymbolPin, SymbolPinList};
use librepcb_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};
use librepcb_core::types::{
    Alignment, Angle, CircuitIdentifier, FileProofName, Length, Point, PositiveLength,
    UnsignedLength, Uuid,
};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::drag::{ItemContainer, TransformOp, apply_ops};
use super::transform::Transformable;
use crate::error::{Error, Result};
use crate::library_editor::ElementCommand;

/// An object of a symbol, identified by its UUID (upstream: the graphics
/// items of `SymbolGraphicsItem`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum SymbolItem {
    /// A pin.
    Pin(Uuid),
    /// A polygon.
    Polygon(Uuid),
    /// A circle.
    Circle(Uuid),
    /// A text.
    Text(Uuid),
    /// An image.
    Image(Uuid),
}

impl SymbolItem {
    /// The UUID of the object.
    pub fn uuid(&self) -> Uuid {
        match self {
            Self::Pin(u) | Self::Polygon(u) | Self::Circle(u) | Self::Text(u) | Self::Image(u) => {
                *u
            }
        }
    }

    /// Whether the object exists in `symbol`.
    pub fn exists_in(&self, symbol: &Symbol) -> bool {
        match self {
            Self::Pin(u) => symbol.pins().contains_uuid(u),
            Self::Polygon(u) => symbol.polygons().contains_uuid(u),
            Self::Circle(u) => symbol.circles().contains_uuid(u),
            Self::Text(u) => symbol.texts().contains_uuid(u),
            Self::Image(u) => symbol.images().contains_uuid(u),
        }
    }
}

/// All items of a symbol.
pub fn all_symbol_items(symbol: &Symbol) -> BTreeSet<SymbolItem> {
    let mut items = BTreeSet::new();
    items.extend(symbol.pins().iter().map(|o| SymbolItem::Pin(o.uuid())));
    items.extend(
        symbol
            .polygons()
            .iter()
            .map(|o| SymbolItem::Polygon(o.uuid())),
    );
    items.extend(
        symbol
            .circles()
            .iter()
            .map(|o| SymbolItem::Circle(o.uuid())),
    );
    items.extend(symbol.texts().iter().map(|o| SymbolItem::Text(o.uuid())));
    items.extend(symbol.images().iter().map(|o| SymbolItem::Image(o.uuid())));
    items
}

/// A complete object of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub enum SymbolObject {
    /// A pin.
    Pin(SymbolPin),
    /// A polygon.
    Polygon(Polygon),
    /// A circle.
    Circle(Circle),
    /// A text.
    Text(Text),
    /// An image (the file must exist in the symbol directory).
    Image(Image),
}

impl SymbolObject {
    /// The item referring to this object.
    pub fn item(&self) -> SymbolItem {
        match self {
            Self::Pin(o) => SymbolItem::Pin(o.uuid()),
            Self::Polygon(o) => SymbolItem::Polygon(o.uuid()),
            Self::Circle(o) => SymbolItem::Circle(o.uuid()),
            Self::Text(o) => SymbolItem::Text(o.uuid()),
            Self::Image(o) => SymbolItem::Image(o.uuid()),
        }
    }

    /// Returns a copy of the object of `item` in `symbol`.
    pub fn from_symbol(symbol: &Symbol, item: SymbolItem) -> Option<Self> {
        Some(match item {
            SymbolItem::Pin(u) => Self::Pin(symbol.pins().by_uuid(&u)?.clone()),
            SymbolItem::Polygon(u) => Self::Polygon(symbol.polygons().by_uuid(&u)?.clone()),
            SymbolItem::Circle(u) => Self::Circle(symbol.circles().by_uuid(&u)?.clone()),
            SymbolItem::Text(u) => Self::Text(symbol.texts().by_uuid(&u)?.clone()),
            SymbolItem::Image(u) => Self::Image(symbol.images().by_uuid(&u)?.clone()),
        })
    }
}

fn tag_name(item: SymbolItem) -> &'static str {
    match item {
        SymbolItem::Pin(_) => "pin",
        SymbolItem::Polygon(_) => "polygon",
        SymbolItem::Circle(_) => "circle",
        SymbolItem::Text(_) => "text",
        SymbolItem::Image(_) => "image",
    }
}

pub(crate) fn not_found(kind: &'static str, id: impl std::fmt::Display) -> Error {
    Error::NotFound {
        kind,
        id: id.to_string(),
    }
}

impl ItemContainer<SymbolItem> for Symbol {
    fn for_each_selected(
        &mut self,
        items: &BTreeSet<SymbolItem>,
        f: &mut dyn FnMut(&mut dyn Transformable),
    ) {
        for o in self.pins_mut().iter_mut() {
            if items.contains(&SymbolItem::Pin(o.uuid())) {
                f(o);
            }
        }
        for o in self.circles_mut().iter_mut() {
            if items.contains(&SymbolItem::Circle(o.uuid())) {
                f(o);
            }
        }
        for o in self.polygons_mut().iter_mut() {
            if items.contains(&SymbolItem::Polygon(o.uuid())) {
                f(o);
            }
        }
        for o in self.texts_mut().iter_mut() {
            if items.contains(&SymbolItem::Text(o.uuid())) {
                f(o);
            }
        }
        for o in self.images_mut().iter_mut() {
            if items.contains(&SymbolItem::Image(o.uuid())) {
                f(o);
            }
        }
    }

    fn center_points(&self, items: &BTreeSet<SymbolItem>) -> Vec<Point> {
        let mut points = Vec::new();
        for o in self.pins().iter() {
            if items.contains(&SymbolItem::Pin(o.uuid())) {
                points.push(o.position());
            }
        }
        for o in self.circles().iter() {
            if items.contains(&SymbolItem::Circle(o.uuid())) {
                points.push(o.center());
            }
        }
        for o in self.polygons().iter() {
            if items.contains(&SymbolItem::Polygon(o.uuid())) {
                points.extend(o.path().vertices().iter().map(|v| v.pos));
            }
        }
        for o in self.texts().iter() {
            if items.contains(&SymbolItem::Text(o.uuid())) {
                points.push(o.position());
            }
        }
        for o in self.images().iter() {
            if items.contains(&SymbolItem::Image(o.uuid())) {
                // Upstream uses the center since mirroring moves the origin.
                points.push(o.center());
            }
        }
        points
    }

    fn positions(&self, items: &BTreeSet<SymbolItem>) -> Vec<Point> {
        let mut points = Vec::new();
        for o in self.pins().iter() {
            if items.contains(&SymbolItem::Pin(o.uuid())) {
                points.push(o.position());
            }
        }
        for o in self.circles().iter() {
            if items.contains(&SymbolItem::Circle(o.uuid())) {
                points.push(o.center());
            }
        }
        for o in self.texts().iter() {
            if items.contains(&SymbolItem::Text(o.uuid())) {
                points.push(o.position());
            }
        }
        points
    }

    fn set_positions(&mut self, items: &BTreeSet<SymbolItem>, positions: &[Point]) -> Result<()> {
        let mut it = positions.iter().copied();
        let mut next = || {
            it.next()
                .ok_or_else(|| Error::InvalidArgument("Too few positions.".to_owned()))
        };
        for o in self.pins_mut().iter_mut() {
            if items.contains(&SymbolItem::Pin(o.uuid())) {
                o.set_position(next()?);
            }
        }
        for o in self.circles_mut().iter_mut() {
            if items.contains(&SymbolItem::Circle(o.uuid())) {
                o.set_center(next()?);
            }
        }
        for o in self.texts_mut().iter_mut() {
            if items.contains(&SymbolItem::Text(o.uuid())) {
                o.set_position(next()?);
            }
        }
        Ok(())
    }
}

/// Adds an object to a symbol (upstream `CmdSymbolPinInsert`,
/// `CmdPolygonInsert`, `CmdCircleInsert`, `CmdTextInsert`). Fails if an
/// object with the same UUID exists.
#[derive(Debug, Clone, PartialEq)]
pub struct AddSymbolObject(pub SymbolObject);

impl ElementCommand<Symbol> for AddSymbolObject {
    type Output = SymbolItem;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementInsert",
            "Add {0}",
            tag_name(self.0.item())
        )
    }

    fn execute(self, symbol: &mut Symbol) -> Result<SymbolItem> {
        let item = self.0.item();
        if item.exists_in(symbol) {
            return Err(Error::InvalidArgument(format!(
                "The {} {} exists already.",
                tag_name(item),
                item.uuid()
            )));
        }
        match self.0 {
            SymbolObject::Pin(o) => {
                if symbol.pins().contains_name(o.name().as_str()) {
                    return Err(Error::InvalidArgument(format!(
                        "There is already a pin with the name \"{}\".",
                        o.name()
                    )));
                }
                symbol.pins_mut().push(o);
            }
            SymbolObject::Polygon(o) => {
                symbol.polygons_mut().push(o);
            }
            SymbolObject::Circle(o) => {
                symbol.circles_mut().push(o);
            }
            SymbolObject::Text(o) => {
                symbol.texts_mut().push(o);
            }
            SymbolObject::Image(o) => {
                symbol.images_mut().push(o);
            }
        }
        Ok(item)
    }
}

/// Replaces an object of a symbol by UUID (upstream `CmdSymbolPinEdit`,
/// `CmdPolygonEdit`, `CmdCircleEdit`, `CmdTextEdit`, `CmdImageEdit`, as
/// used by the properties dialogs).
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateSymbolObject(pub SymbolObject);

impl ElementCommand<Symbol> for UpdateSymbolObject {
    type Output = ();

    fn text(&self) -> String {
        match self.0 {
            SymbolObject::Pin(_) => tr!("librepcb::editor::CmdSymbolPinEdit", "Edit pin"),
            SymbolObject::Polygon(_) => tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"),
            SymbolObject::Circle(_) => tr!("librepcb::editor::CmdCircleEdit", "Edit circle"),
            SymbolObject::Text(_) => tr!("librepcb::editor::CmdTextEdit", "Edit Text"),
            SymbolObject::Image(_) => tr!("librepcb::editor::CmdImageEdit", "Edit Image"),
        }
    }

    fn execute(self, symbol: &mut Symbol) -> Result<()> {
        let item = self.0.item();
        match self.0 {
            SymbolObject::Pin(o) => {
                let uuid = o.uuid();
                if symbol
                    .pins()
                    .iter()
                    .any(|p| p.uuid() != uuid && p.name() == o.name())
                {
                    return Err(Error::InvalidArgument(format!(
                        "There is already a pin with the name \"{}\".",
                        o.name()
                    )));
                }
                *symbol
                    .pins_mut()
                    .by_uuid_mut(&uuid)
                    .ok_or_else(|| not_found("pin", uuid))? = o;
            }
            SymbolObject::Polygon(o) => {
                *symbol
                    .polygons_mut()
                    .by_uuid_mut(&item.uuid())
                    .ok_or_else(|| not_found("polygon", item.uuid()))? = o;
            }
            SymbolObject::Circle(o) => {
                *symbol
                    .circles_mut()
                    .by_uuid_mut(&item.uuid())
                    .ok_or_else(|| not_found("circle", item.uuid()))? = o;
            }
            SymbolObject::Text(o) => {
                *symbol
                    .texts_mut()
                    .by_uuid_mut(&item.uuid())
                    .ok_or_else(|| not_found("text", item.uuid()))? = o;
            }
            SymbolObject::Image(o) => {
                *symbol
                    .images_mut()
                    .by_uuid_mut(&item.uuid())
                    .ok_or_else(|| not_found("image", item.uuid()))? = o;
            }
        }
        Ok(())
    }
}

/// Edits properties of a symbol pin (upstream `CmdSymbolPinEdit`, used by
/// the pin properties dialog); `None` keeps a property.
#[derive(Debug, Clone, PartialEq)]
pub struct EditSymbolPin {
    /// The pin.
    pub pin: Uuid,
    /// New name (must be unique within the symbol).
    pub name: Option<CircuitIdentifier>,
    /// New position.
    pub position: Option<Point>,
    /// New rotation.
    pub rotation: Option<Angle>,
    /// New length.
    pub length: Option<UnsignedLength>,
    /// New name position.
    pub name_position: Option<Point>,
    /// New name rotation.
    pub name_rotation: Option<Angle>,
    /// New name height.
    pub name_height: Option<PositiveLength>,
    /// New name alignment.
    pub name_alignment: Option<Alignment>,
}

impl EditSymbolPin {
    /// An edit of `pin` which does not change anything yet.
    pub fn new(pin: Uuid) -> Self {
        Self {
            pin,
            name: None,
            position: None,
            rotation: None,
            length: None,
            name_position: None,
            name_rotation: None,
            name_height: None,
            name_alignment: None,
        }
    }
}

impl ElementCommand<Symbol> for EditSymbolPin {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdSymbolPinEdit", "Edit pin")
    }

    fn execute(self, symbol: &mut Symbol) -> Result<()> {
        let mut pin = symbol
            .pins()
            .by_uuid(&self.pin)
            .ok_or_else(|| not_found("pin", self.pin))?
            .clone();
        if let Some(v) = self.name {
            pin.set_name(v);
        }
        if let Some(v) = self.position {
            pin.set_position(v);
        }
        if let Some(v) = self.rotation {
            pin.set_rotation(v);
        }
        if let Some(v) = self.length {
            pin.set_length(v);
        }
        if let Some(v) = self.name_position {
            pin.set_name_position(v);
        }
        if let Some(v) = self.name_rotation {
            pin.set_name_rotation(v);
        }
        if let Some(v) = self.name_height {
            pin.set_name_height(v);
        }
        if let Some(v) = self.name_alignment {
            pin.set_name_alignment(v);
        }
        UpdateSymbolObject(SymbolObject::Pin(pin)).execute(symbol)
    }
}

/// Removes objects of a symbol (upstream `CmdRemoveSelectedSymbolItems`).
/// Image files are removed when saving if no image uses them anymore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveSymbolItems {
    /// The objects to remove (missing ones are ignored).
    pub items: BTreeSet<SymbolItem>,
}

impl ElementCommand<Symbol> for RemoveSymbolItems {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdRemoveSelectedSymbolItems",
            "Remove Symbol Elements"
        )
    }

    fn execute(self, symbol: &mut Symbol) -> Result<()> {
        for item in &self.items {
            match item {
                SymbolItem::Pin(u) => {
                    symbol.pins_mut().take_by_uuid(u);
                }
                SymbolItem::Polygon(u) => {
                    symbol.polygons_mut().take_by_uuid(u);
                }
                SymbolItem::Circle(u) => {
                    symbol.circles_mut().take_by_uuid(u);
                }
                SymbolItem::Text(u) => {
                    symbol.texts_mut().take_by_uuid(u);
                }
                SymbolItem::Image(u) => {
                    symbol.images_mut().take_by_uuid(u);
                }
            }
        }
        Ok(())
    }
}

/// Moves, rotates, mirrors or snaps objects of a symbol around their
/// common center (upstream `CmdDragSelectedSymbolItems`). Returns whether
/// anything was modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformSymbolItems {
    /// The objects.
    pub items: BTreeSet<SymbolItem>,
    /// Grid interval (for the center and snapping).
    pub grid: PositiveLength,
    /// The transformations, applied in order.
    pub ops: Vec<TransformOp>,
}

impl ElementCommand<Symbol> for TransformSymbolItems {
    type Output = bool;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdDragSelectedSymbolItems",
            "Drag Symbol Elements"
        )
    }

    fn execute(self, symbol: &mut Symbol) -> Result<bool> {
        apply_ops(symbol, self.items, self.grid, &self.ops)
    }
}

/// Removes vertices of a polygon (upstream
/// `SymbolEditorState_Select::removePolygonVertices()`); does nothing if
/// less than two vertices would remain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveSymbolPolygonVertices {
    /// The polygon.
    pub polygon: Uuid,
    /// Indices of the vertices to remove.
    pub vertices: BTreeSet<usize>,
}

/// Removes vertices from a path like upstream's select states.
pub(crate) fn path_without_vertices(path: &Path, vertices: &BTreeSet<usize>) -> Option<Path> {
    let mut new = Path::new(
        path.vertices()
            .iter()
            .enumerate()
            .filter(|(i, _)| !vertices.contains(i))
            .map(|(_, v)| *v)
            .collect(),
    );
    if path.is_closed() && new.vertices().len() > 2 {
        new.close();
    }
    if new.is_closed() && new.vertices().len() == 3 {
        new.vertices_mut().pop(); // Avoid overlapping lines.
    }
    (new.vertices().len() >= 2).then_some(new)
}

impl ElementCommand<Symbol> for RemoveSymbolPolygonVertices {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon")
    }

    fn execute(self, symbol: &mut Symbol) -> Result<()> {
        let polygon = symbol
            .polygons_mut()
            .by_uuid_mut(&self.polygon)
            .ok_or_else(|| not_found("polygon", self.polygon))?;
        if let Some(path) = path_without_vertices(polygon.path(), &self.vertices) {
            polygon.set_path(path);
        }
        Ok(())
    }
}

/// The MIME type of symbol clipboard data (upstream
/// `SymbolClipboardData::getMimeType()`).
pub fn symbol_clipboard_mime_type(app_version: &str) -> String {
    format!("application/x-librepcb-clipboard.symbol; version={app_version}")
}

/// Copied symbol objects (upstream `SymbolClipboardData`).
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolClipboardData {
    /// The symbol the objects were copied from.
    pub symbol_uuid: Uuid,
    /// The cursor position when copying.
    pub cursor_pos: Point,
    /// Pins.
    pub pins: SymbolPinList,
    /// Polygons.
    pub polygons: PolygonList,
    /// Circles.
    pub circles: CircleList,
    /// Texts.
    pub texts: TextList,
    /// Images.
    pub images: ImageList,
    /// Image files by file name.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl SymbolClipboardData {
    /// Empty data.
    pub fn new(symbol_uuid: Uuid, cursor_pos: Point) -> Self {
        Self {
            symbol_uuid,
            cursor_pos,
            pins: SymbolPinList::new(),
            polygons: PolygonList::new(),
            circles: CircleList::new(),
            texts: TextList::new(),
            images: ImageList::new(),
            files: BTreeMap::new(),
        }
    }

    /// Copies `items` of `symbol`.
    pub fn from_items(
        symbol: &Symbol,
        items: &BTreeSet<SymbolItem>,
        cursor_pos: Point,
    ) -> Result<Self> {
        let mut data = Self::new(symbol.metadata().uuid(), cursor_pos);
        for o in symbol.pins().iter() {
            if items.contains(&SymbolItem::Pin(o.uuid())) {
                data.pins.push(o.clone());
            }
        }
        for o in symbol.circles().iter() {
            if items.contains(&SymbolItem::Circle(o.uuid())) {
                data.circles.push(o.clone());
            }
        }
        for o in symbol.polygons().iter() {
            if items.contains(&SymbolItem::Polygon(o.uuid())) {
                data.polygons.push(o.clone());
            }
        }
        for o in symbol.texts().iter() {
            if items.contains(&SymbolItem::Text(o.uuid())) {
                data.texts.push(o.clone());
            }
        }
        for o in symbol.images().iter() {
            if items.contains(&SymbolItem::Image(o.uuid())) {
                let name = o.file_name().as_str();
                if let Some(content) = symbol.directory().read_if_exists(name)? {
                    data.files.insert(name.to_owned(), content);
                }
                data.images.push(o.clone());
            }
        }
        Ok(data)
    }

    /// Number of objects.
    pub fn item_count(&self) -> usize {
        self.pins.len()
            + self.polygons.len()
            + self.circles.len()
            + self.texts.len()
            + self.images.len()
    }

    /// Serializes the objects (the `symbol.lp` file of the clipboard).
    pub fn to_sexpression(&self) -> SExpression {
        let mut root = List::new("librepcb_clipboard_symbol");
        root.ensure_line_break();
        self.cursor_pos
            .serialize(root.append_list("cursor_position"));
        root.ensure_line_break();
        root.append_child("symbol", &self.symbol_uuid);
        root.ensure_line_break();
        self.pins.serialize(&mut root);
        root.ensure_line_break();
        self.polygons.serialize(&mut root);
        root.ensure_line_break();
        self.circles.serialize(&mut root);
        root.ensure_line_break();
        self.texts.serialize(&mut root);
        root.ensure_line_break();
        self.images.serialize(&mut root);
        root.ensure_line_break();
        SExpression::List(root)
    }

    /// Returns the clipboard content: a ZIP file with `symbol.lp` and the
    /// image files (upstream `toMimeData()`).
    pub fn to_zip(&self) -> Result<Vec<u8>> {
        let dir = TransactionalDirectory::new_temporary()?;
        let fs = dir.file_system();
        for (name, content) in &self.files {
            fs.write(name, content)?;
        }
        fs.write(
            "symbol.lp",
            &self.to_sexpression().to_byte_array(Mode::LibrePcb)?,
        )?;
        Ok(fs.export_to_zip(None)?)
    }

    /// Loads clipboard content (upstream constructor from MIME data).
    pub fn from_zip(zip: &[u8]) -> Result<Self> {
        let dir = TransactionalDirectory::new_temporary()?;
        let fs = dir.file_system();
        fs.load_from_zip_bytes(zip.to_vec())?;
        let root = SExpression::parse(&fs.read("symbol.lp")?, None, Mode::LibrePcb)?;
        let mut data = Self {
            symbol_uuid: root.child_value("symbol/@0")?,
            cursor_pos: Point::deserialize(root.required_child("cursor_position")?)?,
            pins: SymbolPinList::deserialize(&root)?,
            polygons: PolygonList::deserialize(&root)?,
            circles: CircleList::deserialize(&root)?,
            texts: TextList::deserialize(&root)?,
            images: ImageList::deserialize(&root)?,
            files: BTreeMap::new(),
        };
        for image in data.images.iter() {
            let name = image.file_name().as_str();
            if let Some(content) = fs.read_if_exists(name)? {
                data.files.insert(name.to_owned(), content);
            }
        }
        Ok(data)
    }
}

/// Pastes objects into a symbol (upstream `CmdPasteSymbolItems`): objects
/// get new UUIDs if they exist already or come from another symbol, pin
/// names are made unique by incrementing their number. Returns the pasted
/// items.
#[derive(Debug, Clone, PartialEq)]
pub struct PasteSymbolItems {
    /// The objects.
    pub data: SymbolClipboardData,
    /// Offset added to all positions.
    pub offset: Point,
}

impl ElementCommand<Symbol> for PasteSymbolItems {
    type Output = BTreeSet<SymbolItem>;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdPasteSymbolItems",
            "Paste Symbol Elements"
        )
    }

    fn execute(self, symbol: &mut Symbol) -> Result<BTreeSet<SymbolItem>> {
        let data = self.data;
        let offset = self.offset;
        let other_symbol = symbol.metadata().uuid() != data.symbol_uuid;
        let mut pasted = BTreeSet::new();
        let new_uuid = |exists: bool, uuid: Uuid| {
            if exists || other_symbol {
                Uuid::new_random()
            } else {
                uuid
            }
        };

        let pins = data
            .pins
            .sorted_by(|a, b| toolbox::compare_numeric(a.name().as_str(), b.name().as_str()));
        for pin in pins.iter() {
            let uuid = new_uuid(symbol.pins().contains_uuid(&pin.uuid()), pin.uuid());
            let mut name = pin.name().clone();
            for _ in 0..1000 {
                if !symbol.pins().contains_name(name.as_str()) {
                    break;
                }
                name = CircuitIdentifier::new(toolbox::increment_number_in_string(name.as_str()))?;
            }
            let mut copy = pin.with_uuid(uuid);
            copy.set_name(name);
            copy.set_position(pin.position() + offset);
            symbol.pins_mut().push(copy);
            pasted.insert(SymbolItem::Pin(uuid));
        }
        for circle in data.circles.sorted_by_uuid().iter() {
            let uuid = new_uuid(
                symbol.circles().contains_uuid(&circle.uuid()),
                circle.uuid(),
            );
            let mut copy = circle.with_uuid(uuid);
            copy.set_center(circle.center() + offset);
            symbol.circles_mut().push(copy);
            pasted.insert(SymbolItem::Circle(uuid));
        }
        for polygon in data.polygons.sorted_by_uuid().iter() {
            let uuid = new_uuid(
                symbol.polygons().contains_uuid(&polygon.uuid()),
                polygon.uuid(),
            );
            let mut copy = polygon.with_uuid(uuid);
            copy.set_path(polygon.path().translated(offset));
            symbol.polygons_mut().push(copy);
            pasted.insert(SymbolItem::Polygon(uuid));
        }
        for text in data.texts.sorted_by_uuid().iter() {
            let uuid = new_uuid(symbol.texts().contains_uuid(&text.uuid()), text.uuid());
            let mut copy = text.with_uuid(uuid);
            copy.set_position(text.position() + offset);
            symbol.texts_mut().push(copy);
            pasted.insert(SymbolItem::Text(uuid));
        }
        for image in data.images.sorted_by_uuid().iter() {
            let Some(content) = data
                .files
                .get(image.file_name().as_str())
                .filter(|c| !c.is_empty())
            else {
                continue; // Skip images with missing file.
            };
            let uuid = new_uuid(symbol.images().contains_uuid(&image.uuid()), image.uuid());
            let file_name = image_file_for(symbol, image, content)?;
            let mut copy = image.with_uuid(uuid);
            copy.set_file_name(file_name);
            copy.set_position(image.position() + offset);
            symbol.images_mut().push(copy);
            pasted.insert(SymbolItem::Image(uuid));
        }
        Ok(pasted)
    }
}

/// Returns the file name to use for pasting an image file (upstream
/// `ImageHelpers::findExistingFile()` / `getUnusedFileName()`): an existing
/// image file with the same content, else a new file (which is written).
fn image_file_for(symbol: &mut Symbol, image: &Image, content: &[u8]) -> Result<FileProofName> {
    let dir = symbol.directory_mut();
    for file in dir.files("") {
        let ext = file.rsplit('.').next().unwrap_or_default();
        if !Image::SUPPORTED_EXTENSIONS.contains(&ext) {
            continue;
        }
        let Ok(name) = FileProofName::new(file.clone()) else {
            continue;
        };
        if dir.read_if_exists(&file)?.as_deref() == Some(content) {
            return Ok(name);
        }
    }
    let ext = image.file_extension().to_owned();
    let mut base = FileProofName::clean(image.file_basename().trim());
    if base.is_empty() {
        base = "image".to_owned();
    }
    let mut suffix = format!(".{ext}");
    let mut i = 2;
    let name = loop {
        let max = FILE_PROOF_NAME_MAX_LEN.saturating_sub(suffix.chars().count());
        let truncated: String = base.chars().take(max).collect();
        let name = format!("{truncated}{suffix}");
        if !dir.file_exists(&name) {
            break name;
        }
        suffix = format!("-{i}.{ext}");
        i += 1;
    };
    dir.write(&name, content)?;
    Ok(FileProofName::new(name)?)
}

/// Upstream `FileProofNameConstraint::MAX_LEN`.
const FILE_PROOF_NAME_MAX_LEN: usize = 20;

/// Creates the clipboard data of the pins to add for `names` (upstream
/// `SymbolEditorState_AddPins::processImportPins()`): one pin per name,
/// placed below each other with the given pin properties.
pub fn import_pins_data(
    symbol: Uuid,
    names: &[CircuitIdentifier],
    template: &SymbolPin,
) -> SymbolClipboardData {
    let mut data = SymbolClipboardData::new(symbol, Point::ORIGIN);
    let mut pos = Point::ORIGIN;
    for name in names {
        let mut pin = template.with_uuid(Uuid::new_random());
        pin.set_name(name.clone());
        pin.set_position(pos);
        data.pins.push(pin);
        pos.y -= Length::new(2_540_000);
    }
    data
}
