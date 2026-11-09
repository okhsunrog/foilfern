//! The component and package category editor tabs.
//!
//! Port of libs/librepcb/editor/library/cat/{componentcategorytab,
//! packagecategorytab,categorytreebuilder}.{h,cpp} and of the parent
//! chooser tree (libs/librepcb/editor/workspace/categorytreemodel.{h,cpp}):
//! the metadata, the parent category, the checks, undo/redo and saving (see
//! [`BaseElementCore`]).

use std::collections::BTreeSet;

use librepcb_app_ui as ui;
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cat::{CategoryKind, LibraryCategory};
use librepcb_core::types::Uuid;
use librepcb_core::workspace::{CategoryTreeNode, ElementKind};
use librepcb_i18n::tr;

use super::base_element_core::{BaseElementCore, BaseElementKind};
use super::{TabId, TabUpdate};
use crate::models::vec_model;

/// Upstream `CategoryTreeBuilder::buildTree()` (with the root category):
/// the names from the root to `category`.
pub fn category_path(
    db: &librepcb_core::workspace::LibraryDb,
    kind: ElementKind,
    locales: &[String],
    category: Option<Uuid>,
) -> Vec<String> {
    const CTX: &str = "librepcb::editor";
    let mut names = Vec::new();
    let mut visited = BTreeSet::new();
    let mut current = category;
    while let Some(uuid) = current {
        let Some(dir) = db.latest(kind, uuid).ok().flatten() else {
            names.push(tr!(CTX, "ERROR: {0} not found", &uuid.to_string()[..8]));
            names.reverse();
            return names;
        };
        if !visited.insert(uuid) {
            names.push("ERROR: Endless recursion".to_owned());
            names.reverse();
            return names;
        }
        let name = db
            .translations(kind, &dir, locales)
            .ok()
            .flatten()
            .map(|t| t.name)
            .unwrap_or_default();
        names.push(name);
        current = db
            .category_metadata(kind, &dir)
            .ok()
            .flatten()
            .and_then(|c| c.parent);
    }
    names.push(tr!(CTX, "Root Category"));
    names.reverse();
    names
}

/// A category editor tab (upstream `ComponentCategoryTab`,
/// `PackageCategoryTab`).
pub struct CategoryTab<K: CategoryKind>
where
    LibraryCategory<K>: BaseElementKind<Extra = Option<Uuid>>,
{
    id: TabId,
    core: BaseElementCore<LibraryCategory<K>>,
    library_index: i32,
    parent: Option<Uuid>,
    choose_parent: bool,
    parents: Vec<String>,
    parents_tree: Vec<ui::TreeViewItemData>,
}

impl<K: CategoryKind> CategoryTab<K>
where
    LibraryCategory<K>: BaseElementKind<Extra = Option<Uuid>>,
{
    /// Creates the tab for an element core.
    pub fn new(core: BaseElementCore<LibraryCategory<K>>) -> Self {
        let mut tab = Self {
            id: TabId::new(),
            core,
            library_index: -1,
            parent: None,
            choose_parent: false,
            parents: Vec::new(),
            parents_tree: Vec::new(),
        };
        tab.refresh_parent();
        tab.refresh_tree();
        tab
    }

    /// Sets the tab identifier.
    pub fn with_id(mut self, id: TabId) -> Self {
        self.id = id;
        self
    }

    /// The unique tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The shared element state.
    pub fn core(&self) -> &BaseElementCore<LibraryCategory<K>> {
        &self.core
    }

    /// Sets the index of the library in `Data.libraries`.
    pub fn set_library_index(&mut self, index: i32) {
        self.library_index = index;
    }

    fn kind() -> ElementKind {
        <LibraryCategory<K> as BaseElementKind>::KIND
    }

    fn refresh(&mut self) {
        self.core.refresh();
        self.refresh_parent();
    }

    fn refresh_parent(&mut self) {
        self.parent = self.core.element().parent_uuid();
        self.parents = category_path(&self.core.db, Self::kind(), &self.core.locales, self.parent);
    }

    /// The parent chooser tree: "Root Category" and all categories except
    /// this one and its children (upstream `CategoryTreeModel`).
    fn refresh_tree(&mut self) {
        fn add(
            nodes: &[CategoryTreeNode],
            hidden: Uuid,
            level: i32,
            items: &mut Vec<ui::TreeViewItemData>,
        ) {
            for n in nodes.iter().filter(|n| n.uuid != hidden) {
                items.push(ui::TreeViewItemData {
                    level,
                    text: if n.name.is_empty() {
                        n.uuid.to_string()
                    } else {
                        n.name.clone()
                    }
                    .into(),
                    user_data: n.uuid.to_string().into(),
                    ..Default::default()
                });
                add(&n.children, hidden, level + 1, items);
            }
        }
        let mut items = vec![ui::TreeViewItemData {
            level: 0,
            text: tr!("librepcb::editor::CategoryTreeModel", "Root Category").into(),
            user_data: "null".into(),
            ..Default::default()
        }];
        let tree = self
            .core
            .db
            .category_tree(Self::kind(), &self.core.locales)
            .unwrap_or_default();
        add(&tree, self.core.element().metadata().uuid(), 1, &mut items);
        self.parents_tree = items;
    }

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        self.core.tab_data()
    }

    /// The `CategoryTabData`.
    pub fn derived_ui_data(&self) -> ui::CategoryTabData {
        let c = &self.core;
        ui::CategoryTabData {
            library_index: self.library_index,
            path: c.directory_path().as_str().into(),
            name: c.name.as_str().into(),
            name_error: c.name_error.as_str().into(),
            description: c.description.as_str().into(),
            keywords: c.keywords.as_str().into(),
            author: c.author.as_str().into(),
            version: c.version.as_str().into(),
            version_error: c.version_error.as_str().into(),
            deprecated: c.deprecated,
            parents: vec_model(self.parents.iter().map(|p| p.as_str().into()).collect()),
            parents_tree: vec_model(self.parents_tree.clone()),
            choose_parent: self.choose_parent,
            checks: c.checks_data(),
            new_parent: Default::default(),
        }
    }

    /// Applies data written by the UI (upstream `setDerivedUiData()`).
    pub fn set_derived_ui_data(&mut self, data: &ui::CategoryTabData) -> TabUpdate {
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        self.core.set_metadata_from_ui(
            &data.name,
            &data.description,
            &data.keywords,
            &data.author,
            &data.version,
            data.deprecated,
        );
        self.choose_parent = data.choose_parent;
        if !data.new_parent.is_empty() {
            self.parent = data.new_parent.parse::<Uuid>().ok();
            let parent = self.parent;
            update
                .requests
                .extend(self.core.commit(|extra| *extra = parent));
            self.refresh();
        }
        update
    }

    /// A check message row was written by the UI.
    pub fn check_row_written(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> TabUpdate {
        let update = self.core.check_row_written(row, data);
        self.refresh();
        update
    }

    /// Handles a tab action (upstream `ComponentCategoryTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        let parent = self.parent;
        let update = self.core.trigger_common(action, |core| {
            core.commit(|extra| *extra = parent).into_iter().collect()
        });
        match update {
            Some(update) => {
                self.refresh();
                update
            }
            None => TabUpdate::default(),
        }
    }
}
