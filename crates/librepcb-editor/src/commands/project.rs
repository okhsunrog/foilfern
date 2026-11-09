//! Project level commands: port of
//! libs/librepcb/editor/project/cmd/cmdprojectedit.{h,cpp},
//! cmdschematicadd, cmdschematicedit, cmdschematicremove, cmdboardadd,
//! cmdboardedit (name) and cmdboardremove, plus the name handling of
//! `ProjectEditor::execNewSheetDialog()`/`execNewBoardDialog()`.

use librepcb_core::attribute::AttributeList;
use librepcb_core::fileio::{CleanFileNameOptions, FileNameCase, FilePath};
use librepcb_core::geometry::Path;
use librepcb_core::project::board::{Board, BoardItem, BoardPolygonData, BoardSettings};
use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{BoardId, BoardMutation, Mutation, ProjectSettings, SchematicId};
use librepcb_core::types::{
    ElementName, FileProofName, Layer, Length, LengthUnit, Point, PositiveLength, UnsignedLength,
    Uuid,
};
use librepcb_i18n::tr;

use super::resolve;
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

/// Returns the directory name for a page or board name (upstream
/// `FilePath::cleanFileName(name, ReplaceSpaces | ToLowerCase)`).
fn directory_name(name: &str) -> Result<String> {
    let dir = FilePath::clean_file_name(
        name,
        CleanFileNameOptions::new(true, FileNameCase::Lower),
        120,
    );
    if dir.is_empty() {
        return Err(Error::InvalidName(name.to_owned()));
    }
    Ok(dir)
}

/// Returns a directory name not used by `used` (appending `_2`, `_3`, ...),
/// since the model rejects duplicate directory names.
fn unique_directory_name<'a>(base: String, used: impl Iterator<Item = &'a str> + Clone) -> String {
    let mut name = base.clone();
    let mut i = 2;
    while used.clone().any(|u| u == name) {
        name = format!("{base}_{i}");
        i += 1;
    }
    name
}

/// Modifies the project metadata (upstream `CmdProjectEdit`); `None`
/// fields are kept.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditProjectMetadata {
    /// The new name.
    #[serde(default)]
    pub name: Option<ElementName>,
    /// The new author.
    #[serde(default)]
    pub author: Option<String>,
    /// The new version.
    #[serde(default)]
    pub version: Option<FileProofName>,
    /// The new attributes.
    #[serde(default)]
    pub attributes: Option<AttributeList>,
}

impl Command for EditProjectMetadata {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdProjectEdit", "Edit Project Metadata")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let mut metadata = tx.project().metadata().clone();
        if let Some(name) = self.name {
            metadata.name = name;
        }
        if let Some(author) = self.author {
            metadata.author = author;
        }
        if let Some(version) = self.version {
            metadata.version = version;
        }
        if let Some(attributes) = self.attributes {
            metadata.attributes = attributes;
        }
        if metadata != *tx.project().metadata() {
            tx.apply(Mutation::SetProjectMetadata(metadata))?;
        }
        Ok(())
    }
}

/// Modifies the project settings; `None` fields are kept.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditProjectSettings {
    /// Locales in order of preference.
    #[serde(default)]
    pub locale_order: Option<Vec<String>>,
    /// Norms in order of preference.
    #[serde(default)]
    pub norm_order: Option<Vec<String>>,
    /// Attribute keys of custom BOM columns.
    #[serde(default)]
    pub custom_bom_attributes: Option<Vec<String>>,
    /// Default lock of the assembly options of new components.
    #[serde(default)]
    pub default_lock_component_assembly: Option<bool>,
}

impl Command for EditProjectSettings {
    type Output = ();

    fn text(&self) -> String {
        "Edit Project Settings".to_owned()
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let old = tx.project().settings().clone();
        let settings = ProjectSettings {
            locale_order: self.locale_order.unwrap_or(old.locale_order.clone()),
            norm_order: self.norm_order.unwrap_or(old.norm_order.clone()),
            custom_bom_attributes: self
                .custom_bom_attributes
                .unwrap_or(old.custom_bom_attributes.clone()),
            default_lock_component_assembly: self
                .default_lock_component_assembly
                .unwrap_or(old.default_lock_component_assembly),
        };
        if settings != old {
            tx.apply(Mutation::SetProjectSettings(settings))?;
        }
        Ok(())
    }
}

/// Adds a schematic page (upstream `CmdSchematicAdd`); the directory name
/// is derived from the name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddSchematic {
    /// The name, e.g. "Main".
    pub name: ElementName,
    /// Page index (default: append).
    #[serde(default)]
    pub index: Option<usize>,
}

impl Command for AddSchematic {
    type Output = SchematicId;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdSchematicAdd", "Add schematic")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<SchematicId> {
        let base = directory_name(self.name.as_str())?;
        let dir = unique_directory_name(
            base,
            tx.project().schematics().iter().map(|s| s.directory_name()),
        );
        let schematic = Schematic::new(Uuid::new_random(), self.name, dir);
        let id = schematic.id();
        tx.apply(Mutation::AddSchematic {
            schematic,
            index: self.index,
        })?;
        Ok(id)
    }
}

/// Modifies name and grid of a schematic page (upstream
/// `CmdSchematicEdit`); `None` fields are kept.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EditSchematic {
    /// The schematic.
    pub schematic: SchematicId,
    /// The new name.
    #[serde(default)]
    pub name: Option<ElementName>,
    /// The new grid interval.
    #[serde(default)]
    pub grid_interval: Option<PositiveLength>,
    /// The new grid unit.
    #[serde(default)]
    pub grid_unit: Option<LengthUnit>,
}

impl Command for EditSchematic {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdSchematicEdit",
            "Edit sheet properties"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let mut properties = resolve::schematic(tx.project(), Some(self.schematic))?.properties();
        if let Some(name) = self.name {
            properties.name = name;
        }
        if let Some(interval) = self.grid_interval {
            properties.grid_interval = interval;
        }
        if let Some(unit) = self.grid_unit {
            properties.grid_unit = unit;
        }
        tx.apply(Mutation::UpdateSchematic(properties))
    }
}

/// Removes an empty schematic page (upstream `CmdSchematicRemove`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveSchematic {
    /// The schematic.
    pub schematic: SchematicId,
}

impl Command for RemoveSchematic {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdSchematicRemove", "Remove schematic")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        tx.apply(Mutation::RemoveSchematic(self.schematic))
    }
}

/// Adds a board (upstream `CmdBoardAdd`) with the default settings (layers,
/// design rules, DRC settings) and, like upstream, a 100x80 mm board
/// outline (upstream `Board::addDefaultContent()`), or as a copy of another
/// board (upstream `Board::copyFrom()`: its settings and all items, with new
/// UUIDs except for the devices and their texts).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddBoard {
    /// The name, e.g. "default".
    pub name: ElementName,
    /// Board which is copied (default: the default settings and the
    /// default board outline).
    #[serde(default, alias = "copy_settings_from")]
    pub copy_from: Option<BoardId>,
    /// Whether the default board outline is added when not copying
    /// (default: true).
    #[serde(default = "default_true")]
    pub default_outline: bool,
}

fn default_true() -> bool {
    true
}

impl AddBoard {
    /// Creates the command with the default settings and outline.
    pub fn new(name: ElementName) -> Self {
        Self {
            name,
            copy_from: None,
            default_outline: true,
        }
    }
}

/// Result of [`AddBoard`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddedBoard {
    /// The new board.
    pub board: BoardId,
    /// The board outline polygon, if added.
    pub outline: Option<Uuid>,
}

impl Command for AddBoard {
    type Output = AddedBoard;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardAdd", "Add board")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<AddedBoard> {
        let base = directory_name(self.name.as_str())?;
        let dir = unique_directory_name(
            base,
            tx.project().boards().iter().map(|b| b.directory_name()),
        );
        let mut board = Board::new(Uuid::new_random(), self.name, dir);
        let id = board.id();
        let mut outline = None;
        if let Some(other) = self.copy_from {
            let settings: BoardSettings = resolve::board(tx.project(), Some(other))?
                .settings()
                .clone();
            board.set_settings(settings);
        }
        tx.apply(Mutation::AddBoard { board, index: None })?;
        if let Some(other) = self.copy_from {
            let steps = copy_board_items(resolve::board(tx.project(), Some(other))?, id);
            tx.apply_all(steps)?;
        }
        if self.copy_from.is_none() && self.default_outline {
            // upstream `Board::addDefaultContent()`: 100x80mm (1/2 Eurocard).
            let polygon = BoardPolygonData::new(
                Uuid::new_random(),
                Layer::BOARD_OUTLINES,
                UnsignedLength::ZERO,
                Path::rect(
                    Point::ORIGIN,
                    Point::new(Length::new(100_000_000), Length::new(80_000_000)),
                ),
                false,
                false,
                false,
            );
            outline = Some(polygon.uuid());
            tx.apply(Mutation::Board(BoardMutation::AddItem {
                board: id,
                item: BoardItem::Polygon(polygon),
            }))?;
        }
        Ok(AddedBoard { board: id, outline })
    }
}

/// The mutations copying the items of `from` into the board `to` (upstream
/// `Board::copyFrom()`): devices with their texts keep their UUIDs, all
/// other items get new ones; traces are connected to the copied anchors.
fn copy_board_items(from: &librepcb_core::project::board::Board, to: BoardId) -> Vec<Mutation> {
    use librepcb_core::geometry::{Trace, TraceAnchor};
    use librepcb_core::project::board::BoardNetSegment;
    use std::collections::BTreeMap;
    let mut steps = Vec::new();
    for device in from.devices().values() {
        steps.push(Mutation::Board(BoardMutation::AddDevice {
            board: to,
            device: device.clone(),
        }));
    }
    for segment in from.net_segments().values() {
        let mut anchors: BTreeMap<TraceAnchor, TraceAnchor> = BTreeMap::new();
        let pads: Vec<_> = segment
            .pads()
            .values()
            .map(|pad| {
                let copy = pad.with_uuid(Uuid::new_random());
                anchors.insert(TraceAnchor::Pad(pad.uuid()), TraceAnchor::Pad(copy.uuid()));
                copy
            })
            .collect();
        let vias: Vec<_> = segment
            .vias()
            .values()
            .map(|via| {
                let copy = via.with_uuid(Uuid::new_random());
                anchors.insert(TraceAnchor::Via(via.uuid()), TraceAnchor::Via(copy.uuid()));
                copy
            })
            .collect();
        let junctions: Vec<_> = segment
            .junctions()
            .values()
            .map(|junction| {
                let copy = junction.with_uuid(Uuid::new_random());
                anchors.insert(
                    TraceAnchor::Junction(junction.uuid()),
                    TraceAnchor::Junction(copy.uuid()),
                );
                copy
            })
            .collect();
        // Footprint pads keep their anchors (same devices).
        let map = |a: TraceAnchor| anchors.get(&a).copied().unwrap_or(a);
        let traces: Vec<_> = segment
            .traces()
            .values()
            .map(|t| {
                Trace::new(
                    Uuid::new_random(),
                    t.layer(),
                    t.width(),
                    map(t.p1()),
                    map(t.p2()),
                )
            })
            .collect();
        steps.push(Mutation::Board(BoardMutation::AddNetSegment {
            board: to,
            segment: BoardNetSegment::with_elements(
                Uuid::new_random(),
                segment.net(),
                pads,
                vias,
                junctions,
                traces,
            ),
        }));
    }
    for plane in from.planes().values() {
        steps.push(Mutation::Board(BoardMutation::AddPlane {
            board: to,
            plane: plane.with_uuid(Uuid::new_random()),
        }));
    }
    let items = from
        .zones()
        .values()
        .map(|z| BoardItem::Zone(z.with_uuid(Uuid::new_random())))
        .chain(
            from.polygons()
                .values()
                .map(|p| BoardItem::Polygon(p.with_uuid(Uuid::new_random()))),
        )
        .chain(
            from.stroke_texts()
                .values()
                .map(|t| BoardItem::StrokeText(t.with_uuid(Uuid::new_random()))),
        )
        .chain(
            from.holes()
                .values()
                .map(|h| BoardItem::Hole(h.with_uuid(Uuid::new_random()))),
        );
    for item in items {
        steps.push(Mutation::Board(BoardMutation::AddItem { board: to, item }));
    }
    steps
}

/// Renames a board (upstream `CmdBoardEdit::setName()`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RenameBoard {
    /// The board.
    pub board: BoardId,
    /// The new name.
    pub name: ElementName,
}

impl Command for RenameBoard {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardEdit", "Modify Board Setup")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let mut properties = resolve::board(tx.project(), Some(self.board))?.properties();
        properties.name = self.name;
        tx.apply(Mutation::UpdateBoard(properties))
    }
}

/// Removes a board with all its items (upstream `CmdBoardRemove`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveBoard {
    /// The board.
    pub board: BoardId,
}

impl Command for RemoveBoard {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdBoardRemove", "Remove board")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        tx.apply(Mutation::RemoveBoard(self.board))
    }
}
