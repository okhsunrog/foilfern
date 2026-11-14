//! Kinds of library elements stored in the workspace library database
//! (replaces the element type template parameters of upstream
//! `WorkspaceLibraryDb` / `WorkspaceLibraryDbWriter`, and their
//! `getElementTable<T>()` / `getCategoryTable<T>()` helpers).

use std::fmt;
use std::str::FromStr;

/// Kind of a library element (or library) in the workspace library
/// database.
///
/// Serialized (serde, [`Display`](fmt::Display), [`FromStr`]) as snake case
/// name, e.g. `"component_category"`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    /// Libraries (`*.lplib`).
    Library,
    /// Component categories.
    ComponentCategory,
    /// Package categories.
    PackageCategory,
    /// Symbols.
    Symbol,
    /// Packages.
    Package,
    /// Components.
    Component,
    /// Devices.
    Device,
    /// Organizations.
    Organization,
}

impl ElementKind {
    /// All kinds.
    pub const ALL: [ElementKind; 8] = [
        Self::Library,
        Self::ComponentCategory,
        Self::PackageCategory,
        Self::Symbol,
        Self::Package,
        Self::Component,
        Self::Device,
        Self::Organization,
    ];

    /// The kinds which can be placed in schematics or boards or are needed
    /// for that (symbols, packages, components, devices).
    pub const ELEMENTS: [ElementKind; 4] =
        [Self::Symbol, Self::Package, Self::Component, Self::Device];

    /// Returns the database table (upstream `getElementTable<T>()`).
    pub fn table(self) -> &'static str {
        match self {
            Self::Library => "libraries",
            Self::ComponentCategory => "component_categories",
            Self::PackageCategory => "package_categories",
            Self::Symbol => "symbols",
            Self::Package => "packages",
            Self::Component => "components",
            Self::Device => "devices",
            Self::Organization => "organizations",
        }
    }

    /// Returns the snake case name (e.g. `"component_category"`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::ComponentCategory => "component_category",
            Self::PackageCategory => "package_category",
            Self::Symbol => "symbol",
            Self::Package => "package",
            Self::Component => "component",
            Self::Device => "device",
            Self::Organization => "organization",
        }
    }

    /// Returns the kind of categories elements of this kind are assigned to
    /// (upstream `getCategoryTable<T>()`), or `None` for kinds without
    /// categories.
    pub fn category_kind(self) -> Option<ElementKind> {
        match self {
            Self::Symbol | Self::Component | Self::Device => Some(Self::ComponentCategory),
            Self::Package => Some(Self::PackageCategory),
            _ => None,
        }
    }

    /// Returns whether this is a category kind.
    pub fn is_category(self) -> bool {
        matches!(self, Self::ComponentCategory | Self::PackageCategory)
    }

    /// Returns whether elements of this kind have resources (components and
    /// devices).
    pub fn has_resources(self) -> bool {
        matches!(self, Self::Component | Self::Device)
    }

    /// Returns whether elements of this kind have a `generated_by` column.
    pub fn has_generated_by(self) -> bool {
        matches!(
            self,
            Self::Symbol | Self::Package | Self::Component | Self::Device
        )
    }
}

impl fmt::Display for ElementKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for ElementKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|k| k.name() == s)
            .ok_or_else(|| format!("Unknown library element kind: {s}"))
    }
}
