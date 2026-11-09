//! Port of libs/librepcb/editor/library/cmd/cmdlibrarybaseelementedit.{h,cpp}
//! and cmdlibraryelementedit.{h,cpp}: editing the metadata of a library
//! element (names, descriptions, keywords, version, author, deprecation,
//! categories, resources).

use std::collections::BTreeSet;

use librepcb_core::library::ResourceList;
use librepcb_core::serialization::{
    LocalizedDescriptionMap, LocalizedKeywordsMap, LocalizedNameMap,
};
use librepcb_core::types::{ElementName, Uuid, Version};
use librepcb_i18n::tr;

use crate::error::Result;
use crate::library_editor::{EditableElement, ElementCommand};

/// Edits the metadata of a library element; `None` keeps a value (upstream
/// `CmdLibraryElementEdit`, texts of the element tabs' `commitUiData()`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditElementMetadata {
    /// The undo text (default: "Edit <Element> Properties").
    pub text: Option<String>,
    /// New default (en_US) name.
    pub name: Option<ElementName>,
    /// New names (all locales, replaces `name`).
    pub names: Option<LocalizedNameMap>,
    /// New default (en_US) description.
    pub description: Option<String>,
    /// New descriptions (all locales).
    pub descriptions: Option<LocalizedDescriptionMap>,
    /// New default (en_US) keywords.
    pub keywords: Option<String>,
    /// New keywords (all locales).
    pub all_keywords: Option<LocalizedKeywordsMap>,
    /// New version.
    pub version: Option<Version>,
    /// New author.
    pub author: Option<String>,
    /// New deprecation state.
    pub deprecated: Option<bool>,
    /// New generator.
    pub generated_by: Option<String>,
    /// New categories.
    pub categories: Option<BTreeSet<Uuid>>,
    /// New resources.
    pub resources: Option<ResourceList>,
}

/// The default undo text of a metadata edit of element type `E`.
fn default_text<E: EditableElement>() -> String {
    match E::TAB_CONTEXT {
        "SymbolTab" => tr!("librepcb::editor::SymbolTab", "Edit Symbol Properties"),
        "PackageTab" => tr!(
            "librepcb::editor::CmdPackageEdit",
            "Edit Package Properties"
        ),
        "ComponentTab" => tr!(
            "librepcb::editor::CmdComponentEdit",
            "Edit Component Properties"
        ),
        _ => tr!("librepcb::editor::CmdDeviceEdit", "Edit Device Properties"),
    }
}

impl<E: EditableElement> ElementCommand<E> for EditElementMetadata {
    type Output = ();

    fn text(&self) -> String {
        self.text.clone().unwrap_or_else(default_text::<E>)
    }

    fn execute(self, element: &mut E) -> Result<()> {
        let base = element.metadata_mut();
        if let Some(v) = self.names {
            base.set_names(v);
        }
        if let Some(v) = self.name {
            let mut names = base.names().clone();
            names.set_default_value(v);
            base.set_names(names);
        }
        if let Some(v) = self.descriptions {
            base.set_descriptions(v);
        }
        if let Some(v) = self.description {
            let mut map = base.descriptions().clone();
            map.set_default_value(v);
            base.set_descriptions(map);
        }
        if let Some(v) = self.all_keywords {
            base.set_keywords(v);
        }
        if let Some(v) = self.keywords {
            let mut map = base.keywords().clone();
            map.set_default_value(v);
            base.set_keywords(map);
        }
        if let Some(v) = self.version {
            base.set_version(v);
        }
        if let Some(v) = self.author {
            base.set_author(v);
        }
        if let Some(v) = self.deprecated {
            base.set_deprecated(v);
        }
        let meta = element.element_metadata_mut();
        if let Some(v) = self.generated_by {
            meta.set_generated_by(v);
        }
        if let Some(v) = self.categories {
            meta.set_categories(v);
        }
        if let Some(v) = self.resources {
            meta.set_resources(v);
        }
        Ok(())
    }
}
