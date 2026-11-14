//! Port of libs/librepcb/core/job/outputjob.{h,cpp}.
//!
//! Differences to upstream:
//! - The class hierarchy (`OutputJob` base class with one subclass per job
//!   type) is replaced by the struct [`OutputJob`] with the common data
//!   (UUID, name, forward compatibility options) and the enum
//!   [`OutputJobKind`] holding the type specific settings, so a job runner
//!   can `match` on the kind. The `deserialize<std::shared_ptr<OutputJob>>()`
//!   factory is [`OutputJob::deserialize()`].
//! - `ObjectSet<T>` (the all/default/custom flags) is the enum
//!   [`ObjectSet`].
//! - Signals (`onEdited`) are not ported; setters return whether the value
//!   changed. Icons (`getTypeIcon()`) are not ported (UI).
//! - Unlike upstream's copy constructor, `Clone` also copies the forward
//!   compatibility options (see COMPAT.md).

use std::collections::{BTreeMap, BTreeSet};

use super::{
    ArchiveOutputJob, Board3DOutputJob, BomOutputJob, CopyOutputJob, GerberExcellonOutputJob,
    GerberX3OutputJob, GraphicsOutputJob, InteractiveHtmlBomOutputJob, LppzOutputJob,
    NetlistOutputJob, PickPlaceOutputJob, ProjectJsonOutputJob, UnknownOutputJob,
};
use crate::geometry::object_list;
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::types::{ElementName, Uuid};

/// A set of objects (boards, assembly variants) an output job is run for
/// (upstream `OutputJob::ObjectSet<T>`).
///
/// Serialized as one `(<key> all)`, one `(<key> default)` or one
/// `(<key> <value>)` per custom value (sorted).
///
/// Serde: externally tagged (`"All"`, `"Default"`, `{"Custom": [...]}`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ObjectSet<T: Ord> {
    /// All objects.
    All,
    /// The default object (e.g. the primary board).
    Default,
    /// A custom set of objects (`None` elements mean "no object", e.g.
    /// "no assembly variant").
    Custom(BTreeSet<T>),
}

impl<T: Ord> ObjectSet<T> {
    /// Creates a custom set (upstream `ObjectSet::set()`).
    pub fn custom(values: impl IntoIterator<Item = T>) -> Self {
        Self::Custom(values.into_iter().collect())
    }

    /// Returns whether all objects are selected.
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// Returns whether only the default object is selected.
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }

    /// Returns whether a custom set is selected.
    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Returns the custom set (upstream `getSet()`), `None` for
    /// [`All`](Self::All) and [`Default`](Self::Default).
    pub fn custom_set(&self) -> Option<&BTreeSet<T>> {
        match self {
            Self::Custom(set) => Some(set),
            _ => None,
        }
    }
}

impl<T: Ord + FromSExpression> ObjectSet<T> {
    /// Loads the set from the `key` children of `node` (upstream
    /// constructor `ObjectSet(node, childName)`).
    pub fn deserialize(node: &SExpression, key: &str) -> serialization::Result<Self> {
        match node.child(&format!("{key}/@0")).map(SExpression::value) {
            Some(Ok("all")) => Ok(Self::All),
            Some(Ok("default")) => Ok(Self::Default),
            _ => Ok(Self::Custom(
                node.children_named(key)
                    .map(|child| child.child_value("@0"))
                    .collect::<serialization::Result<_>>()?,
            )),
        }
    }
}

impl<T: Ord + ToSExpression> ObjectSet<T> {
    /// Appends the set as `key` children to `root` (upstream `serialize()`).
    pub fn serialize(&self, root: &mut List, key: &str) {
        match self {
            Self::All => {
                root.ensure_line_break();
                root.append_child(key, &SExpression::token("all"));
            }
            Self::Default => {
                root.ensure_line_break();
                root.append_child(key, &SExpression::token("default"));
            }
            Self::Custom(set) => {
                for value in set {
                    root.ensure_line_break();
                    root.append_child(key, value);
                }
            }
        }
        root.ensure_line_break();
    }
}

/// Common interface of the type specific settings of output jobs (the
/// upstream `OutputJob` subclasses).
pub trait OutputJobType: Clone + Into<OutputJobKind> + SerializeObject + DeserializeObject {
    /// The type name in files, e.g. `"gerber_excellon"` (upstream
    /// `getTypeName()`).
    const TYPE_NAME: &'static str;

    /// Returns the translated type name for the UI (upstream
    /// `getTypeTrStatic()`).
    fn type_tr() -> String;

    /// Returns the (translated) name of new jobs of this type.
    fn default_name() -> ElementName;
}

/// The type specific part of an [`OutputJob`].
///
/// Serde: externally tagged enum.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
// Jobs are few and not moved around in hot paths; boxing the larger
// variants would only complicate matching on them.
#[allow(clippy::large_enum_variant)]
pub enum OutputJobKind {
    /// PDF/SVG/image export of schematics and boards.
    Graphics(GraphicsOutputJob),
    /// Gerber/Excellon fabrication data.
    GerberExcellon(GerberExcellonOutputJob),
    /// Pick&place CSV files.
    PickPlace(PickPlaceOutputJob),
    /// Pick&place and glue mask Gerber X3 files.
    GerberX3(GerberX3OutputJob),
    /// IPC-D-356A netlist.
    Netlist(NetlistOutputJob),
    /// Bill of materials CSV.
    Bom(BomOutputJob),
    /// Interactive HTML bill of materials.
    InteractiveHtmlBom(InteractiveHtmlBomOutputJob),
    /// 3D model (STEP) of boards.
    Board3D(Board3DOutputJob),
    /// Project data as JSON.
    ProjectJson(ProjectJsonOutputJob),
    /// Project archive (`*.lppz`).
    Lppz(LppzOutputJob),
    /// Copy of a (template) file.
    Copy(CopyOutputJob),
    /// ZIP archive of the output of other jobs.
    Archive(ArchiveOutputJob),
    /// A job of a type unknown to this version (kept verbatim).
    Unknown(UnknownOutputJob),
}

/// Calls `$f` with the type specific settings as `$job` and its type as
/// `$ty`, for each known kind; `$unknown` handles unknown jobs.
macro_rules! dispatch {
    ($kind:expr, $job:ident => $f:expr, $unknown:ident => $u:expr) => {
        match $kind {
            OutputJobKind::Graphics($job) => $f,
            OutputJobKind::GerberExcellon($job) => $f,
            OutputJobKind::PickPlace($job) => $f,
            OutputJobKind::GerberX3($job) => $f,
            OutputJobKind::Netlist($job) => $f,
            OutputJobKind::Bom($job) => $f,
            OutputJobKind::InteractiveHtmlBom($job) => $f,
            OutputJobKind::Board3D($job) => $f,
            OutputJobKind::ProjectJson($job) => $f,
            OutputJobKind::Lppz($job) => $f,
            OutputJobKind::Copy($job) => $f,
            OutputJobKind::Archive($job) => $f,
            OutputJobKind::Unknown($unknown) => $u,
        }
    };
}

/// Returns `T::TYPE_NAME` for the type of a value.
fn type_name_of<T: OutputJobType>(_: &T) -> &'static str {
    T::TYPE_NAME
}

/// Returns `T::type_tr()` for the type of a value.
fn type_tr_of<T: OutputJobType>(_: &T) -> String {
    T::type_tr()
}

/// Returns `T::default_name()` for the type of a value.
fn default_name_of<T: OutputJobType>(_: &T) -> ElementName {
    T::default_name()
}

impl OutputJobKind {
    /// Returns the type name in files (upstream `getType()`), e.g.
    /// `"graphics"`.
    pub fn type_name(&self) -> &str {
        dispatch!(self, job => type_name_of(job), unknown => unknown.type_name())
    }

    /// Returns the translated type name for the UI (upstream
    /// `getTypeTr()`).
    pub fn type_tr(&self) -> String {
        dispatch!(self, job => type_tr_of(job), unknown => unknown.type_tr())
    }

    /// Returns the (translated) name of new jobs of this kind, `None` for
    /// unknown jobs.
    pub fn default_name(&self) -> Option<ElementName> {
        dispatch!(self, job => Some(default_name_of(job)), _unknown => None)
    }

    /// Returns the UUIDs of the jobs this job depends on (upstream
    /// `getDependencies()`).
    pub fn dependencies(&self) -> BTreeSet<Uuid> {
        match self {
            Self::Archive(job) => job.input_jobs.keys().copied().collect(),
            _ => BTreeSet::new(),
        }
    }

    /// Removes the dependency to the job with the UUID `job` (upstream
    /// `removeDependency()`), returns whether something changed.
    pub fn remove_dependency(&mut self, job: &Uuid) -> bool {
        match self {
            Self::Archive(archive) => archive.input_jobs.remove(job).is_some(),
            _ => false,
        }
    }

    /// Deserializes the type specific settings of the job `node` of type
    /// `type_name` (upstream `deserialize<std::shared_ptr<OutputJob>>()`).
    fn deserialize(type_name: &str, node: &SExpression) -> serialization::Result<Self> {
        fn load<T: OutputJobType>(node: &SExpression) -> serialization::Result<OutputJobKind> {
            T::deserialize(node).map(Into::into)
        }
        match type_name {
            GraphicsOutputJob::TYPE_NAME => load::<GraphicsOutputJob>(node),
            GerberExcellonOutputJob::TYPE_NAME => load::<GerberExcellonOutputJob>(node),
            PickPlaceOutputJob::TYPE_NAME => load::<PickPlaceOutputJob>(node),
            GerberX3OutputJob::TYPE_NAME => load::<GerberX3OutputJob>(node),
            NetlistOutputJob::TYPE_NAME => load::<NetlistOutputJob>(node),
            BomOutputJob::TYPE_NAME => load::<BomOutputJob>(node),
            InteractiveHtmlBomOutputJob::TYPE_NAME => load::<InteractiveHtmlBomOutputJob>(node),
            Board3DOutputJob::TYPE_NAME => load::<Board3DOutputJob>(node),
            ProjectJsonOutputJob::TYPE_NAME => load::<ProjectJsonOutputJob>(node),
            LppzOutputJob::TYPE_NAME => load::<LppzOutputJob>(node),
            CopyOutputJob::TYPE_NAME => load::<CopyOutputJob>(node),
            ArchiveOutputJob::TYPE_NAME => load::<ArchiveOutputJob>(node),
            _ => Ok(Self::Unknown(UnknownOutputJob::new(
                type_name,
                node.clone(),
            ))),
        }
    }
}

macro_rules! impl_from_kind {
    ($($variant:ident($ty:ident)),* $(,)?) => {
        $(
            impl From<$ty> for OutputJobKind {
                fn from(job: $ty) -> Self {
                    Self::$variant(job)
                }
            }
        )*
    };
}

impl_from_kind!(
    Graphics(GraphicsOutputJob),
    GerberExcellon(GerberExcellonOutputJob),
    PickPlace(PickPlaceOutputJob),
    GerberX3(GerberX3OutputJob),
    Netlist(NetlistOutputJob),
    Bom(BomOutputJob),
    InteractiveHtmlBom(InteractiveHtmlBomOutputJob),
    Board3D(Board3DOutputJob),
    ProjectJson(ProjectJsonOutputJob),
    Lppz(LppzOutputJob),
    Copy(CopyOutputJob),
    Archive(ArchiveOutputJob),
    Unknown(UnknownOutputJob),
);

/// An output job of a project (or an organization template): common data
/// plus the type specific settings ([`OutputJobKind`]).
///
/// Serde: an object with the fields `uuid`, `name`, `options` and `kind`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutputJob {
    uuid: Uuid,
    name: ElementName,
    /// Arbitrary options for forward compatibility (`option` children, by
    /// their first value), written back unchanged.
    options: BTreeMap<String, Vec<SExpression>>,
    kind: OutputJobKind,
}

crate::geometry::impl_uuid!(OutputJob);

impl OutputJob {
    /// Creates a job.
    pub fn new(uuid: Uuid, name: ElementName, kind: impl Into<OutputJobKind>) -> Self {
        Self {
            uuid,
            name,
            options: BTreeMap::new(),
            kind: kind.into(),
        }
    }

    /// Creates a job of type `T` with the upstream default settings, a
    /// random UUID and the default name (upstream default constructors).
    pub fn new_default<T: OutputJobType + Default>() -> Self {
        Self::new(Uuid::new_random(), T::default_name(), T::default())
    }

    /// Returns the name.
    pub fn name(&self) -> &ElementName {
        &self.name
    }

    /// Sets the name, returns whether it changed.
    pub fn set_name(&mut self, name: ElementName) -> bool {
        if name == self.name {
            return false;
        }
        self.name = name;
        // upstream: emits onEdited(NameChanged)
        true
    }

    /// Sets the UUID, returns whether it changed.
    pub fn set_uuid(&mut self, uuid: Uuid) -> bool {
        if uuid == self.uuid {
            return false;
        }
        self.uuid = uuid;
        // upstream: emits onEdited(UuidChanged)
        true
    }

    /// Returns the forward compatibility options (by option name).
    pub fn options(&self) -> &BTreeMap<String, Vec<SExpression>> {
        &self.options
    }

    /// Returns the type specific settings.
    pub fn kind(&self) -> &OutputJobKind {
        &self.kind
    }

    /// Returns the type specific settings for modification.
    pub fn kind_mut(&mut self) -> &mut OutputJobKind {
        &mut self.kind
    }

    /// Returns the type name in files, e.g. `"graphics"`.
    pub fn type_name(&self) -> &str {
        self.kind.type_name()
    }

    /// Returns the translated type name (upstream `getTypeTr()`).
    pub fn type_tr(&self) -> String {
        self.kind.type_tr()
    }

    /// Returns the UUIDs of the jobs this job depends on.
    pub fn dependencies(&self) -> BTreeSet<Uuid> {
        self.kind.dependencies()
    }

    /// Removes the dependency to the job `job`, returns whether something
    /// changed.
    pub fn remove_dependency(&mut self, job: &Uuid) -> bool {
        self.kind.remove_dependency(job)
    }
}

impl SerializeObject for OutputJob {
    fn serialize(&self, root: &mut List) {
        if let OutputJobKind::Unknown(unknown) = &self.kind {
            // Upstream `UnknownOutputJob::serialize()`: written verbatim.
            if let Some(node) = unknown.node().as_list() {
                *root = node.clone();
                return;
            }
        }
        root.append_value(&self.uuid);
        root.append_child("name", &self.name);
        root.ensure_line_break();
        root.append_child("type", &SExpression::token(self.kind.type_name()));
        dispatch!(&self.kind, job => job.serialize(root), _unknown => {});
        for node in self.options.values().flatten() {
            root.ensure_line_break();
            root.push(node.clone());
        }
        root.ensure_line_break();
    }
}

impl DeserializeObject for OutputJob {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let type_name: String = node.child_value("type/@0")?;
        let mut options: BTreeMap<String, Vec<SExpression>> = BTreeMap::new();
        for child in node.children_named("option") {
            options
                .entry(child.child_value("@0")?)
                .or_default()
                .push(child.clone());
        }
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
            options,
            kind: OutputJobKind::deserialize(&type_name, node)?,
        })
    }
}

object_list!(
    /// List of output jobs (upstream `OutputJobList`), serialized as `job`
    /// children.
    OutputJobList,
    OutputJobListTag,
    OutputJob,
    "job"
);
