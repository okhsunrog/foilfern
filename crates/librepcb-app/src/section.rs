//! Window sections: a main window is split horizontally into sections, each
//! with its own tab bar.
//!
//! Port of libs/librepcb/editor/windowsection.{h,cpp}. The section's
//! `tabs` model holds the base `TabData` of every tab; the per-kind models
//! (`schematic-tabs`, `board-2d-tabs`, ...) are "proxy views" with the same
//! row count, holding the kind's data for tabs of that kind and default
//! data for the others (upstream `DerivedUiObjectList`). Models of tab
//! kinds which are not ported yet stay empty (indexing them yields default
//! data in Slint).

use std::rc::Rc;

use librepcb_app_ui as ui;

use crate::models::{UiModel, model_rc};
use crate::project::AppProject;
use crate::tabs::{Tab, TabId};

/// The models of a section written by the UI: base tab data and the
/// per-kind tab data.
pub type SectionModels<'a> = (&'a Rc<UiModel<ui::TabData>>, &'a DerivedModels);

/// The per-kind "proxy view" models of a section (one row per tab, default
/// data for tabs of other kinds).
#[derive(Default)]
pub struct DerivedModels {
    /// `schematic-tabs`.
    pub schematic: Rc<UiModel<ui::SchematicTabData>>,
    /// `board-2d-tabs`.
    pub board: Rc<UiModel<ui::Board2dTabData>>,
    /// `create-library-tabs`.
    pub create_library: Rc<UiModel<ui::CreateLibraryTabData>>,
    /// `download-library-tabs`.
    pub download_library: Rc<UiModel<ui::DownloadLibraryTabData>>,
    /// `library-tabs`.
    pub library: Rc<UiModel<ui::LibraryTabData>>,
    /// `component-category-tabs`.
    pub component_category: Rc<UiModel<ui::CategoryTabData>>,
    /// `package-category-tabs`.
    pub package_category: Rc<UiModel<ui::CategoryTabData>>,
    /// `symbol-tabs`.
    pub symbol: Rc<UiModel<ui::SymbolTabData>>,
    /// `package-tabs`.
    pub package: Rc<UiModel<ui::PackageTabData>>,
    /// `component-tabs`.
    pub component: Rc<UiModel<ui::ComponentTabData>>,
    /// `device-tabs`.
    pub device: Rc<UiModel<ui::DeviceTabData>>,
    /// `organization-tabs`.
    pub organization: Rc<UiModel<ui::OrganizationTabData>>,
    /// `project-library-tabs`.
    pub project_library: Rc<UiModel<ui::ProjectLibraryTabData>>,
}

/// Calls `$m!(field)` for every model of [`DerivedModels`].
macro_rules! for_each_model {
    ($m:ident) => {
        $m!(schematic);
        $m!(board);
        $m!(create_library);
        $m!(download_library);
        $m!(library);
        $m!(component_category);
        $m!(package_category);
        $m!(symbol);
        $m!(package);
        $m!(component);
        $m!(device);
        $m!(organization);
        $m!(project_library);
    };
}

impl DerivedModels {
    fn insert(&self, index: usize, tab: &Tab, projects: &[Rc<AppProject>]) {
        macro_rules! ins {
            ($f:ident) => {
                self.$f.insert(index, Default::default());
            };
        }
        for_each_model!(ins);
        self.set(index, tab, projects);
    }

    fn remove(&self, index: usize) {
        macro_rules! rem {
            ($f:ident) => {
                self.$f.remove(index);
            };
        }
        for_each_model!(rem);
    }

    /// Sets the row of the tab's kind.
    fn set(&self, index: usize, tab: &Tab, projects: &[Rc<AppProject>]) {
        match tab {
            Tab::Home(_) => {}
            Tab::Schematic(_) => self.schematic.set(index, tab.schematic_data(projects)),
            Tab::Board2d(_) => self.board.set(index, tab.board_data(projects)),
            Tab::CreateLibrary(t) => self.create_library.set(index, t.derived_ui_data()),
            Tab::DownloadLibrary(t) => self.download_library.set(index, t.derived_ui_data()),
            Tab::Library(t) => self.library.set(index, t.derived_ui_data()),
            Tab::Symbol(t) => self.symbol.set(index, t.derived_ui_data()),
            Tab::Package(t) => self.package.set(index, t.derived_ui_data()),
            Tab::Component(t) => self.component.set(index, t.derived_ui_data()),
            Tab::Device(t) => self.device.set(index, t.derived_ui_data()),
            Tab::ComponentCategory(t) => self.component_category.set(index, t.derived_ui_data()),
            Tab::PackageCategory(t) => self.package_category.set(index, t.derived_ui_data()),
            Tab::Organization(t) => self.organization.set(index, t.derived_ui_data()),
            Tab::ProjectLibrary(t) => self.project_library.set(index, t.derived_ui_data(projects)),
        }
    }
}

/// A window section.
pub struct WindowSection {
    tabs: Vec<Tab>,
    current: i32,
    /// Recently active tabs (last is current), to go back when closing.
    previous: Vec<TabId>,
    highlight: bool,
    tabs_model: Rc<UiModel<ui::TabData>>,
    derived: DerivedModels,
}

impl Default for WindowSection {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowSection {
    /// Creates an empty section.
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            current: -1,
            previous: Vec::new(),
            highlight: false,
            tabs_model: UiModel::shared(Vec::new()),
            derived: DerivedModels::default(),
        }
    }

    /// The `WindowSectionData`.
    pub fn ui_data(&self) -> ui::WindowSectionData {
        let d = &self.derived;
        ui::WindowSectionData {
            tabs: model_rc(&self.tabs_model),
            create_library_tabs: model_rc(&d.create_library),
            download_library_tabs: model_rc(&d.download_library),
            library_tabs: model_rc(&d.library),
            component_category_tabs: model_rc(&d.component_category),
            package_category_tabs: model_rc(&d.package_category),
            symbol_tabs: model_rc(&d.symbol),
            package_tabs: model_rc(&d.package),
            component_tabs: model_rc(&d.component),
            device_tabs: model_rc(&d.device),
            organization_tabs: model_rc(&d.organization),
            project_library_tabs: model_rc(&d.project_library),
            schematic_tabs: model_rc(&d.schematic),
            board_2d_tabs: model_rc(&d.board),
            current_tab_index: self.current,
            highlight: self.highlight,
            ..Default::default()
        }
    }

    /// The models written by the UI (to connect their handlers).
    pub fn models(&self) -> SectionModels<'_> {
        (&self.tabs_model, &self.derived)
    }

    /// The tabs.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// A tab, mutably.
    pub fn tab_mut(&mut self, index: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(index)
    }

    /// The tabs, mutably.
    pub fn tabs_mut(&mut self) -> &mut [Tab] {
        &mut self.tabs
    }

    /// The index of a tab.
    pub fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id() == id)
    }

    /// The current tab index (-1 if none).
    pub fn current_index(&self) -> i32 {
        self.current
    }

    /// The current tab.
    pub fn current_tab_mut(&mut self) -> Option<&mut Tab> {
        usize::try_from(self.current)
            .ok()
            .and_then(|i| self.tabs.get_mut(i))
    }

    /// Whether the first tab is the home tab.
    pub fn has_home_tab(&self) -> bool {
        matches!(self.tabs.first(), Some(Tab::Home(_)))
    }

    /// Adds a tab at `index` (end if `None`), optionally making it current.
    pub fn add_tab(
        &mut self,
        tab: Tab,
        index: Option<usize>,
        switch_to: bool,
        projects: &[Rc<AppProject>],
    ) -> usize {
        let index = index.unwrap_or(self.tabs.len()).min(self.tabs.len());
        let mut current = self.current.max(0);
        if !self.tabs.is_empty() && index as i32 <= current {
            current += 1;
        }
        self.tabs_model.insert(index, tab.ui_data());
        self.derived.insert(index, &tab, projects);
        self.tabs.insert(index, tab);
        self.set_current_tab(if switch_to { index as i32 } else { current });
        index
    }

    /// Removes a tab; returns it and whether it was the current tab.
    pub fn remove_tab(&mut self, index: usize) -> Option<(Tab, bool)> {
        if index >= self.tabs.len() {
            return None;
        }
        let was_current = self.current == index as i32;
        let tab = self.tabs.remove(index);
        self.tabs_model.remove(index);
        self.derived.remove(index);
        self.previous.retain(|id| *id != tab.id());
        let mut current = self.current;
        if was_current {
            if let Some(i) = self
                .previous
                .last()
                .and_then(|id| self.tabs.iter().position(|t| t.id() == *id))
            {
                current = i as i32;
            }
        } else if current > index as i32 {
            current -= 1;
        }
        self.current = -2; // Force the update.
        self.set_current_tab(current.min(self.tabs.len() as i32 - 1));
        Some((tab, was_current))
    }

    /// Makes a tab current; returns whether it changed.
    pub fn set_current_tab(&mut self, index: i32) -> bool {
        if index == self.current {
            return false;
        }
        self.current = index;
        if let Some(tab) = usize::try_from(index).ok().and_then(|i| self.tabs.get(i)) {
            let id = tab.id();
            self.previous.retain(|p| *p != id);
            self.previous.push(id);
        }
        true
    }

    /// Updates the model rows of a tab (after its data changed).
    pub fn refresh_tab(&self, index: usize, projects: &[Rc<AppProject>]) {
        if let Some(tab) = self.tabs.get(index) {
            self.tabs_model.set(index, tab.ui_data());
            self.derived.set(index, tab, projects);
        }
    }

    /// Updates only the per-kind rows of a tab (e.g. the frame counter).
    pub fn refresh_derived(&self, index: usize, projects: &[Rc<AppProject>]) {
        if let Some(tab) = self.tabs.get(index) {
            self.derived.set(index, tab, projects);
        }
    }

    /// Updates the model rows of all tabs.
    pub fn refresh_all(&self, projects: &[Rc<AppProject>]) {
        for i in 0..self.tabs.len() {
            self.refresh_tab(i, projects);
        }
    }

    /// Sets the highlight flag (upstream highlights a section for one
    /// second when a tab is opened in it).
    pub fn set_highlight(&mut self, highlight: bool) {
        self.highlight = highlight;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_tabs() {
        let mut s = WindowSection::new();
        s.add_tab(Tab::Home(TabId::new()), None, false, &[]);
        assert_eq!(s.current_index(), 0);
        let a = TabId::new();
        s.add_tab(Tab::Home(a), None, true, &[]);
        assert_eq!(s.current_index(), 1);
        let b = TabId::new();
        s.add_tab(Tab::Home(b), Some(0), false, &[]);
        // Inserted before the current tab: the current index follows.
        assert_eq!(s.current_index(), 2);
        assert_eq!(s.tab_index(a), Some(2));
        assert_eq!(s.models().0.len(), 3);
        assert_eq!(s.models().1.schematic.len(), 3);
        assert_eq!(s.models().1.library.len(), 3);
        s.set_current_tab(0);
        // Closing the current tab goes back to the previous one.
        let (tab, was_current) = s.remove_tab(0).unwrap();
        assert_eq!(tab.id(), b);
        assert!(was_current);
        assert_eq!(s.current_index(), 1);
        assert_eq!(s.tab_index(a), Some(1));
        assert_eq!(s.models().1.board.len(), 2);
    }
}
