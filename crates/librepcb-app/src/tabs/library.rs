//! The library tab (library overview).
//!
//! Port of libs/librepcb/editor/library/lib/librarytab.{h,cpp} and
//! libs/librepcb/editor/library/lib/librarydependenciesmodel.{h,cpp}:
//!
//! - **Metadata:** icon, name, description, keywords, author, version,
//!   deprecated, URL, dependencies (the other workspace libraries) and
//!   manufacturer, validated while typing and applied as one undo step
//!   ("apply", upstream `CmdLibraryEdit`) to the [`OpenLibrary`].
//! - **Content:** the category tree (component and package categories,
//!   uncategorized elements, organizations) and the elements of the
//!   selected category from the workspace library database, filtered by
//!   the "find" term; elements are opened, duplicated and removed from
//!   here (the element tabs, see the application).
//! - **Checks:** the library checks with approvals and the automatic fixes
//!   of upstream (title case name, missing author).
//! - **Wizard mode** after creating a library: metadata page, then the
//!   import page.
//!
//! Differences to upstream: moving/copying elements into other libraries
//! is not ported yet; the checks run synchronously after each change.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use librepcb_app_ui as ui;
use librepcb_core::fileio::FilePath;
use librepcb_core::library::LibraryCheckMessage;
use librepcb_core::library::{
    LibraryBaseElement, LibraryBaseElementCheckMessage, title_case_fixed_name,
};
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{ElementName, SimpleString, Uuid, Version};
use librepcb_core::workspace::ElementKind;
use librepcb_i18n::tr;

use super::{TabId, TabRequest, TabUpdate, feature};
use crate::mcp::SharedWorkspace;
use crate::models::UiModel;
use crate::open_library::OpenLibrary;
use crate::rule_check::{CheckMessage, RowAction, RuleCheckMessages};
use crate::validation;

/// An item of the content tree (upstream `LibraryTab::TreeItem`).
#[derive(Debug, Clone)]
struct TreeItem {
    kind: ui::LibraryTreeViewItemType,
    path: Option<FilePath>,
    name: String,
    summary: String,
    deprecated: bool,
    external: bool,
    user_data: String,
    children: Vec<usize>,
}

/// The content tree of a library.
#[derive(Debug, Default)]
struct Tree {
    items: Vec<TreeItem>,
    by_user_data: HashMap<String, usize>,
}

impl Tree {
    fn add(&mut self, item: TreeItem) -> usize {
        let index = self.items.len();
        self.by_user_data.insert(item.user_data.clone(), index);
        self.items.push(item);
        index
    }

    fn root(&mut self, kind: ui::LibraryTreeViewItemType) -> usize {
        self.add(TreeItem {
            kind,
            path: None,
            name: String::new(),
            summary: String::new(),
            deprecated: false,
            external: false,
            user_data: Uuid::new_random().to_string(),
            children: Vec::new(),
        })
    }

    fn sort(&mut self, children: &mut [usize]) {
        let items = &self.items;
        children.sort_by(|a, b| {
            let (a, b) = (&items[*a], &items[*b]);
            (a.kind as i32).cmp(&(b.kind as i32)).then_with(|| {
                crate::dialogs::natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase())
            })
        });
    }

    fn sort_recursive(&mut self, index: usize) {
        let mut children = std::mem::take(&mut self.items[index].children);
        self.sort(&mut children);
        for child in &children {
            self.sort_recursive(*child);
        }
        self.items[index].children = children;
    }

    fn children_recursive(&self, index: usize, out: &mut Vec<usize>) {
        for child in &self.items[index].children {
            if !out.contains(child) {
                out.push(*child);
                self.children_recursive(*child, out);
            }
        }
    }
}

/// The library tab (upstream `LibraryTab`).
pub struct LibraryTab {
    id: TabId,
    library: Rc<OpenLibrary>,
    workspace: SharedWorkspace,
    library_index: i32,
    wizard_mode: bool,
    page_index: i32,
    // Metadata (as edited in the UI).
    name: String,
    name_error: String,
    name_parsed: ElementName,
    description: String,
    keywords: String,
    author: String,
    version: String,
    version_error: String,
    version_parsed: Version,
    deprecated: bool,
    url: String,
    url_error: String,
    manufacturer: String,
    dependencies: BTreeSet<Uuid>,
    dependencies_model: Rc<UiModel<ui::LibraryDependency>>,
    // Content.
    tree: Tree,
    categories: Rc<UiModel<ui::LibraryTreeViewItemData>>,
    categories_index: i32,
    categories_content_y: f32,
    elements: Vec<ui::LibraryTreeViewItemData>,
    filtered_elements: Rc<UiModel<ui::LibraryTreeViewItemData>>,
    filtered_elements_index: i32,
    filtered_elements_content_y: f32,
    filter: String,
    // Checks.
    checks: Rc<RefCell<RuleCheckMessages>>,
    check_error: String,
    check_messages: Vec<LibraryCheckMessage>,
    disappeared_approvals: BTreeSet<SExpression>,
    supported_approvals: BTreeSet<SExpression>,
    checked_state: Option<u64>,
}

impl LibraryTab {
    /// Creates the tab of an opened library; `library_index` is its index
    /// in `Data.libraries`. `on_check_row` is called with message rows
    /// written by the UI, `on_dependency_row` with dependency rows.
    pub fn new(
        library: Rc<OpenLibrary>,
        workspace: SharedWorkspace,
        library_index: i32,
        wizard_mode: bool,
        on_check_row: impl Fn(usize, ui::RuleCheckMessageData) + 'static,
        on_dependency_row: impl Fn(usize, ui::LibraryDependency) + 'static,
    ) -> Self {
        let (name, version) = {
            let lib = library.library();
            (
                lib.metadata().name().clone(),
                lib.metadata().version().clone(),
            )
        };
        let dependencies_model = UiModel::shared(Vec::new());
        dependencies_model.set_handler(on_dependency_row);
        let mut tab = Self {
            id: TabId::new(),
            library,
            workspace,
            library_index,
            wizard_mode,
            page_index: if wizard_mode { 0 } else { 2 },
            name: String::new(),
            name_error: String::new(),
            name_parsed: name,
            description: String::new(),
            keywords: String::new(),
            author: String::new(),
            version: String::new(),
            version_error: String::new(),
            version_parsed: version,
            deprecated: false,
            url: String::new(),
            url_error: String::new(),
            manufacturer: String::new(),
            dependencies: BTreeSet::new(),
            dependencies_model,
            tree: Tree::default(),
            categories: UiModel::shared(Vec::new()),
            categories_index: 0,
            categories_content_y: 0.0,
            elements: Vec::new(),
            filtered_elements: UiModel::shared(Vec::new()),
            filtered_elements_index: -1,
            filtered_elements_content_y: 0.0,
            filter: String::new(),
            checks: Rc::new(RefCell::new(RuleCheckMessages::new(on_check_row))),
            check_error: String::new(),
            check_messages: Vec::new(),
            disappeared_approvals: BTreeSet::new(),
            supported_approvals: BTreeSet::new(),
            checked_state: None,
        };
        tab.refresh_ui_data();
        tab.refresh_elements();
        tab.run_checks_if_modified();
        tab
    }

    /// The unique tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// Sets the tab identifier (to connect models before the tab exists).
    pub fn with_id(mut self, id: TabId) -> Self {
        self.id = id;
        self
    }

    /// The opened library.
    pub fn library(&self) -> &Rc<OpenLibrary> {
        &self.library
    }

    /// Sets the index of the library in `Data.libraries`.
    pub fn set_library_index(&mut self, index: i32) {
        self.library_index = index;
    }

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        let writable = self.library.is_writable();
        ui::TabData {
            r#type: ui::TabType::Library,
            title: self.library.library().metadata().name().to_string().into(),
            features: ui::TabFeatures {
                save: feature(writable),
                undo: feature(self.library.can_undo()),
                redo: feature(self.library.can_redo()),
                find: feature(true),
                ..Default::default()
            },
            read_only: !writable,
            unsaved_changes: self.library.has_unsaved_changes(),
            undo_text: self.library.undo_text().into(),
            redo_text: self.library.redo_text().into(),
            find_term: self.filter.as_str().into(),
            ..Default::default()
        }
    }

    /// Applies the find term (the element filter).
    pub fn set_find_term(&mut self, term: &str) -> bool {
        let term = term.trim();
        if term == self.filter {
            return false;
        }
        self.filter = term.to_owned();
        self.apply_filter();
        true
    }

    /// The `LibraryTabData`.
    pub fn derived_ui_data(&self) -> ui::LibraryTabData {
        let checks = self.checks.borrow();
        let icon = {
            let lib = self.library.library();
            crate::icons::decode_png(lib.icon()).unwrap_or_default()
        };
        ui::LibraryTabData {
            library_index: self.library_index,
            wizard_mode: self.wizard_mode,
            page_index: self.page_index,
            icon,
            name: self.name.as_str().into(),
            name_error: self.name_error.as_str().into(),
            description: self.description.as_str().into(),
            keywords: self.keywords.as_str().into(),
            author: self.author.as_str().into(),
            version: self.version.as_str().into(),
            version_error: self.version_error.as_str().into(),
            deprecated: self.deprecated,
            url: self.url.as_str().into(),
            url_error: self.url_error.as_str().into(),
            dependencies: crate::models::model_rc(&self.dependencies_model),
            manufacturer: self.manufacturer.as_str().into(),
            categories: crate::models::model_rc(&self.categories),
            categories_index: self.categories_index,
            categories_content_y: self.categories_content_y,
            filtered_elements: crate::models::model_rc(&self.filtered_elements),
            filtered_elements_index: self.filtered_elements_index,
            filtered_elements_content_y: self.filtered_elements_content_y,
            checks: ui::RuleCheckData {
                r#type: ui::RuleCheckType::LibraryCheck,
                state: ui::RuleCheckState::UpToDate,
                messages: crate::models::model_rc(checks.model()),
                unapproved: checks.unapproved() as i32,
                errors: checks.errors() as i32,
                execution_error: self.check_error.as_str().into(),
                read_only: !self.library.is_writable(),
            },
            move_or_copy_to_lib: Default::default(),
            move_or_copy_action: ui::LibraryElementsMoveAction::None,
        }
    }

    /// Applies the data written by the UI (upstream `setDerivedUiData()`).
    pub fn set_derived_ui_data(&mut self, data: &ui::LibraryTabData) -> TabUpdate {
        self.categories_content_y = data.categories_content_y;
        self.filtered_elements_content_y = data.filtered_elements_content_y;
        self.name = data.name.to_string();
        if let Some(name) = validation::element_name(&self.name, &mut self.name_error) {
            self.name_parsed = name;
        }
        self.description = data.description.to_string();
        self.keywords = data.keywords.to_string();
        self.author = data.author.to_string();
        self.version = data.version.to_string();
        if let Some(v) = validation::version(&self.version, &mut self.version_error) {
            self.version_parsed = v;
        }
        self.deprecated = data.deprecated;
        self.url = data.url.to_string();
        validation::url(&self.url, &mut self.url_error, true);
        self.manufacturer = data.manufacturer.to_string();
        self.page_index = data.page_index;
        if data.categories_index != self.categories_index {
            self.categories_index = data.categories_index;
            self.set_selected_category();
        }
        self.filtered_elements_index = data.filtered_elements_index;
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        if !data.move_or_copy_to_lib.is_empty() {
            update.status = Some(tr!(
                "librepcb::editor::MainWindow",
                "Not available yet in this version: {0}",
                "move/copy"
            ));
        }
        update
    }

    /// A dependency row was written by the UI (checked/unchecked).
    pub fn dependency_row_written(
        &mut self,
        row: usize,
        data: &ui::LibraryDependency,
    ) -> TabUpdate {
        let Ok(uuid) = data.uuid.parse::<Uuid>() else {
            return TabUpdate::default();
        };
        if uuid == self.library.library().metadata().uuid()
            || data.checked == self.dependencies.contains(&uuid)
        {
            return TabUpdate::default();
        }
        if data.checked {
            self.dependencies.insert(uuid);
        } else {
            self.dependencies.remove(&uuid);
        }
        self.dependencies_model
            .update(row, |r| r.checked = data.checked);
        self.commit_ui_data()
    }

    /// A check message row was written by the UI (approval, autofix).
    pub fn check_row_written(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> TabUpdate {
        let action = self.checks.borrow_mut().row_written(row, data);
        match action {
            RowAction::Approve { approval, approved } => {
                self.library.set_message_approved(&approval, approved);
                self.after_change()
            }
            RowAction::Autofix { index } => self.autofix(index),
            _ => TabUpdate::default(),
        }
    }

    /// The (library) check message at a row of the sorted list.
    fn message_at(&self, index: usize) -> Option<LibraryCheckMessage> {
        let checks = self.checks.borrow();
        let entry = checks.entries().get(index)?;
        self.check_messages
            .iter()
            .find(|m| m.to_message().approval() == entry.message.approval())
            .cloned()
    }

    /// Upstream `autoFixImpl()`: title case name, missing author.
    fn autofix(&mut self, index: usize) -> TabUpdate {
        match self.message_at(index) {
            Some(LibraryCheckMessage::BaseElement(
                LibraryBaseElementCheckMessage::NameNotTitleCase { name },
            )) => {
                self.name_parsed = title_case_fixed_name(&name);
                self.commit_ui_data()
            }
            Some(LibraryCheckMessage::BaseElement(
                LibraryBaseElementCheckMessage::MissingAuthor,
            )) => {
                self.author = self.workspace.lock().settings().user_name.get().clone();
                self.commit_ui_data()
            }
            _ => TabUpdate::default(),
        }
    }

    fn can_autofix(msg: &LibraryCheckMessage) -> bool {
        matches!(
            msg,
            LibraryCheckMessage::BaseElement(
                LibraryBaseElementCheckMessage::NameNotTitleCase { .. }
                    | LibraryBaseElementCheckMessage::MissingAuthor
            )
        )
    }

    /// Updates the UI data from the library (upstream `refreshUiData()`).
    fn refresh_ui_data(&mut self) {
        let lib = self.library.library();
        let m = lib.metadata();
        self.name = m.name().to_string();
        self.name_error.clear();
        self.name_parsed = m.name().clone();
        self.description = m.descriptions().default_value().clone();
        self.keywords = m.keywords().default_value().clone();
        self.author = m.author().clone();
        self.version = m.version().to_string();
        self.version_error.clear();
        self.version_parsed = m.version().clone();
        self.deprecated = m.is_deprecated();
        self.url = lib.url().clone();
        self.url_error.clear();
        self.manufacturer = lib.manufacturer().to_string();
        self.dependencies = lib.dependencies().clone();
        let own = m.uuid();
        drop(lib);
        self.refresh_dependencies(own);
    }

    /// The dependency list: all workspace libraries except this one
    /// (upstream `LibraryDependenciesModel::refresh()`).
    fn refresh_dependencies(&mut self, own: Uuid) {
        let mut items = Vec::new();
        {
            let ws = self.workspace.lock();
            let db = ws.library_db();
            let locales = ws.settings().library_locale_order.get().clone();
            let mut processed = HashSet::new();
            if let Ok(libraries) = db.all(ElementKind::Library, None, None) {
                for (_, dir) in libraries {
                    let Ok(Some(info)) = db.metadata(ElementKind::Library, &dir) else {
                        continue;
                    };
                    if info.uuid == own || !processed.insert(info.uuid) {
                        continue;
                    }
                    let icon = db
                        .library_metadata(&dir)
                        .ok()
                        .flatten()
                        .and_then(|m| crate::icons::decode_png(&m.icon_png))
                        .unwrap_or_default();
                    let name = db
                        .translations(ElementKind::Library, &dir, &locales)
                        .ok()
                        .flatten()
                        .map(|t| t.name)
                        .unwrap_or_default();
                    items.push(ui::LibraryDependency {
                        uuid: info.uuid.to_string().into(),
                        icon,
                        name: name.into(),
                        checked: self.dependencies.contains(&info.uuid),
                    });
                }
            }
        }
        items.sort_by(|a, b| {
            crate::dialogs::natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase())
        });
        self.dependencies_model.replace_all(items);
    }

    /// Applies the edited metadata as one undo step (upstream
    /// `commitUiData()`).
    fn commit_ui_data(&mut self) -> TabUpdate {
        let mut content = self.library.content();
        let m = &mut content.metadata;
        let mut names = m.names().clone();
        names.set_default_value(self.name_parsed.clone());
        m.set_names(names);
        if self.description != *m.descriptions().default_value() {
            let mut d = m.descriptions().clone();
            d.set_default_value(self.description.trim().to_owned());
            m.set_descriptions(d);
        }
        if self.keywords != *m.keywords().default_value() {
            let mut k = m.keywords().clone();
            k.set_default_value(validation::clean_keywords(&self.keywords));
            m.set_keywords(k);
        }
        if self.author != *m.author() {
            m.set_author(self.author.trim().to_owned());
        }
        m.set_version(self.version_parsed.clone());
        m.set_deprecated(self.deprecated);
        if self.url != content.url {
            content.url = self.url.trim().to_owned();
        }
        content.dependencies = self.dependencies.clone();
        if self.manufacturer != content.manufacturer.as_str() {
            content.manufacturer = SimpleString::clean(&self.manufacturer);
        }
        let text = tr!("librepcb::editor::CmdLibraryEdit", "Edit library metadata");
        let mut update = TabUpdate::default();
        if let Err(e) = self.library.edit(&text, content) {
            update
                .requests
                .push(super::editing::error_notification(e.to_string()));
        }
        update.requests.push(TabRequest::LibraryModified);
        let mut after = self.after_change();
        after.requests.extend(update.requests);
        after
    }

    /// Refreshes the data and checks after a modification.
    fn after_change(&mut self) -> TabUpdate {
        self.refresh_ui_data();
        self.run_checks_if_modified();
        TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        }
    }

    /// Runs the library checks if the library changed since the last run.
    pub fn run_checks_if_modified(&mut self) {
        let state = self.library.state_id();
        if self.checked_state == Some(state) {
            return;
        }
        self.checked_state = Some(state);
        self.run_checks();
    }

    fn run_checks(&mut self) {
        match self.library.run_checks() {
            Ok(messages) => {
                self.check_error.clear();
                let approvals = self
                    .library
                    .library()
                    .metadata()
                    .message_approvals()
                    .clone();
                let produced: BTreeSet<SExpression> = messages
                    .iter()
                    .map(|m| m.to_message().approval().clone())
                    .collect();
                self.supported_approvals.extend(produced.iter().cloned());
                self.disappeared_approvals = self
                    .supported_approvals
                    .difference(&produced)
                    .cloned()
                    .collect();
                let entries = messages
                    .iter()
                    .map(|m| CheckMessage {
                        message: m.to_message(),
                        schematic: None,
                        autofix: Self::can_autofix(m),
                    })
                    .collect();
                self.checks.borrow_mut().set_messages(entries, approvals);
                self.check_messages = messages;
            }
            Err(e) => self.check_error = e.to_string(),
        }
    }

    // --- Content ---

    /// Reloads the content tree from the library database (upstream
    /// `refreshLibElements()`), e.g. after a library scan.
    pub fn refresh_elements(&mut self) {
        let workspace = Arc::clone(&self.workspace);
        let ws = workspace.lock();
        let db = ws.library_db();
        let locales = ws.settings().library_locale_order.get().clone();
        let lib_dir = self.library.path().clone();
        let mut tree = Tree::default();
        let uncategorized = tree.root(ui::LibraryTreeViewItemType::Uncategorized);
        let cmp_root = tree.root(ui::LibraryTreeViewItemType::ComponentCategory);
        let pkg_root = tree.root(ui::LibraryTreeViewItemType::PackageCategory);
        let org_root = tree.root(ui::LibraryTreeViewItemType::Organization);
        let mut counts = (0usize, 0usize, 0usize);

        // The categories of this library.
        let lib_categories: HashMap<Uuid, FilePath> =
            [ElementKind::ComponentCategory, ElementKind::PackageCategory]
                .iter()
                .flat_map(|k| db.all_in_library(*k, &lib_dir).unwrap_or_default())
                .map(|(fp, uuid)| (uuid, fp))
                .collect();
        for (kind, t, root) in [
            (
                ElementKind::ComponentCategory,
                ui::LibraryTreeViewItemType::ComponentCategory,
                cmp_root,
            ),
            (
                ElementKind::PackageCategory,
                ui::LibraryTreeViewItemType::PackageCategory,
                pkg_root,
            ),
        ] {
            for uuid in db
                .all_in_library(kind, &lib_dir)
                .unwrap_or_default()
                .values()
            {
                let mut processed = HashSet::new();
                get_or_create_category(
                    &mut tree,
                    db,
                    &locales,
                    &lib_categories,
                    kind,
                    t,
                    *uuid,
                    root,
                    &mut processed,
                );
            }
        }
        for (kind, t, cat_kind, cat_t, root, count) in [
            (
                ElementKind::Symbol,
                ui::LibraryTreeViewItemType::Symbol,
                ElementKind::ComponentCategory,
                ui::LibraryTreeViewItemType::ComponentCategory,
                cmp_root,
                &mut counts.0,
            ),
            (
                ElementKind::Package,
                ui::LibraryTreeViewItemType::Package,
                ElementKind::PackageCategory,
                ui::LibraryTreeViewItemType::PackageCategory,
                pkg_root,
                &mut counts.1,
            ),
        ] {
            load_elements(
                &mut tree,
                db,
                &locales,
                &lib_dir,
                &lib_categories,
                (kind, t),
                (cat_kind, cat_t),
                root,
                uncategorized,
                count,
            );
        }
        for (kind, t) in [
            (
                ElementKind::Component,
                ui::LibraryTreeViewItemType::Component,
            ),
            (ElementKind::Device, ui::LibraryTreeViewItemType::Device),
        ] {
            load_elements(
                &mut tree,
                db,
                &locales,
                &lib_dir,
                &lib_categories,
                (kind, t),
                (
                    ElementKind::ComponentCategory,
                    ui::LibraryTreeViewItemType::ComponentCategory,
                ),
                cmp_root,
                uncategorized,
                &mut counts.0,
            );
        }
        // Organizations.
        let orgs = db
            .all_in_library(ElementKind::Organization, &lib_dir)
            .unwrap_or_default();
        counts.2 = orgs.len();
        for fp in orgs.keys() {
            let item = element_item(
                db,
                &locales,
                ElementKind::Organization,
                ui::LibraryTreeViewItemType::Organization,
                fp,
            );
            let index = tree.add(item);
            tree.items[org_root].children.push(index);
        }
        drop(ws);
        for root in [cmp_root, pkg_root, org_root] {
            tree.sort_recursive(root);
        }

        // Category model.
        let mut rows = vec![ui::LibraryTreeViewItemData {
            r#type: ui::LibraryTreeViewItemType::All,
            elements: (counts.0 + counts.1 + counts.2) as i32,
            ..Default::default()
        }];
        let root_row = |tree: &Tree, root: usize, count: usize| ui::LibraryTreeViewItemData {
            r#type: tree.items[root].kind,
            name: tree.items[root].name.as_str().into(),
            elements: count as i32,
            user_data: tree.items[root].user_data.as_str().into(),
            ..Default::default()
        };
        if !tree.items[uncategorized].children.is_empty() {
            let count = tree.items[uncategorized].children.len();
            rows.push(root_row(&tree, uncategorized, count));
            add_categories(
                &tree,
                uncategorized,
                ui::LibraryTreeViewItemType::Uncategorized,
                1,
                &mut rows,
            );
        }
        rows.push(root_row(&tree, cmp_root, counts.0));
        add_categories(
            &tree,
            cmp_root,
            ui::LibraryTreeViewItemType::ComponentCategory,
            1,
            &mut rows,
        );
        rows.push(root_row(&tree, pkg_root, counts.1));
        add_categories(
            &tree,
            pkg_root,
            ui::LibraryTreeViewItemType::PackageCategory,
            1,
            &mut rows,
        );
        if !tree.items[org_root].children.is_empty() {
            rows.push(root_row(&tree, org_root, counts.2));
        }
        self.tree = tree;
        self.categories.replace_all(rows);
        if self.categories_index as usize >= self.categories.len() {
            self.categories_index = 0;
        }
        self.set_selected_category();
    }

    /// Shows the elements of the selected category (upstream
    /// `setSelectedCategory()`).
    fn set_selected_category(&mut self) {
        let data = usize::try_from(self.categories_index)
            .ok()
            .and_then(|i| self.categories.get(i));
        let is_root = data.as_ref().is_none_or(|d| d.level == 0);
        let item = data
            .as_ref()
            .and_then(|d| self.tree.by_user_data.get(d.user_data.as_str()).copied());
        let mut items: Vec<usize> = Vec::new();
        if is_root {
            match item {
                Some(i) => self.tree.children_recursive(i, &mut items),
                None => items = (0..self.tree.items.len()).collect(),
            }
            self.tree.sort(&mut items);
        } else if let Some(i) = item {
            items = self.tree.items[i].children.clone();
        }
        let element_types = [
            ui::LibraryTreeViewItemType::Symbol,
            ui::LibraryTreeViewItemType::Package,
            ui::LibraryTreeViewItemType::Component,
            ui::LibraryTreeViewItemType::Device,
            ui::LibraryTreeViewItemType::Organization,
        ];
        let mut rows = Vec::new();
        let mut current_type = None;
        let mut seen = HashSet::new();
        for i in items {
            let item = &self.tree.items[i];
            if !item.children.is_empty()
                || item.name.is_empty()
                || !element_types.contains(&item.kind)
                || !seen.insert(item.user_data.clone())
            {
                continue;
            }
            if current_type != Some(item.kind) {
                rows.push(ui::LibraryTreeViewItemData {
                    r#type: item.kind,
                    ..Default::default()
                });
                current_type = Some(item.kind);
            }
            rows.push(ui::LibraryTreeViewItemData {
                r#type: item.kind,
                level: 1,
                name: item.name.as_str().into(),
                summary: item.summary.as_str().into(),
                deprecated: item.deprecated,
                user_data: item.user_data.as_str().into(),
                ..Default::default()
            });
        }
        self.elements = rows;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let filter = self.filter.to_lowercase();
        let rows: Vec<_> = self
            .elements
            .iter()
            .filter(|e| {
                e.level == 0 || filter.is_empty() || e.name.to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        self.filtered_elements.replace_all(rows);
    }

    /// The selected category or element item (the element if one is
    /// selected, upstream `trigger()`).
    fn selected_item(&self, element: bool) -> Option<&TreeItem> {
        let data = if element {
            usize::try_from(self.filtered_elements_index)
                .ok()
                .and_then(|i| self.filtered_elements.get(i))
        } else {
            usize::try_from(self.categories_index)
                .ok()
                .and_then(|i| self.categories.get(i))
        }?;
        let index = self.tree.by_user_data.get(data.user_data.as_str())?;
        let item = &self.tree.items[*index];
        item.path.is_some().then_some(item)
    }

    /// The categories and elements of the tree (for tests).
    pub fn categories_model(&self) -> &Rc<UiModel<ui::LibraryTreeViewItemData>> {
        &self.categories
    }

    /// The shown elements (for tests).
    pub fn elements_model(&self) -> &Rc<UiModel<ui::LibraryTreeViewItemData>> {
        &self.filtered_elements
    }

    /// The check messages.
    pub fn checks(&self) -> std::cell::Ref<'_, RuleCheckMessages> {
        self.checks.borrow()
    }

    // --- Actions ---

    /// Handles a tab action (upstream `LibraryTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        use ui::TabAction as A;
        let item = self
            .selected_item(self.filtered_elements_index >= 0)
            .map(|i| (i.kind, i.path.clone(), i.name.clone()));
        match action {
            A::Back => {
                if self.wizard_mode && self.page_index > 0 {
                    self.page_index -= 1;
                }
                TabUpdate {
                    data_changed: true,
                    ..TabUpdate::default()
                }
            }
            A::Next => {
                let mut update = self.commit_ui_data();
                if self.wizard_mode && self.page_index == 0 {
                    match self.save() {
                        Ok(()) => self.page_index += 1,
                        Err(e) => update.requests.push(e),
                    }
                    update.requests.push(TabRequest::LibraryModified);
                } else if self.wizard_mode && self.page_index == 1 {
                    self.wizard_mode = false;
                    self.page_index += 1;
                }
                update
            }
            A::Apply => self.commit_ui_data(),
            A::Save => {
                let mut update = self.commit_ui_data();
                if let Err(e) = self.save() {
                    update.requests.push(e);
                }
                update.requests.push(TabRequest::LibraryModified);
                update.requests.push(TabRequest::RescanLibraries);
                update
            }
            A::Undo | A::Redo => {
                let _ = self.commit_ui_data();
                if action == A::Undo {
                    self.library.undo();
                } else {
                    self.library.redo();
                }
                let mut update = self.after_change();
                update.requests.push(TabRequest::LibraryModified);
                update
            }
            A::Abort => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.apply_filter();
                } else if self.categories_index != 0 {
                    self.categories_index = 0;
                    self.categories_content_y = 0.0;
                    self.filtered_elements_index = 0;
                    self.filtered_elements_content_y = 0.0;
                    self.set_selected_category();
                }
                TabUpdate {
                    data_changed: true,
                    ..TabUpdate::default()
                }
            }
            A::EditProperties => match item {
                Some((kind, Some(path), _)) => TabUpdate {
                    requests: vec![TabRequest::OpenLibraryElement {
                        library: self.library.path().clone(),
                        kind,
                        path,
                        duplicate: false,
                    }],
                    ..TabUpdate::default()
                },
                _ => TabUpdate::default(),
            },
            A::LibraryCategoriesDuplicate | A::LibraryElementsDuplicate => {
                let item = self
                    .selected_item(action == A::LibraryElementsDuplicate)
                    .map(|i| (i.kind, i.path.clone()));
                match item {
                    Some((kind, Some(path))) => TabUpdate {
                        requests: vec![TabRequest::OpenLibraryElement {
                            library: self.library.path().clone(),
                            kind,
                            path,
                            duplicate: true,
                        }],
                        ..TabUpdate::default()
                    },
                    _ => TabUpdate::default(),
                }
            }
            A::LibraryCategoriesRemove | A::LibraryElementsRemove => {
                let item = self
                    .selected_item(action == A::LibraryElementsRemove)
                    .map(|i| (i.path.clone(), i.name.clone()));
                match item {
                    Some((Some(path), name)) => TabUpdate {
                        requests: vec![TabRequest::RemoveLibraryElements(vec![(path, name)])],
                        ..TabUpdate::default()
                    },
                    _ => TabUpdate::default(),
                }
            }
            A::LibraryChooseIcon => TabUpdate {
                requests: vec![TabRequest::ChooseLibraryIcon],
                ..TabUpdate::default()
            },
            A::FindRefreshSuggestions => TabUpdate::default(),
            _ => TabUpdate::default(),
        }
    }

    /// Sets a new icon (PNG file content) chosen by the user.
    pub fn set_icon(&mut self, png: Vec<u8>) -> TabUpdate {
        let mut content = self.library.content();
        content.icon = png;
        let text = tr!("librepcb::editor::CmdLibraryEdit", "Edit library metadata");
        if let Err(e) = self.library.edit(&text, content) {
            log::error!("Failed to set the library icon: {e}");
        }
        let mut update = self.after_change();
        update.requests.push(TabRequest::LibraryModified);
        update
    }

    /// Saves the library (upstream: removes obsolete approvals first).
    fn save(&mut self) -> Result<(), TabRequest> {
        self.run_checks();
        self.library
            .save(&self.disappeared_approvals)
            .map_err(|e| super::editing::error_notification(e.to_string()))?;
        self.checked_state = None;
        self.run_checks_if_modified();
        Ok(())
    }

    /// Whether the tab has unsaved changes of its library.
    pub fn has_unsaved_changes(&self) -> bool {
        self.library.has_unsaved_changes()
    }

    /// Called when the library was modified elsewhere (another tab of it).
    pub fn library_modified(&mut self) -> TabUpdate {
        self.refresh_ui_data();
        self.run_checks_if_modified();
        TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn get_or_create_category(
    tree: &mut Tree,
    db: &librepcb_core::workspace::LibraryDb,
    locales: &[String],
    lib_categories: &HashMap<Uuid, FilePath>,
    kind: ElementKind,
    t: ui::LibraryTreeViewItemType,
    uuid: Uuid,
    root: usize,
    processed: &mut HashSet<Uuid>,
) -> Option<usize> {
    let key = uuid.to_string();
    if let Some(i) = tree.by_user_data.get(&key) {
        return Some(*i);
    }
    // Catch endless loops in category parent/child relations.
    if !processed.insert(uuid) {
        return None;
    }
    let (fp, external) = match lib_categories.get(&uuid) {
        Some(fp) => (Some(fp.clone()), false),
        None => (db.latest(kind, uuid).ok().flatten(), true),
    };
    let name = fp
        .as_ref()
        .and_then(|fp| db.translations(kind, fp, locales).ok().flatten())
        .map(|t| t.name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{} ({uuid})",
                tr!("librepcb::editor::LibraryTab", "Unknown")
            )
        });
    let deprecated = fp
        .as_ref()
        .and_then(|fp| db.metadata(kind, fp).ok().flatten())
        .is_some_and(|m| m.deprecated);
    let parent_uuid = fp
        .as_ref()
        .and_then(|fp| db.category_metadata(kind, fp).ok().flatten())
        .and_then(|c| c.parent);
    let mut summary = String::new();
    let mut parent = root;
    if let Some(p) = parent_uuid {
        match get_or_create_category(
            tree,
            db,
            locales,
            lib_categories,
            kind,
            t,
            p,
            root,
            processed,
        ) {
            Some(p) => parent = p,
            None => {
                summary = tr!("librepcb::editor::LibraryTab", "Invalid Parent").to_uppercase();
            }
        }
    }
    let index = tree.add(TreeItem {
        kind: t,
        path: if external { None } else { fp },
        name,
        summary,
        deprecated,
        external,
        user_data: key,
        children: Vec::new(),
    });
    tree.items[parent].children.push(index);
    Some(index)
}

fn element_item(
    db: &librepcb_core::workspace::LibraryDb,
    locales: &[String],
    kind: ElementKind,
    t: ui::LibraryTreeViewItemType,
    fp: &FilePath,
) -> TreeItem {
    let texts = db
        .translations(kind, fp, locales)
        .ok()
        .flatten()
        .unwrap_or_default();
    let summary: String = texts
        .description
        .split('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    TreeItem {
        kind: t,
        path: Some(fp.clone()),
        name: texts.name,
        summary,
        deprecated: db
            .metadata(kind, fp)
            .ok()
            .flatten()
            .is_some_and(|m| m.deprecated),
        external: false,
        user_data: fp.as_str().to_owned(),
        children: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn load_elements(
    tree: &mut Tree,
    db: &librepcb_core::workspace::LibraryDb,
    locales: &[String],
    lib_dir: &FilePath,
    lib_categories: &HashMap<Uuid, FilePath>,
    (kind, t): (ElementKind, ui::LibraryTreeViewItemType),
    (cat_kind, cat_t): (ElementKind, ui::LibraryTreeViewItemType),
    root: usize,
    uncategorized: usize,
    count: &mut usize,
) {
    let elements = db.all_in_library(kind, lib_dir).unwrap_or_default();
    *count += elements.len();
    for fp in elements.keys() {
        let index = tree.add(element_item(db, locales, kind, t, fp));
        let mut added = false;
        for cat in db.categories_of(kind, fp).unwrap_or_default() {
            let mut processed = HashSet::new();
            if let Some(c) = get_or_create_category(
                tree,
                db,
                locales,
                lib_categories,
                cat_kind,
                cat_t,
                cat,
                root,
                &mut processed,
            ) {
                tree.items[c].children.push(index);
                added = true;
            }
        }
        if !added {
            tree.items[uncategorized].children.push(index);
        }
    }
}

fn add_categories(
    tree: &Tree,
    item: usize,
    t: ui::LibraryTreeViewItemType,
    level: i32,
    rows: &mut Vec<ui::LibraryTreeViewItemData>,
) {
    for child in &tree.items[item].children {
        let c = &tree.items[*child];
        if c.kind == t {
            let count = c
                .children
                .iter()
                .filter(|i| tree.items[**i].kind != t)
                .count();
            rows.push(ui::LibraryTreeViewItemData {
                r#type: t,
                level,
                name: c.name.as_str().into(),
                summary: c.summary.as_str().into(),
                deprecated: c.deprecated,
                elements: count as i32,
                is_external: c.external,
                user_data: c.user_data.as_str().into(),
            });
        }
        add_categories(tree, *child, t, level + 1, rows);
    }
}
