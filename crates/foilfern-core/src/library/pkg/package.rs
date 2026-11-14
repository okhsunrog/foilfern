//! Port of libs/librepcb/core/library/pkg/package.{h,cpp}.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::str::FromStr;

use super::error::Error;
use super::footprint::{Footprint, FootprintList};
use super::package_check::run_package_checks;
use super::package_model::{PackageModel, PackageModelList};
use super::package_pad::PackagePadList;
use crate::fileio::{FileSystem, TransactionalDirectory};
use crate::geometry::property;
use crate::library::{
    BaseMetadata, ElementMetadata, LibraryBaseElement, LibraryCheckMessage, Result,
    element_accessors, impl_library_element,
};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::types::{ElementName, Length, PositiveLength, SimpleString, UnsignedLength, Uuid};

/// An alternative name of a package (e.g. the name used by a manufacturer or
/// standard), upstream `Package::AlternativeName`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlternativeName {
    /// The name.
    pub name: ElementName,
    /// Where the name comes from (e.g. `"JEDEC"`), may be empty.
    pub reference: SimpleString,
}

impl SerializeObject for AlternativeName {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.name);
        root.append_child("reference", &self.reference);
    }
}

impl DeserializeObject for AlternativeName {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            name: node.child_value("@0")?,
            reference: node.child_value("reference/@0")?,
        })
    }
}

/// How a package is mounted on a board (upstream `Package::AssemblyType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AssemblyType {
    /// Nothing to mount (i.e. not a package, just a footprint).
    None,
    /// Pure THT package.
    Tht,
    /// Pure SMT package.
    Smt,
    /// Mixed THT/SMT package.
    Mixed,
    /// Anything special, e.g. mechanical parts.
    Other,
    /// Auto detection (deprecated, only for file format migration).
    Auto,
}

impl AssemblyType {
    /// All values.
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Tht,
        Self::Smt,
        Self::Mixed,
        Self::Other,
        Self::Auto,
    ];

    /// Returns the serialization token.
    pub fn to_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Tht => "tht",
            Self::Smt => "smt",
            Self::Mixed => "mixed",
            Self::Other => "other",
            Self::Auto => "auto",
        }
    }
}

impl fmt::Display for AssemblyType {
    /// Formats the serialization token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_str())
    }
}

impl FromStr for AssemblyType {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::ALL
            .into_iter()
            .find(|t| t.to_str() == s)
            .ok_or_else(|| Error::UnknownAssemblyType(s.to_owned()))
    }
}

impl ToSExpression for AssemblyType {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_str())
    }
}

impl FromSExpression for AssemblyType {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

/// A package of a component (pads, footprints and 3D models).
///
/// The UUID, the package pads (their UUIDs, neither adding nor removing
/// pads) and the footprints (their UUIDs and pads; adding footprints is
/// allowed) are the interface of a package and must never be changed.
#[derive(Debug)]
pub struct Package {
    directory: TransactionalDirectory,
    metadata: ElementMetadata,
    /// Optional.
    alternative_names: Vec<AlternativeName>,
    assembly_type: AssemblyType,
    grid_interval: PositiveLength,
    /// For the package checks.
    min_copper_clearance: UnsignedLength,
    /// Empty if the package has no pads.
    pads: PackagePadList,
    /// Optional.
    models: PackageModelList,
    /// At least one footprint (checked by the package check).
    footprints: FootprintList,
}

static_assertions::assert_impl_all!(Package: Send, Sync);

impl Package {
    /// Creates a new, empty package in a temporary directory.
    pub fn new(metadata: BaseMetadata, assembly_type: AssemblyType) -> Result<Self> {
        Ok(Self {
            directory: TransactionalDirectory::new_temporary()?,
            metadata: ElementMetadata::new(metadata),
            alternative_names: Vec::new(),
            assembly_type,
            grid_interval: PositiveLength::new(Length::new(2_540_000))
                .expect("constant is positive"),
            min_copper_clearance: UnsignedLength::new(Length::new(200_000))
                .expect("constant is unsigned"),
            pads: PackagePadList::new(),
            models: PackageModelList::new(),
            footprints: FootprintList::new(),
        })
    }

    property!(
        /// Returns the alternative names.
        ref alternative_names: Vec<AlternativeName>, set_alternative_names
    );
    property!(
        /// Returns the grid interval used in the editor.
        copy grid_interval: PositiveLength, set_grid_interval
    );
    property!(
        /// Returns the minimum copper clearance between pads (used by the
        /// package check).
        copy min_copper_clearance: UnsignedLength, set_min_copper_clearance
    );

    /// Returns the assembly type as stored in the file (may be
    /// [`AssemblyType::Auto`]), upstream `getAssemblyType(false)`.
    pub fn assembly_type(&self) -> AssemblyType {
        self.assembly_type
    }

    /// Returns the assembly type, with [`AssemblyType::Auto`] resolved by
    /// [`guess_assembly_type()`](Self::guess_assembly_type), upstream
    /// `getAssemblyType(true)`.
    pub fn resolved_assembly_type(&self) -> AssemblyType {
        match self.assembly_type {
            AssemblyType::Auto => self.guess_assembly_type(),
            other => other,
        }
    }

    /// Sets the assembly type, returns whether it was modified.
    pub fn set_assembly_type(&mut self, value: AssemblyType) -> bool {
        let modified = value != self.assembly_type;
        self.assembly_type = value;
        modified
    }

    /// Guesses the assembly type from the pads of the default (first)
    /// footprint.
    pub fn guess_assembly_type(&self) -> AssemblyType {
        // If there are no package pads, probably there's nothing to mount.
        if self.pads.is_empty() {
            return AssemblyType::None;
        }

        // Auto-detect based on default footprint pads.
        let mut has_tht_pads = false;
        let mut has_smt_pads = false;
        if let Some(footprint) = self.footprints.first() {
            for pad in footprint.pads().iter().map(|p| p.pad()) {
                if pad.function_needs_soldering() {
                    if pad.is_tht() {
                        has_tht_pads = true;
                    } else {
                        has_smt_pads = true;
                    }
                }
            }
        }
        match (has_tht_pads, has_smt_pads) {
            (true, true) => AssemblyType::Mixed,
            (true, false) => AssemblyType::Tht,
            (false, true) => AssemblyType::Smt,
            (false, false) => AssemblyType::None,
        }
    }

    /// Returns the package pads.
    pub fn pads(&self) -> &PackagePadList {
        &self.pads
    }

    /// Returns the package pads for modification.
    pub fn pads_mut(&mut self) -> &mut PackagePadList {
        // upstream: emits onEdited(PadsEdited) on modifications
        &mut self.pads
    }

    /// Returns the 3D models.
    pub fn models(&self) -> &PackageModelList {
        &self.models
    }

    /// Returns the 3D models for modification.
    pub fn models_mut(&mut self) -> &mut PackageModelList {
        // upstream: emits onEdited(ModelsEdited) on modifications
        &mut self.models
    }

    /// Returns the 3D models used by the footprint `footprint` (in the
    /// order of the package's models, empty if the footprint doesn't
    /// exist).
    pub fn models_for_footprint(&self, footprint: &Uuid) -> Vec<&PackageModel> {
        match self.footprints.by_uuid(footprint) {
            Some(footprint) => self
                .models
                .iter()
                .filter(|m| footprint.models().contains(&m.uuid()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Returns the footprints.
    pub fn footprints(&self) -> &FootprintList {
        &self.footprints
    }

    /// Returns the footprints for modification.
    pub fn footprints_mut(&mut self) -> &mut FootprintList {
        // upstream: emits onEdited(FootprintsEdited) on modifications
        &mut self.footprints
    }

    /// Makes this package a copy of `other` with new UUIDs of all contained
    /// objects (but keeps the package UUID), removing all files of this
    /// package and copying the 3D model files (upstream `duplicateFrom()`).
    ///
    /// Like upstream, translations, tags and message approvals of the
    /// footprints are not copied.
    pub fn duplicate_from(&mut self, other: &Package) -> Result<()> {
        self.directory.remove_dir_recursively("")?;
        self.metadata.duplicate_from(&other.metadata);
        self.alternative_names = other.alternative_names.clone();
        self.assembly_type = other.assembly_type;
        self.grid_interval = other.grid_interval;
        self.min_copper_clearance = other.min_copper_clearance;

        // Copy pads but generate new UUIDs.
        let mut pad_uuid_map = HashMap::new();
        self.pads = other
            .pads
            .iter()
            .map(|pad| {
                let new_uuid = Uuid::new_random();
                pad_uuid_map.insert(pad.uuid(), new_uuid);
                pad.with_uuid(new_uuid)
            })
            .collect();

        // Copy 3D models but generate new UUIDs.
        let mut models_uuid_map = HashMap::new();
        let mut models = PackageModelList::new();
        for model in &other.models {
            let new_model = model.with_uuid(Uuid::new_random());
            models_uuid_map.insert(model.uuid(), new_model.uuid());
            if other.directory.file_exists(&model.file_name()) {
                let content = other.directory.read(&model.file_name())?;
                self.directory.write(&new_model.file_name(), &content)?;
            }
            models.push(new_model);
        }
        self.models = models;

        // Copy footprints but generate new UUIDs.
        self.footprints = other
            .footprints
            .iter()
            .map(|footprint| duplicate_footprint(footprint, &pad_uuid_map, &models_uuid_map))
            .collect();
        Ok(())
    }
}

fn duplicate_footprint(
    footprint: &Footprint,
    pad_uuid_map: &HashMap<Uuid, Uuid>,
    models_uuid_map: &HashMap<Uuid, Uuid>,
) -> Footprint {
    // Don't copy translations as they would need to be adjusted anyway.
    let mut new = Footprint::new(
        Uuid::new_random(),
        footprint.names().default_value().clone(),
        footprint.descriptions().default_value().clone(),
    );
    new.set_model_position(footprint.model_position());
    new.set_model_rotation(footprint.model_rotation());
    // Copy models but with the new UUIDs.
    new.set_models(
        footprint
            .models()
            .iter()
            .filter_map(|uuid| models_uuid_map.get(uuid).copied())
            .collect::<BTreeSet<_>>(),
    );
    // Copy pads but generate new UUIDs (and translate the package pads).
    *new.pads_mut() = footprint
        .pads()
        .iter()
        .map(|pad| {
            let mut new_pad = pad.with_uuid(Uuid::new_random());
            new_pad.set_package_pad_uuid(
                pad.package_pad_uuid()
                    .and_then(|uuid| pad_uuid_map.get(&uuid).copied()),
            );
            new_pad
        })
        .collect();
    *new.polygons_mut() = footprint
        .polygons()
        .iter()
        .map(|o| o.with_uuid(Uuid::new_random()))
        .collect();
    *new.circles_mut() = footprint
        .circles()
        .iter()
        .map(|o| o.with_uuid(Uuid::new_random()))
        .collect();
    *new.stroke_texts_mut() = footprint
        .stroke_texts()
        .iter()
        .map(|o| o.with_uuid(Uuid::new_random()))
        .collect();
    *new.zones_mut() = footprint
        .zones()
        .iter()
        .map(|o| o.with_uuid(Uuid::new_random()))
        .collect();
    *new.holes_mut() = footprint
        .holes()
        .iter()
        .map(|o| o.with_uuid(Uuid::new_random()))
        .collect();
    new
}

impl LibraryBaseElement for Package {
    const SHORT_ELEMENT_NAME: &'static str = "pkg";
    const LONG_ELEMENT_NAME: &'static str = "package";

    element_accessors!();

    fn load(directory: TransactionalDirectory, root: &SExpression) -> Result<Self> {
        Ok(Self {
            directory,
            metadata: ElementMetadata::deserialize(root)?,
            alternative_names: root
                .children_named("alternative_name")
                .map(AlternativeName::deserialize)
                .collect::<serialization::Result<_>>()?,
            assembly_type: root.child_value("assembly_type/@0")?,
            grid_interval: root.child_value("grid_interval/@0")?,
            min_copper_clearance: root.child_value("min_copper_clearance/@0")?,
            pads: PackagePadList::deserialize(root)?,
            models: PackageModelList::deserialize(root)?,
            footprints: FootprintList::deserialize(root)?,
        })
    }

    fn run_checks(&self) -> Result<Vec<LibraryCheckMessage>> {
        let mut msgs = Vec::new();
        run_package_checks(self, &mut msgs)?;
        Ok(msgs)
    }
}

impl_library_element!(Package);

impl SerializeObject for Package {
    fn serialize(&self, root: &mut List) {
        self.metadata.serialize(root);
        for name in &self.alternative_names {
            root.ensure_line_break();
            name.serialize(root.append_list("alternative_name"));
        }
        root.ensure_line_break();
        root.append_child("assembly_type", &self.assembly_type);
        root.ensure_line_break();
        root.append_child("grid_interval", &self.grid_interval);
        root.ensure_line_break();
        root.append_child("min_copper_clearance", &self.min_copper_clearance);
        root.ensure_line_break();
        self.pads.serialize(root);
        root.ensure_line_break();
        self.models.serialize(root);
        root.ensure_line_break();
        self.footprints.serialize(root);
        root.ensure_line_break();
        self.metadata.base().serialize_message_approvals(root);
        root.ensure_line_break();
    }
}
