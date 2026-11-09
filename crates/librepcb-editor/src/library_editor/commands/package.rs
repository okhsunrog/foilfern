//! Package commands: port of libs/librepcb/editor/library/cmd/
//! {cmdpackageedit,cmdpackagepadedit,cmdfootprintedit,cmdfootprintpadedit,
//! cmdpackagemodeladd,cmdpackagemodeledit,cmdpackagemodelremove,
//! cmdremoveselectedfootprintitems,cmdpastefootprintitems,
//! cmddragselectedfootprintitems}.{h,cpp}, of the editing logic of
//! libs/librepcb/editor/library/pkg/{packagepadlistmodel,footprintlistmodel}.cpp,
//! of `PackageEditorState_Select::generateOutline()`/`generateCourtyard()`
//! and of libs/librepcb/editor/library/pkg/footprintclipboarddata.{h,cpp}.
//!
//! Differences to upstream: footprint objects are identified by
//! [`FootprintItem`]s; edits of footprint objects replace the whole object
//! ([`UpdateFootprintObject`]). Writing 3D model files is not undone (the
//! file of a removed model is deleted when saving, see
//! [`LibraryElementEditor::save()`](crate::library_editor::LibraryElementEditor::save)).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::fileio::FileSystem;
use librepcb_core::geometry::{
    Circle, CircleList, Hole, HoleList, Path, Polygon, PolygonList, StrokeText, StrokeTextList,
    Zone, ZoneList,
};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::pkg::{
    AlternativeName, AssemblyType, Footprint, FootprintPad, FootprintPadList, Package,
    PackageModel, PackagePad, PackagePadList,
};
use librepcb_core::serialization::{DeserializeObject, List, Mode, SExpression, SerializeObject};
use librepcb_core::types::{
    Angle3D, CircuitIdentifier, ElementName, Layer, Length, Point, Point3D, PositiveLength, Tag,
    UnsignedLength, Uuid,
};
use librepcb_core::utils::{clipper_helpers, painter_path, toolbox};
use librepcb_i18n::tr;

use super::drag::{ItemContainer, TransformOp, apply_ops};
use super::symbol::{not_found, path_without_vertices};
use super::transform::Transformable;
use crate::error::{Error, Result};
use crate::library_editor::ElementCommand;

/// An object of a footprint, identified by its UUID (upstream: the graphics
/// items of `FootprintGraphicsItem`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum FootprintItem {
    /// A pad.
    Pad(Uuid),
    /// A polygon.
    Polygon(Uuid),
    /// A circle.
    Circle(Uuid),
    /// A stroke text.
    StrokeText(Uuid),
    /// A keepout zone.
    Zone(Uuid),
    /// A non-plated hole.
    Hole(Uuid),
}

impl FootprintItem {
    /// The UUID of the object.
    pub fn uuid(&self) -> Uuid {
        match self {
            Self::Pad(u)
            | Self::Polygon(u)
            | Self::Circle(u)
            | Self::StrokeText(u)
            | Self::Zone(u)
            | Self::Hole(u) => *u,
        }
    }

    /// Whether the object exists in `footprint`.
    pub fn exists_in(&self, footprint: &Footprint) -> bool {
        match self {
            Self::Pad(u) => footprint.pads().contains_uuid(u),
            Self::Polygon(u) => footprint.polygons().contains_uuid(u),
            Self::Circle(u) => footprint.circles().contains_uuid(u),
            Self::StrokeText(u) => footprint.stroke_texts().contains_uuid(u),
            Self::Zone(u) => footprint.zones().contains_uuid(u),
            Self::Hole(u) => footprint.holes().contains_uuid(u),
        }
    }
}

/// All items of a footprint.
pub fn all_footprint_items(footprint: &Footprint) -> BTreeSet<FootprintItem> {
    let mut items = BTreeSet::new();
    items.extend(
        footprint
            .pads()
            .iter()
            .map(|o| FootprintItem::Pad(o.uuid())),
    );
    items.extend(
        footprint
            .polygons()
            .iter()
            .map(|o| FootprintItem::Polygon(o.uuid())),
    );
    items.extend(
        footprint
            .circles()
            .iter()
            .map(|o| FootprintItem::Circle(o.uuid())),
    );
    items.extend(
        footprint
            .stroke_texts()
            .iter()
            .map(|o| FootprintItem::StrokeText(o.uuid())),
    );
    items.extend(
        footprint
            .zones()
            .iter()
            .map(|o| FootprintItem::Zone(o.uuid())),
    );
    items.extend(
        footprint
            .holes()
            .iter()
            .map(|o| FootprintItem::Hole(o.uuid())),
    );
    items
}

/// A complete object of a footprint.
#[derive(Debug, Clone, PartialEq)]
pub enum FootprintObject {
    /// A pad.
    Pad(FootprintPad),
    /// A polygon.
    Polygon(Polygon),
    /// A circle.
    Circle(Circle),
    /// A stroke text.
    StrokeText(StrokeText),
    /// A keepout zone.
    Zone(Zone),
    /// A non-plated hole.
    Hole(Hole),
}

impl FootprintObject {
    /// The item referring to this object.
    pub fn item(&self) -> FootprintItem {
        match self {
            Self::Pad(o) => FootprintItem::Pad(o.uuid()),
            Self::Polygon(o) => FootprintItem::Polygon(o.uuid()),
            Self::Circle(o) => FootprintItem::Circle(o.uuid()),
            Self::StrokeText(o) => FootprintItem::StrokeText(o.uuid()),
            Self::Zone(o) => FootprintItem::Zone(o.uuid()),
            Self::Hole(o) => FootprintItem::Hole(o.uuid()),
        }
    }

    /// Returns a copy of the object of `item` in `footprint`.
    pub fn from_footprint(footprint: &Footprint, item: FootprintItem) -> Option<Self> {
        Some(match item {
            FootprintItem::Pad(u) => Self::Pad(footprint.pads().by_uuid(&u)?.clone()),
            FootprintItem::Polygon(u) => Self::Polygon(footprint.polygons().by_uuid(&u)?.clone()),
            FootprintItem::Circle(u) => Self::Circle(footprint.circles().by_uuid(&u)?.clone()),
            FootprintItem::StrokeText(u) => {
                Self::StrokeText(footprint.stroke_texts().by_uuid(&u)?.clone())
            }
            FootprintItem::Zone(u) => Self::Zone(footprint.zones().by_uuid(&u)?.clone()),
            FootprintItem::Hole(u) => Self::Hole(footprint.holes().by_uuid(&u)?.clone()),
        })
    }
}

fn tag_name(item: FootprintItem) -> &'static str {
    match item {
        FootprintItem::Pad(_) => "pad",
        FootprintItem::Polygon(_) => "polygon",
        FootprintItem::Circle(_) => "circle",
        FootprintItem::StrokeText(_) => "stroke_text",
        FootprintItem::Zone(_) => "zone",
        FootprintItem::Hole(_) => "hole",
    }
}

/// Returns the footprint `uuid` of `package` for modification.
pub(crate) fn footprint_mut(package: &mut Package, uuid: Uuid) -> Result<&mut Footprint> {
    package
        .footprints_mut()
        .by_uuid_mut(&uuid)
        .ok_or_else(|| not_found("footprint", uuid))
}

impl ItemContainer<FootprintItem> for Footprint {
    fn for_each_selected(
        &mut self,
        items: &BTreeSet<FootprintItem>,
        f: &mut dyn FnMut(&mut dyn Transformable),
    ) {
        for o in self.pads_mut().iter_mut() {
            if items.contains(&FootprintItem::Pad(o.uuid())) {
                f(o);
            }
        }
        for o in self.circles_mut().iter_mut() {
            if items.contains(&FootprintItem::Circle(o.uuid())) {
                f(o);
            }
        }
        for o in self.polygons_mut().iter_mut() {
            if items.contains(&FootprintItem::Polygon(o.uuid())) {
                f(o);
            }
        }
        for o in self.stroke_texts_mut().iter_mut() {
            if items.contains(&FootprintItem::StrokeText(o.uuid())) {
                f(o);
            }
        }
        for o in self.zones_mut().iter_mut() {
            if items.contains(&FootprintItem::Zone(o.uuid())) {
                f(o);
            }
        }
        for o in self.holes_mut().iter_mut() {
            if items.contains(&FootprintItem::Hole(o.uuid())) {
                f(o);
            }
        }
    }

    fn center_points(&self, items: &BTreeSet<FootprintItem>) -> Vec<Point> {
        let mut points = Vec::new();
        for o in self.pads().iter() {
            if items.contains(&FootprintItem::Pad(o.uuid())) {
                points.push(o.pad().position());
            }
        }
        for o in self.circles().iter() {
            if items.contains(&FootprintItem::Circle(o.uuid())) {
                points.push(o.center());
            }
        }
        for o in self.polygons().iter() {
            if items.contains(&FootprintItem::Polygon(o.uuid())) {
                points.extend(o.path().vertices().iter().map(|v| v.pos));
            }
        }
        for o in self.stroke_texts().iter() {
            if items.contains(&FootprintItem::StrokeText(o.uuid())) {
                points.push(o.position());
            }
        }
        for o in self.zones().iter() {
            if items.contains(&FootprintItem::Zone(o.uuid())) {
                points.extend(o.outline().vertices().iter().map(|v| v.pos));
            }
        }
        for o in self.holes().iter() {
            if items.contains(&FootprintItem::Hole(o.uuid())) {
                points.push(o.path().first().pos);
            }
        }
        points
    }

    fn positions(&self, items: &BTreeSet<FootprintItem>) -> Vec<Point> {
        let mut points = Vec::new();
        for o in self.pads().iter() {
            if items.contains(&FootprintItem::Pad(o.uuid())) {
                points.push(o.pad().position());
            }
        }
        for o in self.circles().iter() {
            if items.contains(&FootprintItem::Circle(o.uuid())) {
                points.push(o.center());
            }
        }
        for o in self.stroke_texts().iter() {
            if items.contains(&FootprintItem::StrokeText(o.uuid())) {
                points.push(o.position());
            }
        }
        for o in self.holes().iter() {
            if items.contains(&FootprintItem::Hole(o.uuid())) {
                points.push(o.path().first().pos);
            }
        }
        points
    }

    fn set_positions(
        &mut self,
        items: &BTreeSet<FootprintItem>,
        positions: &[Point],
    ) -> Result<()> {
        let mut it = positions.iter().copied();
        let mut next = || {
            it.next()
                .ok_or_else(|| Error::InvalidArgument("Too few positions.".to_owned()))
        };
        for o in self.pads_mut().iter_mut() {
            if items.contains(&FootprintItem::Pad(o.uuid())) {
                o.pad_mut().set_position(next()?);
            }
        }
        for o in self.circles_mut().iter_mut() {
            if items.contains(&FootprintItem::Circle(o.uuid())) {
                o.set_center(next()?);
            }
        }
        for o in self.stroke_texts_mut().iter_mut() {
            if items.contains(&FootprintItem::StrokeText(o.uuid())) {
                o.set_position(next()?);
            }
        }
        for o in self.holes_mut().iter_mut() {
            if items.contains(&FootprintItem::Hole(o.uuid())) {
                // Upstream CmdHoleEdit::setPositionOfFirstVertex().
                let p0 = o.path().first().pos;
                o.translate(next()? - p0);
            }
        }
        Ok(())
    }
}

// --- Package ---

/// Edits package properties (upstream `CmdPackageEdit`); `None` keeps a
/// value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditPackage {
    /// New alternative names.
    pub alternative_names: Option<Vec<AlternativeName>>,
    /// New assembly type.
    pub assembly_type: Option<AssemblyType>,
    /// New minimum copper clearance.
    pub min_copper_clearance: Option<UnsignedLength>,
}

impl ElementCommand<Package> for EditPackage {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdPackageEdit",
            "Edit Package Properties"
        )
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        if let Some(v) = self.alternative_names {
            package.set_alternative_names(v);
        }
        if let Some(v) = self.assembly_type {
            package.set_assembly_type(v);
        }
        if let Some(v) = self.min_copper_clearance {
            package.set_min_copper_clearance(v);
        }
        Ok(())
    }
}

fn duplicate_pad_name_error(name: &str) -> Error {
    Error::InvalidArgument(tr!(
        "librepcb::editor::PackagePadListModel",
        "There is already a pad with the name \"{0}\".",
        name
    ))
}

/// Returns the next free numeric pad name (upstream
/// `PackagePadListModel::getNextPadNameProposal()`).
pub fn next_package_pad_name(package: &Package) -> String {
    let mut i = 1;
    while package.pads().contains_name(&i.to_string()) {
        i += 1;
    }
    i.to_string()
}

/// Adds package pads (upstream `PackagePadListModel::add()`): `names` may
/// contain ranges (e.g. `"1..8"`); if empty, the next free number is used.
/// Returns the UUIDs of the new pads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddPackagePads {
    /// Pad names (with ranges).
    pub names: String,
}

impl ElementCommand<Package> for AddPackagePads {
    type Output = Vec<Uuid>;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::PackagePadListModel",
            "Add Package Pad(s)"
        )
    }

    fn execute(self, package: &mut Package) -> Result<Vec<Uuid>> {
        let names = if self.names.is_empty() {
            next_package_pad_name(package)
        } else {
            self.names
        };
        let mut uuids = Vec::new();
        for name in toolbox::expand_ranges_in_string(&names) {
            let name = CircuitIdentifier::clean(&name);
            if package.pads().contains_name(&name) {
                return Err(duplicate_pad_name_error(&name));
            }
            let uuid = Uuid::new_random();
            package
                .pads_mut()
                .push(PackagePad::new(uuid, CircuitIdentifier::new(name)?));
            uuids.push(uuid);
        }
        Ok(uuids)
    }
}

/// Renames a package pad (upstream `CmdPackagePadEdit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePackagePad {
    /// The pad.
    pub pad: Uuid,
    /// The new name (must be unique).
    pub name: CircuitIdentifier,
}

impl ElementCommand<Package> for RenamePackagePad {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdPackagePadEdit", "Edit package pad")
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        if package
            .pads()
            .iter()
            .any(|p| p.uuid() != self.pad && p.name() == &self.name)
        {
            return Err(duplicate_pad_name_error(self.name.as_str()));
        }
        package
            .pads_mut()
            .by_uuid_mut(&self.pad)
            .ok_or_else(|| not_found("package pad", self.pad))?
            .set_name(self.name);
        Ok(())
    }
}

/// Removes a package pad (upstream `CmdPackagePadRemove`; footprint pads
/// keep their reference, which the package check reports).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePackagePad {
    /// The pad.
    pub pad: Uuid,
}

impl ElementCommand<Package> for RemovePackagePad {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementRemove",
            "Remove {0}",
            "pad"
        )
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        package
            .pads_mut()
            .take_by_uuid(&self.pad)
            .ok_or_else(|| not_found("package pad", self.pad))?;
        Ok(())
    }
}

// --- Footprints ---

/// Adds a footprint (upstream `FootprintListModel::add()`). Returns its
/// UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddFootprint {
    /// The name.
    pub name: ElementName,
}

impl ElementCommand<Package> for AddFootprint {
    type Output = Uuid;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementInsert",
            "Add {0}",
            "footprint"
        )
    }

    fn execute(self, package: &mut Package) -> Result<Uuid> {
        let uuid = Uuid::new_random();
        package
            .footprints_mut()
            .push(Footprint::new(uuid, self.name, String::new()));
        Ok(uuid)
    }
}

/// Edits footprint properties (upstream `CmdFootprintEdit`); `None` keeps
/// a value.
#[derive(Debug, Clone, PartialEq)]
pub struct EditFootprint {
    /// The footprint.
    pub footprint: Uuid,
    /// New default name.
    pub name: Option<ElementName>,
    /// New tags.
    pub tags: Option<BTreeSet<Tag>>,
    /// New 3D model position.
    pub model_position: Option<Point3D>,
    /// New 3D model rotation.
    pub model_rotation: Option<Angle3D>,
    /// New 3D models.
    pub models: Option<BTreeSet<Uuid>>,
}

impl EditFootprint {
    /// An edit of `footprint` which does not change anything yet.
    pub fn new(footprint: Uuid) -> Self {
        Self {
            footprint,
            name: None,
            tags: None,
            model_position: None,
            model_rotation: None,
            models: None,
        }
    }
}

impl ElementCommand<Package> for EditFootprint {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdFootprintEdit", "Edit footprint")
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        let fpt = footprint_mut(package, self.footprint)?;
        if let Some(v) = self.name {
            fpt.names_mut().set_default_value(v);
        }
        if let Some(v) = self.tags {
            fpt.set_tags(v);
        }
        if let Some(v) = self.model_position {
            fpt.set_model_position(v);
        }
        if let Some(v) = self.model_rotation {
            fpt.set_model_rotation(v);
        }
        if let Some(v) = self.models {
            fpt.set_models(v);
        }
        Ok(())
    }
}

/// Moves a footprint one position up (upstream `CmdFootprintsSwap`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveFootprintUp {
    /// The footprint.
    pub footprint: Uuid,
}

impl ElementCommand<Package> for MoveFootprintUp {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementsSwap",
            "Move {0}",
            "footprint"
        )
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        let index = package
            .footprints()
            .index_of_uuid(&self.footprint)
            .ok_or_else(|| not_found("footprint", self.footprint))?;
        if index > 0 {
            package.footprints_mut().swap(index, index - 1);
        }
        Ok(())
    }
}

/// Duplicates a footprint (upstream `FootprintListModel::trigger(Duplicate)`,
/// the pads keep their UUIDs). Returns the UUID of the copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateFootprint {
    /// The footprint.
    pub footprint: Uuid,
}

impl ElementCommand<Package> for DuplicateFootprint {
    type Output = Uuid;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementInsert",
            "Add {0}",
            "footprint"
        )
    }

    fn execute(self, package: &mut Package) -> Result<Uuid> {
        let orig = package
            .footprints()
            .by_uuid(&self.footprint)
            .ok_or_else(|| not_found("footprint", self.footprint))?;
        let name = ElementName::new(format!("Copy of {}", orig.names().default_value()))?;
        let uuid = Uuid::new_random();
        let mut copy = Footprint::new(uuid, name, String::new());
        *copy.descriptions_mut() = orig.descriptions().clone();
        copy.set_tags(orig.tags().clone());
        copy.set_model_position(orig.model_position());
        copy.set_model_rotation(orig.model_rotation());
        copy.set_models(orig.models().clone());
        *copy.pads_mut() = orig.pads().clone();
        *copy.polygons_mut() = orig.polygons().clone();
        *copy.circles_mut() = orig.circles().clone();
        *copy.stroke_texts_mut() = orig.stroke_texts().clone();
        *copy.zones_mut() = orig.zones().clone();
        *copy.holes_mut() = orig.holes().clone();
        package.footprints_mut().push(copy);
        Ok(uuid)
    }
}

/// Removes a footprint (upstream `CmdFootprintRemove`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveFootprint {
    /// The footprint.
    pub footprint: Uuid,
}

impl ElementCommand<Package> for RemoveFootprint {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementRemove",
            "Remove {0}",
            "footprint"
        )
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        package
            .footprints_mut()
            .take_by_uuid(&self.footprint)
            .ok_or_else(|| not_found("footprint", self.footprint))?;
        Ok(())
    }
}

// --- 3D models ---

/// Adds a 3D model (upstream `CmdPackageModelAdd`): writes the STEP file
/// and optionally adds the model to all footprints. Returns its UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPackageModel {
    /// The name.
    pub name: ElementName,
    /// Content of the STEP file.
    pub step: Vec<u8>,
    /// Whether to use the model in all footprints.
    pub add_to_footprints: bool,
}

impl ElementCommand<Package> for AddPackageModel {
    type Output = Uuid;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdPackageModelAdd", "Add 3D model")
    }

    fn execute(self, package: &mut Package) -> Result<Uuid> {
        let model = PackageModel::new(Uuid::new_random(), self.name);
        let uuid = model.uuid();
        package
            .directory_mut()
            .write(&model.file_name(), &self.step)?;
        package.models_mut().push(model);
        if self.add_to_footprints {
            for fpt in package.footprints_mut().iter_mut() {
                let mut models = fpt.models().clone();
                models.insert(uuid);
                fpt.set_models(models);
            }
        }
        Ok(uuid)
    }
}

/// Edits a 3D model (upstream `CmdPackageModelEdit`); replacing the STEP
/// file is not undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPackageModel {
    /// The model.
    pub model: Uuid,
    /// New name.
    pub name: Option<ElementName>,
    /// New STEP file content.
    pub step: Option<Vec<u8>>,
}

impl ElementCommand<Package> for EditPackageModel {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdPackageModelEdit", "Edit 3D Model")
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        let model = package
            .models_mut()
            .by_uuid_mut(&self.model)
            .ok_or_else(|| not_found("3D model", self.model))?;
        if let Some(name) = self.name {
            model.set_name(name);
        }
        let file = model.file_name();
        if let Some(step) = self.step {
            package.directory_mut().write(&file, &step)?;
        }
        Ok(())
    }
}

/// Removes a 3D model and its use in all footprints (upstream
/// `CmdPackageModelRemove`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePackageModel {
    /// The model.
    pub model: Uuid,
}

impl ElementCommand<Package> for RemovePackageModel {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdPackageModelRemove", "Remove 3D model")
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        package
            .models_mut()
            .take_by_uuid(&self.model)
            .ok_or_else(|| not_found("3D model", self.model))?;
        for fpt in package.footprints_mut().iter_mut() {
            if fpt.models().contains(&self.model) {
                let mut models = fpt.models().clone();
                models.remove(&self.model);
                fpt.set_models(models);
            }
        }
        Ok(())
    }
}

/// Moves a 3D model one position up (upstream `CmdPackageModelsSwap` of
/// `PackageModelListModel::trigger(MoveUp)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePackageModelUp {
    /// The model.
    pub model: Uuid,
}

impl ElementCommand<Package> for MovePackageModelUp {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementsSwap",
            "Move {0}",
            "3D model"
        )
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        let index = package
            .models()
            .index_of_uuid(&self.model)
            .ok_or_else(|| not_found("3D model", self.model))?;
        if index > 0 {
            package.models_mut().swap(index, index - 1);
        }
        Ok(())
    }
}

// --- Footprint objects ---

/// Adds an object to a footprint (upstream `CmdFootprintPadInsert`,
/// `CmdPolygonInsert`, ...). Fails if an object with the same UUID exists.
#[derive(Debug, Clone, PartialEq)]
pub struct AddFootprintObject {
    /// The footprint.
    pub footprint: Uuid,
    /// The object.
    pub object: FootprintObject,
}

impl ElementCommand<Package> for AddFootprintObject {
    type Output = FootprintItem;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementInsert",
            "Add {0}",
            tag_name(self.object.item())
        )
    }

    fn execute(self, package: &mut Package) -> Result<FootprintItem> {
        let fpt = footprint_mut(package, self.footprint)?;
        let item = self.object.item();
        if item.exists_in(fpt) {
            return Err(Error::InvalidArgument(format!(
                "The {} {} exists already.",
                tag_name(item),
                item.uuid()
            )));
        }
        match self.object {
            FootprintObject::Pad(o) => {
                fpt.pads_mut().push(o);
            }
            FootprintObject::Polygon(o) => {
                fpt.polygons_mut().push(o);
            }
            FootprintObject::Circle(o) => {
                fpt.circles_mut().push(o);
            }
            FootprintObject::StrokeText(o) => {
                fpt.stroke_texts_mut().push(o);
            }
            FootprintObject::Zone(o) => {
                fpt.zones_mut().push(o);
            }
            FootprintObject::Hole(o) => {
                fpt.holes_mut().push(o);
            }
        }
        Ok(item)
    }
}

/// Replaces an object of a footprint by UUID (upstream
/// `CmdFootprintPadEdit`, `CmdPolygonEdit`, `CmdCircleEdit`,
/// `CmdStrokeTextEdit`, `CmdZoneEdit`, `CmdHoleEdit`, as used by the
/// properties dialogs).
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateFootprintObject {
    /// The footprint.
    pub footprint: Uuid,
    /// The object.
    pub object: FootprintObject,
}

impl ElementCommand<Package> for UpdateFootprintObject {
    type Output = ();

    fn text(&self) -> String {
        match self.object {
            FootprintObject::Pad(_) => tr!(
                "librepcb::editor::CmdFootprintPadEdit",
                "Edit footprint pad"
            ),
            FootprintObject::Polygon(_) => tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"),
            FootprintObject::Circle(_) => tr!("librepcb::editor::CmdCircleEdit", "Edit circle"),
            FootprintObject::StrokeText(_) => {
                tr!("librepcb::editor::CmdStrokeTextEdit", "Edit stroke text")
            }
            FootprintObject::Zone(_) => tr!("librepcb::editor::CmdZoneEdit", "Edit zone"),
            FootprintObject::Hole(_) => tr!("librepcb::editor::CmdHoleEdit", "Edit hole"),
        }
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        let fpt = footprint_mut(package, self.footprint)?;
        let uuid = self.object.item().uuid();
        match self.object {
            FootprintObject::Pad(o) => {
                *fpt.pads_mut()
                    .by_uuid_mut(&uuid)
                    .ok_or_else(|| not_found("pad", uuid))? = o;
            }
            FootprintObject::Polygon(o) => {
                *fpt.polygons_mut()
                    .by_uuid_mut(&uuid)
                    .ok_or_else(|| not_found("polygon", uuid))? = o;
            }
            FootprintObject::Circle(o) => {
                *fpt.circles_mut()
                    .by_uuid_mut(&uuid)
                    .ok_or_else(|| not_found("circle", uuid))? = o;
            }
            FootprintObject::StrokeText(o) => {
                *fpt.stroke_texts_mut()
                    .by_uuid_mut(&uuid)
                    .ok_or_else(|| not_found("stroke text", uuid))? = o;
            }
            FootprintObject::Zone(o) => {
                *fpt.zones_mut()
                    .by_uuid_mut(&uuid)
                    .ok_or_else(|| not_found("zone", uuid))? = o;
            }
            FootprintObject::Hole(o) => {
                *fpt.holes_mut()
                    .by_uuid_mut(&uuid)
                    .ok_or_else(|| not_found("hole", uuid))? = o;
            }
        }
        Ok(())
    }
}

/// Removes objects of a footprint (upstream
/// `CmdRemoveSelectedFootprintItems`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveFootprintItems {
    /// The footprint.
    pub footprint: Uuid,
    /// The objects to remove (missing ones are ignored).
    pub items: BTreeSet<FootprintItem>,
}

impl ElementCommand<Package> for RemoveFootprintItems {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdRemoveSelectedFootprintItems",
            "Remove Footprint Elements"
        )
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        let fpt = footprint_mut(package, self.footprint)?;
        for item in &self.items {
            match item {
                FootprintItem::Pad(u) => {
                    fpt.pads_mut().take_by_uuid(u);
                }
                FootprintItem::Polygon(u) => {
                    fpt.polygons_mut().take_by_uuid(u);
                }
                FootprintItem::Circle(u) => {
                    fpt.circles_mut().take_by_uuid(u);
                }
                FootprintItem::StrokeText(u) => {
                    fpt.stroke_texts_mut().take_by_uuid(u);
                }
                FootprintItem::Zone(u) => {
                    fpt.zones_mut().take_by_uuid(u);
                }
                FootprintItem::Hole(u) => {
                    fpt.holes_mut().take_by_uuid(u);
                }
            }
        }
        Ok(())
    }
}

/// Moves, rotates, mirrors, flips or snaps objects of a footprint around
/// their common center (upstream `CmdDragSelectedFootprintItems`).
/// Returns whether anything was modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformFootprintItems {
    /// The footprint.
    pub footprint: Uuid,
    /// The objects.
    pub items: BTreeSet<FootprintItem>,
    /// Grid interval (for the center and snapping).
    pub grid: PositiveLength,
    /// The transformations, applied in order.
    pub ops: Vec<TransformOp>,
}

impl ElementCommand<Package> for TransformFootprintItems {
    type Output = bool;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdDragSelectedFootprintItems",
            "Drag Footprint Elements"
        )
    }

    fn execute(self, package: &mut Package) -> Result<bool> {
        let fpt = footprint_mut(package, self.footprint)?;
        apply_ops(fpt, self.items, self.grid, &self.ops)
    }
}

/// Removes vertices of a polygon or zone of a footprint (upstream
/// `PackageEditorState_Select::removePolygonVertices()` and
/// `removeZoneVertices()`); does nothing if less than two vertices would
/// remain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveFootprintVertices {
    /// The footprint.
    pub footprint: Uuid,
    /// The polygon or zone.
    pub item: FootprintItem,
    /// Indices of the vertices to remove.
    pub vertices: BTreeSet<usize>,
}

impl ElementCommand<Package> for RemoveFootprintVertices {
    type Output = ();

    fn text(&self) -> String {
        match self.item {
            FootprintItem::Zone(_) => tr!("librepcb::editor::CmdZoneEdit", "Edit zone"),
            _ => tr!("librepcb::editor::CmdPolygonEdit", "Edit polygon"),
        }
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        let fpt = footprint_mut(package, self.footprint)?;
        match self.item {
            FootprintItem::Polygon(u) => {
                let polygon = fpt
                    .polygons_mut()
                    .by_uuid_mut(&u)
                    .ok_or_else(|| not_found("polygon", u))?;
                if let Some(path) = path_without_vertices(polygon.path(), &self.vertices) {
                    polygon.set_path(path);
                }
            }
            FootprintItem::Zone(u) => {
                let zone = fpt
                    .zones_mut()
                    .by_uuid_mut(&u)
                    .ok_or_else(|| not_found("zone", u))?;
                if let Some(path) = path_without_vertices(zone.outline(), &self.vertices) {
                    zone.set_outline(path);
                }
            }
            other => {
                return Err(Error::InvalidArgument(format!(
                    "The {} has no vertices.",
                    tag_name(other)
                )));
            }
        }
        Ok(())
    }
}

/// Connects footprint pads to package pads (e.g. the result of the
/// "re-number pads" tool, upstream `CmdFootprintPadEdit::setPackagePadUuid()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetFootprintPadConnections {
    /// The footprint.
    pub footprint: Uuid,
    /// Footprint pad → package pad (`None`: unconnected).
    pub connections: BTreeMap<Uuid, Option<Uuid>>,
}

impl ElementCommand<Package> for SetFootprintPadConnections {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::PackageEditorState_ReNumberPads",
            "Re-number pads"
        )
    }

    fn execute(self, package: &mut Package) -> Result<()> {
        let pkg_pads = package.pads().uuid_set();
        if let Some(bad) = self
            .connections
            .values()
            .flatten()
            .find(|u| !pkg_pads.contains(u))
        {
            return Err(not_found("package pad", bad));
        }
        let fpt = footprint_mut(package, self.footprint)?;
        for (pad, pkg_pad) in self.connections {
            fpt.pads_mut()
                .by_uuid_mut(&pad)
                .ok_or_else(|| not_found("pad", pad))?
                .set_package_pad_uuid(pkg_pad);
        }
        Ok(())
    }
}

/// Generates the package outlines of a footprint from its documentation
/// and pads (upstream `PackageEditorState_Select::generateOutline()`).
/// Returns `false` if there is no content to generate them from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratePackageOutline {
    /// The footprint.
    pub footprint: Uuid,
}

impl ElementCommand<Package> for GeneratePackageOutline {
    type Output = bool;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::PackageEditorState_Select",
            "Generate package outline"
        )
    }

    fn execute(self, package: &mut Package) -> Result<bool> {
        let fpt = footprint_mut(package, self.footprint)?;
        let mut modified = false;
        for bottom in [false, true] {
            let map = |l: Layer| if bottom { l.mirrored(None) } else { l };
            let mut paths: Vec<Path> = Vec::new();
            for polygon in fpt.polygons().iter() {
                if polygon.layer() == map(Layer::TOP_DOCUMENTATION) {
                    match PositiveLength::new(*polygon.line_width()) {
                        Ok(width) => paths.extend(polygon.path().to_outline_strokes(width)),
                        Err(_) => paths.push(polygon.path().clone()),
                    }
                }
            }
            for circle in fpt.circles().iter() {
                if circle.layer() == map(Layer::TOP_DOCUMENTATION) {
                    let diameter = *circle.diameter() + *circle.line_width();
                    if let Ok(d) = PositiveLength::new(diameter) {
                        paths.push(Path::circle(d).translated(circle.center()));
                    }
                }
            }
            // Generate bottom outlines only if there is documentation on the
            // bottom side!
            if !painter_path::is_empty_px(&paths) || !bottom {
                for pad in fpt.pads().iter() {
                    let pad = pad.pad();
                    if pad.is_on_layer(map(Layer::TOP_COPPER)) {
                        let outlines = pad.geometry().to_outlines().unwrap_or_default();
                        for outline in outlines {
                            let mut p = outline
                                .rotated(pad.rotation(), Point::ORIGIN)
                                .translated(pad.position());
                            p.close();
                            paths.push(p);
                        }
                    }
                }
            }
            if painter_path::is_empty_px(&paths) {
                continue;
            }
            let rect = painter_path::bounding_rect_px(&paths);
            if rect.width <= 0.0 || rect.height <= 0.0 {
                continue;
            }
            let p1 = Point::from_px(rect.left(), rect.top()).unwrap_or_default();
            let p2 = Point::from_px(rect.right(), rect.bottom()).unwrap_or_default();
            let mut path = Path::rect(p1, p2);
            path.open();
            let layer = map(Layer::TOP_PACKAGE_OUTLINES);
            let mut outline_set = false;
            let mut remove = Vec::new();
            for polygon in fpt.polygons_mut().iter_mut() {
                if polygon.layer() == layer {
                    if !outline_set {
                        polygon.set_line_width(UnsignedLength::default());
                        polygon.set_path(path.clone());
                        outline_set = true;
                    } else {
                        remove.push(polygon.uuid());
                    }
                }
            }
            for uuid in remove {
                fpt.polygons_mut().take_by_uuid(&uuid);
            }
            if !outline_set {
                fpt.polygons_mut().push(Polygon::new(
                    Uuid::new_random(),
                    layer,
                    UnsignedLength::default(),
                    false,
                    false,
                    path,
                ));
            }
            modified = true;
        }
        Ok(modified)
    }
}

/// Generates the courtyard of a footprint by offsetting its package
/// outlines (upstream `PackageEditorState_Select::generateCourtyard()`).
/// Returns `false` if there is no package outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateCourtyard {
    /// The footprint.
    pub footprint: Uuid,
    /// The courtyard excess (upstream default 0.2 mm from IPC7351C).
    pub offset: PositiveLength,
}

impl ElementCommand<Package> for GenerateCourtyard {
    type Output = bool;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::PackageEditorState_Select",
            "Generate courtyard"
        )
    }

    fn execute(self, package: &mut Package) -> Result<bool> {
        let fpt = footprint_mut(package, self.footprint)?;
        let max_arc_tolerance = PositiveLength::new(Length::new(50_000))
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let courtyard_of = |l: Layer| {
            if l.is_top() {
                Layer::TOP_COURTYARD
            } else {
                Layer::BOT_COURTYARD
            }
        };
        let mut modified = false;

        // Offset polygons.
        let mut polygons: Vec<(Layer, Path)> = Vec::new();
        for polygon in fpt.polygons().iter() {
            if polygon.layer().is_package_outline() {
                let mut paths = vec![clipper_helpers::path_to_clipper(
                    polygon.path(),
                    max_arc_tolerance,
                )];
                clipper_helpers::offset(
                    &mut paths,
                    *self.offset,
                    max_arc_tolerance,
                    clipper::JoinType::Miter,
                )
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
                for mut path in clipper_helpers::paths_from_clipper(&paths) {
                    path.open();
                    polygons.push((courtyard_of(polygon.layer()), path));
                }
            }
        }
        let mut polygons = polygons.into_iter();
        let mut remove = Vec::new();
        for polygon in fpt.polygons_mut().iter_mut() {
            if polygon.layer().is_package_courtyard() {
                if let Some((layer, path)) = polygons.next() {
                    polygon.set_layer(layer);
                    polygon.set_line_width(UnsignedLength::default());
                    polygon.set_path(path);
                } else {
                    remove.push(polygon.uuid());
                }
                modified = true;
            }
        }
        for uuid in remove {
            fpt.polygons_mut().take_by_uuid(&uuid);
        }
        for (layer, path) in polygons {
            fpt.polygons_mut().push(Polygon::new(
                Uuid::new_random(),
                layer,
                UnsignedLength::default(),
                false,
                false,
                path,
            ));
            modified = true;
        }

        // Offset circles.
        let mut circles: Vec<(Layer, Point, PositiveLength)> = Vec::new();
        for circle in fpt.circles().iter() {
            if circle.layer().is_package_outline() {
                let diameter = PositiveLength::new(*circle.diameter() + *self.offset * 2)
                    .map_err(|e| Error::InvalidArgument(e.to_string()))?;
                circles.push((courtyard_of(circle.layer()), circle.center(), diameter));
            }
        }
        let mut circles = circles.into_iter();
        let mut remove = Vec::new();
        for circle in fpt.circles_mut().iter_mut() {
            if circle.layer().is_package_courtyard() {
                if let Some((layer, center, diameter)) = circles.next() {
                    circle.set_layer(layer);
                    circle.set_line_width(UnsignedLength::default());
                    circle.set_center(center);
                    circle.set_diameter(diameter);
                } else {
                    remove.push(circle.uuid());
                }
                modified = true;
            }
        }
        for uuid in remove {
            fpt.circles_mut().take_by_uuid(&uuid);
        }
        for (layer, center, diameter) in circles {
            fpt.circles_mut().push(Circle::new(
                Uuid::new_random(),
                layer,
                UnsignedLength::default(),
                false,
                false,
                center,
                diameter,
            ));
            modified = true;
        }
        Ok(modified)
    }
}

// --- Clipboard ---

/// The MIME type of footprint clipboard data (upstream
/// `FootprintClipboardData::getMimeType()`).
pub fn footprint_clipboard_mime_type(app_version: &str) -> String {
    format!("application/x-librepcb-clipboard.footprint; version={app_version}")
}

/// Copied footprint objects (upstream `FootprintClipboardData`).
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintClipboardData {
    /// The footprint the objects were copied from.
    pub footprint_uuid: Uuid,
    /// The package pads (to map pads by name when pasting).
    pub package_pads: PackagePadList,
    /// The cursor position when copying.
    pub cursor_pos: Point,
    /// Pads.
    pub pads: FootprintPadList,
    /// Polygons.
    pub polygons: PolygonList,
    /// Circles.
    pub circles: CircleList,
    /// Stroke texts.
    pub stroke_texts: StrokeTextList,
    /// Zones.
    pub zones: ZoneList,
    /// Holes.
    pub holes: HoleList,
}

impl FootprintClipboardData {
    /// Empty data.
    pub fn new(footprint_uuid: Uuid, package_pads: PackagePadList, cursor_pos: Point) -> Self {
        Self {
            footprint_uuid,
            package_pads,
            cursor_pos,
            pads: FootprintPadList::new(),
            polygons: PolygonList::new(),
            circles: CircleList::new(),
            stroke_texts: StrokeTextList::new(),
            zones: ZoneList::new(),
            holes: HoleList::new(),
        }
    }

    /// Copies `items` of footprint `footprint` of `package`.
    pub fn from_items(
        package: &Package,
        footprint: Uuid,
        items: &BTreeSet<FootprintItem>,
        cursor_pos: Point,
    ) -> Result<Self> {
        let fpt = package
            .footprints()
            .by_uuid(&footprint)
            .ok_or_else(|| not_found("footprint", footprint))?;
        let mut data = Self::new(footprint, package.pads().clone(), cursor_pos);
        for o in fpt.pads().iter() {
            if items.contains(&FootprintItem::Pad(o.uuid())) {
                data.pads.push(o.clone());
            }
        }
        for o in fpt.circles().iter() {
            if items.contains(&FootprintItem::Circle(o.uuid())) {
                data.circles.push(o.clone());
            }
        }
        for o in fpt.polygons().iter() {
            if items.contains(&FootprintItem::Polygon(o.uuid())) {
                data.polygons.push(o.clone());
            }
        }
        for o in fpt.stroke_texts().iter() {
            if items.contains(&FootprintItem::StrokeText(o.uuid())) {
                data.stroke_texts.push(o.clone());
            }
        }
        for o in fpt.zones().iter() {
            if items.contains(&FootprintItem::Zone(o.uuid())) {
                data.zones.push(o.clone());
            }
        }
        for o in fpt.holes().iter() {
            if items.contains(&FootprintItem::Hole(o.uuid())) {
                data.holes.push(o.clone());
            }
        }
        Ok(data)
    }

    /// Number of objects.
    pub fn item_count(&self) -> usize {
        self.pads.len()
            + self.polygons.len()
            + self.circles.len()
            + self.stroke_texts.len()
            + self.zones.len()
            + self.holes.len()
    }

    /// Serializes the data (upstream `toMimeData()`).
    pub fn to_sexpression(&self) -> SExpression {
        let mut root = List::new("librepcb_clipboard_footprint");
        root.ensure_line_break();
        self.cursor_pos
            .serialize(root.append_list("cursor_position"));
        root.ensure_line_break();
        root.append_child("footprint", &self.footprint_uuid);
        root.ensure_line_break();
        self.package_pads.serialize(root.append_list("package"));
        root.ensure_line_break();
        self.pads.serialize(&mut root);
        root.ensure_line_break();
        self.polygons.serialize(&mut root);
        root.ensure_line_break();
        self.circles.serialize(&mut root);
        root.ensure_line_break();
        self.stroke_texts.serialize(&mut root);
        root.ensure_line_break();
        self.zones.serialize(&mut root);
        root.ensure_line_break();
        self.holes.serialize(&mut root);
        root.ensure_line_break();
        SExpression::List(root)
    }

    /// The clipboard content (the S-expression as UTF-8).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.to_sexpression().to_byte_array(Mode::LibrePcb)?)
    }

    /// Loads clipboard content.
    pub fn from_bytes(content: &[u8]) -> Result<Self> {
        let root = SExpression::parse(content, None, Mode::LibrePcb)?;
        Ok(Self {
            footprint_uuid: root.child_value("footprint/@0")?,
            package_pads: PackagePadList::deserialize(root.required_child("package")?)?,
            cursor_pos: Point::deserialize(root.required_child("cursor_position")?)?,
            pads: FootprintPadList::deserialize(&root)?,
            polygons: PolygonList::deserialize(&root)?,
            circles: CircleList::deserialize(&root)?,
            stroke_texts: StrokeTextList::deserialize(&root)?,
            zones: ZoneList::deserialize(&root)?,
            holes: HoleList::deserialize(&root)?,
        })
    }
}

/// Pastes objects into a footprint (upstream `CmdPasteFootprintItems`):
/// objects get new UUIDs if they exist already or come from another
/// footprint; pads are connected to the package pads with the same name.
/// Returns the pasted items.
#[derive(Debug, Clone, PartialEq)]
pub struct PasteFootprintItems {
    /// The footprint.
    pub footprint: Uuid,
    /// The objects.
    pub data: FootprintClipboardData,
    /// Offset added to all positions.
    pub offset: Point,
}

impl ElementCommand<Package> for PasteFootprintItems {
    type Output = BTreeSet<FootprintItem>;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdPasteFootprintItems",
            "Paste Footprint Elements"
        )
    }

    fn execute(self, package: &mut Package) -> Result<BTreeSet<FootprintItem>> {
        let data = self.data;
        let offset = self.offset;
        let pkg_pads = package.pads().clone();
        let fpt = footprint_mut(package, self.footprint)?;
        let other = fpt.uuid() != data.footprint_uuid;
        let new_uuid = |exists: bool, uuid: Uuid| {
            if exists || other {
                Uuid::new_random()
            } else {
                uuid
            }
        };
        let mut pasted = BTreeSet::new();
        for pad in data.pads.sorted_by_uuid().iter() {
            let uuid = new_uuid(fpt.pads().contains_uuid(&pad.uuid()), pad.uuid());
            let pkg_pad = pad
                .package_pad_uuid()
                .and_then(|u| data.package_pads.by_uuid(&u))
                .and_then(|p| pkg_pads.by_name(p.name().as_str(), true))
                .map(|p| p.uuid());
            let mut copy = pad.with_uuid(uuid);
            copy.set_package_pad_uuid(pkg_pad);
            copy.translate(offset);
            fpt.pads_mut().push(copy);
            pasted.insert(FootprintItem::Pad(uuid));
        }
        for circle in data.circles.sorted_by_uuid().iter() {
            let uuid = new_uuid(fpt.circles().contains_uuid(&circle.uuid()), circle.uuid());
            let mut copy = circle.with_uuid(uuid);
            copy.translate(offset);
            fpt.circles_mut().push(copy);
            pasted.insert(FootprintItem::Circle(uuid));
        }
        for polygon in data.polygons.sorted_by_uuid().iter() {
            let uuid = new_uuid(
                fpt.polygons().contains_uuid(&polygon.uuid()),
                polygon.uuid(),
            );
            let mut copy = polygon.with_uuid(uuid);
            copy.translate(offset);
            fpt.polygons_mut().push(copy);
            pasted.insert(FootprintItem::Polygon(uuid));
        }
        for text in data.stroke_texts.sorted_by_uuid().iter() {
            let uuid = new_uuid(fpt.stroke_texts().contains_uuid(&text.uuid()), text.uuid());
            let mut copy = text.with_uuid(uuid);
            copy.translate(offset);
            fpt.stroke_texts_mut().push(copy);
            pasted.insert(FootprintItem::StrokeText(uuid));
        }
        for zone in data.zones.sorted_by_uuid().iter() {
            let uuid = new_uuid(fpt.zones().contains_uuid(&zone.uuid()), zone.uuid());
            let mut copy = zone.with_uuid(uuid);
            copy.translate(offset);
            fpt.zones_mut().push(copy);
            pasted.insert(FootprintItem::Zone(uuid));
        }
        for hole in data.holes.sorted_by_uuid().iter() {
            let uuid = new_uuid(fpt.holes().contains_uuid(&hole.uuid()), hole.uuid());
            let mut copy = hole.with_uuid(uuid);
            copy.translate(offset);
            fpt.holes_mut().push(copy);
            pasted.insert(FootprintItem::Hole(uuid));
        }
        Ok(pasted)
    }
}
