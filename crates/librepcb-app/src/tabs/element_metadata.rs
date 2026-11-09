//! The metadata part of the library element tabs (symbols, packages,
//! components, devices): name, description, keywords, author, version,
//! deprecated and the categories with the category chooser.
//!
//! Port of the metadata handling of upstream `SymbolTab`, `PackageTab`,
//! `ComponentTab` and `DeviceTab` (`refreshUiData()`, `setDerivedUiData()`,
//! `commitUiData()`) and of
//! libs/librepcb/editor/library/libraryelementcategoriesmodel.{h,cpp} and
//! the category tree of `CategoryTreeModel`.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use chrono::Utc;
use librepcb_app_ui as ui;
use librepcb_core::library::LibraryElement;
use librepcb_core::types::{ElementName, Uuid, Version};
use librepcb_core::workspace::{CategoryTreeNode, ElementKind, LibraryDb};
use librepcb_editor::library_editor::commands::EditElementMetadata;
use librepcb_i18n::tr;

use crate::models::{UiModel, vec_model};
use crate::validation;

/// The metadata as edited in the UI.
pub struct ElementMetadataUi {
    db: Arc<LibraryDb>,
    locales: Vec<String>,
    category_kind: ElementKind,
    /// Name (text).
    pub name: String,
    /// Name error.
    pub name_error: String,
    name_parsed: Option<ElementName>,
    /// Description.
    pub description: String,
    /// Keywords.
    pub keywords: String,
    /// Author.
    pub author: String,
    /// Version (text).
    pub version: String,
    /// Version error.
    pub version_error: String,
    version_parsed: Option<Version>,
    /// Deprecated.
    pub deprecated: bool,
    /// Age of the element in days.
    pub age_days: f32,
    categories: BTreeSet<Uuid>,
    categories_model: Rc<UiModel<ui::LibraryElementCategoryData>>,
    categories_tree: Rc<UiModel<ui::TreeViewItemData>>,
    /// Whether the category chooser is shown.
    pub choose_category: bool,
}

impl ElementMetadataUi {
    /// Metadata of elements with categories of `category_kind` (component
    /// or package categories; categories have parent categories of their
    /// own kind). `on_category_row` is called with category rows written
    /// by the UI (the "delete" flag).
    pub fn new(
        db: Arc<LibraryDb>,
        locales: Vec<String>,
        category_kind: ElementKind,
        on_category_row: impl Fn(usize, ui::LibraryElementCategoryData) + 'static,
    ) -> Self {
        let categories_model = UiModel::shared(Vec::new());
        categories_model.set_handler(on_category_row);
        let mut m = Self {
            db,
            locales,
            category_kind,
            name: String::new(),
            name_error: String::new(),
            name_parsed: None,
            description: String::new(),
            keywords: String::new(),
            author: String::new(),
            version: String::new(),
            version_error: String::new(),
            version_parsed: None,
            deprecated: false,
            age_days: 0.0,
            categories: BTreeSet::new(),
            categories_model,
            categories_tree: UiModel::shared(Vec::new()),
            choose_category: false,
        };
        m.refresh_categories_tree();
        m
    }

    /// Updates the UI data from the element (upstream `refreshUiData()`).
    pub fn refresh<E: LibraryElement>(&mut self, element: &E) {
        let m = element.metadata();
        self.name = m.name().to_string();
        self.name_error.clear();
        self.name_parsed = Some(m.name().clone());
        self.description = m.descriptions().default_value().clone();
        self.keywords = m.keywords().default_value().clone();
        self.author = m.author().clone();
        self.version = m.version().to_string();
        self.version_error.clear();
        self.version_parsed = Some(m.version().clone());
        self.deprecated = m.is_deprecated();
        let age = Utc::now().signed_duration_since(m.created());
        self.age_days = age.num_seconds() as f32 / 86_400.0;
        self.set_categories(element.element_metadata().categories().clone());
    }

    /// Clears the name (new elements: the user starts typing).
    pub fn clear_name(&mut self) {
        self.name.clear();
        validation::element_name(&self.name, &mut self.name_error);
        self.name_parsed = None;
    }

    /// Applies text inputs of the UI (upstream `setDerivedUiData()`).
    #[allow(clippy::too_many_arguments)]
    pub fn set_from_ui(
        &mut self,
        name: &str,
        description: &str,
        keywords: &str,
        author: &str,
        version: &str,
        deprecated: bool,
        new_category: &str,
        choose_category: bool,
    ) -> bool {
        self.name = name.to_owned();
        if let Some(n) = validation::element_name(&self.name, &mut self.name_error) {
            self.name_parsed = Some(n);
        }
        self.description = description.to_owned();
        self.keywords = keywords.to_owned();
        self.author = author.to_owned();
        self.version = version.to_owned();
        if let Some(v) = validation::version(&self.version, &mut self.version_error) {
            self.version_parsed = Some(v);
        }
        self.deprecated = deprecated;
        self.choose_category = choose_category;
        // A category chosen in the dialog: added right away (upstream
        // `LibraryElementCategoriesModel::add()` emits `modified`).
        match new_category.parse::<Uuid>() {
            Ok(uuid) if self.categories.insert(uuid) => {
                self.update_categories_model();
                true
            }
            _ => false,
        }
    }

    /// A category row was written by the UI; returns whether the categories
    /// changed (the "delete" flag).
    pub fn category_row_written(
        &mut self,
        row: usize,
        data: &ui::LibraryElementCategoryData,
    ) -> bool {
        if !data.delete {
            return false;
        }
        let Some(uuid) = self
            .categories_model
            .get(row)
            .and_then(|r| r.uuid.parse::<Uuid>().ok())
        else {
            return false;
        };
        let removed = self.categories.remove(&uuid);
        self.update_categories_model();
        removed
    }

    /// The categories.
    pub fn categories(&self) -> &BTreeSet<Uuid> {
        &self.categories
    }

    /// Sets the categories (from the element).
    pub fn set_categories(&mut self, categories: BTreeSet<Uuid>) {
        if categories != self.categories || self.categories_model.len() != categories.len() {
            self.categories = categories;
            self.update_categories_model();
        }
    }

    /// The command applying the edited metadata (upstream
    /// `commitUiData()`, `CmdLibraryElementEdit`); `None` values keep the
    /// element's.
    pub fn command<E: LibraryElement>(&self, element: &E) -> EditElementMetadata {
        let m = element.metadata();
        EditElementMetadata {
            name: self.name_parsed.clone(),
            description: (self.description != *m.descriptions().default_value())
                .then(|| self.description.trim().to_owned()),
            keywords: (self.keywords != *m.keywords().default_value())
                .then(|| validation::clean_keywords(&self.keywords)),
            author: (self.author != *m.author()).then(|| self.author.trim().to_owned()),
            version: self.version_parsed.clone(),
            deprecated: Some(self.deprecated),
            categories: (self.categories != *element.element_metadata().categories())
                .then(|| self.categories.clone()),
            ..EditElementMetadata::default()
        }
    }

    /// The categories model (`categories`).
    pub fn categories_model(&self) -> slint::ModelRc<ui::LibraryElementCategoryData> {
        crate::models::model_rc(&self.categories_model)
    }

    /// The category tree of the chooser (`categories-tree`).
    pub fn categories_tree(&self) -> slint::ModelRc<ui::TreeViewItemData> {
        crate::models::model_rc(&self.categories_tree)
    }

    /// Upstream `LibraryElementCategoriesModel`: each category with the
    /// names of its parents, sorted.
    fn update_categories_model(&mut self) {
        let mut rows: Vec<(Vec<String>, Uuid)> = self
            .categories
            .iter()
            .map(|uuid| (self.category_path(*uuid), *uuid))
            .collect();
        rows.sort_by(|a, b| {
            crate::dialogs::natural_cmp(
                &a.0.join("/").to_lowercase(),
                &b.0.join("/").to_lowercase(),
            )
        });
        self.categories_model.replace_all(
            rows.into_iter()
                .map(|(names, uuid)| ui::LibraryElementCategoryData {
                    uuid: uuid.to_string().into(),
                    names: vec_model(names.into_iter().map(Into::into).collect()),
                    delete: false,
                })
                .collect(),
        );
    }

    /// The names from the root category to `uuid`.
    fn category_path(&self, uuid: Uuid) -> Vec<String> {
        let mut names = Vec::new();
        let mut current = Some(uuid);
        let mut visited = BTreeSet::new();
        while let Some(u) = current {
            if !visited.insert(u) {
                break;
            }
            let dir = self.db.latest(self.category_kind, u).ok().flatten();
            let name = dir
                .as_ref()
                .and_then(|d| {
                    self.db
                        .translations(self.category_kind, d, &self.locales)
                        .ok()
                        .flatten()
                })
                .map(|t| t.name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    format!("{} ({u})", tr!("librepcb::editor::LibraryTab", "Unknown"))
                });
            names.push(name);
            current = dir
                .as_ref()
                .and_then(|d| {
                    self.db
                        .category_metadata(self.category_kind, d)
                        .ok()
                        .flatten()
                })
                .and_then(|c| c.parent);
        }
        names.reverse();
        names
    }

    /// The flattened category tree of the chooser dialog (upstream
    /// `CategoryTreeModel`).
    fn refresh_categories_tree(&mut self) {
        fn add(nodes: &[CategoryTreeNode], level: i32, items: &mut Vec<ui::TreeViewItemData>) {
            for n in nodes {
                items.push(ui::TreeViewItemData {
                    level,
                    text: n.name.as_str().into(),
                    user_data: n.uuid.to_string().into(),
                    has_children: !n.children.is_empty(),
                    expanded: true,
                    ..Default::default()
                });
                add(&n.children, level + 1, items);
            }
        }
        let tree = self
            .db
            .category_tree(self.category_kind, &self.locales)
            .unwrap_or_default();
        let mut items = Vec::new();
        add(&tree, 0, &mut items);
        self.categories_tree.replace_all(items);
    }
}
