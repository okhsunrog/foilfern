//! The organization editor tab.
//!
//! Port of libs/librepcb/editor/library/org/{organizationtab,
//! organizationpcbdesignrulesmodel}.{h,cpp}: logo, metadata, URL,
//! priority, the list of PCB design rules (add, rename, duplicate, remove),
//! the checks, undo/redo and saving (see [`BaseElementCore`]).
//!
//! Differences to upstream: the design rules and the output jobs of an
//! organization can't be edited yet (upstream opens the board setup and
//! output jobs dialogs on a temporary project).

use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_core::library::org::{
    BoardDesignRuleCheckSettings, Organization, OrganizationPcbDesignRules,
};
use librepcb_core::types::{ElementName, Uuid};
use librepcb_i18n::tr;

use super::base_element_core::BaseElementCore;
use super::{TabId, TabRequest, TabUpdate};
use crate::models::{UiModel, model_rc};
use crate::validation;

const CTX: &str = "librepcb::editor::OrganizationPcbDesignRulesModel";

/// What the name of the "PCB Design Rules Name" dialog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignRulesNamePurpose {
    /// New design rules.
    Add,
    /// Rename the design rules at the index.
    Rename(usize),
    /// Copy the design rules at the index.
    Duplicate(usize),
}

/// The organization editor tab (upstream `OrganizationTab`).
pub struct OrganizationTab {
    id: TabId,
    core: BaseElementCore<Organization>,
    library_index: i32,
    logo: Vec<u8>,
    url: String,
    url_error: String,
    priority: i32,
    rules: Rc<UiModel<ui::OrganizationPcbDesignRulesData>>,
    pending: Vec<TabRequest>,
}

impl OrganizationTab {
    /// Creates the tab; `on_rules_row` receives the design rules rows
    /// written by the UI.
    pub fn new(
        core: BaseElementCore<Organization>,
        on_rules_row: impl Fn(usize, ui::OrganizationPcbDesignRulesData) + 'static,
    ) -> Self {
        let rules = UiModel::shared(Vec::new());
        rules.set_handler(on_rules_row);
        let mut tab = Self {
            id: TabId::new(),
            core,
            library_index: -1,
            logo: Vec::new(),
            url: String::new(),
            url_error: String::new(),
            priority: 0,
            rules,
            pending: Vec::new(),
        };
        tab.refresh_content();
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
    pub fn core(&self) -> &BaseElementCore<Organization> {
        &self.core
    }

    /// Sets the index of the library in `Data.libraries`.
    pub fn set_library_index(&mut self, index: i32) {
        self.library_index = index;
    }

    fn refresh(&mut self) {
        self.core.refresh();
        self.refresh_content();
    }

    fn refresh_content(&mut self) {
        let org = self.core.element();
        self.logo = org.logo_png().clone();
        self.url = org.url().clone();
        self.url_error.clear();
        self.priority = org.priority();
        self.rules.replace_all(
            org.pcb_design_rules()
                .iter()
                .map(|r| ui::OrganizationPcbDesignRulesData {
                    name: r.names().default_value().as_str().into(),
                    action: ui::OrganizationPcbDesignRulesAction::None,
                })
                .collect(),
        );
    }

    fn commit(&mut self) -> Vec<TabRequest> {
        let (logo, url, priority) = (self.logo.clone(), self.url.trim().to_owned(), self.priority);
        let requests: Vec<TabRequest> = self
            .core
            .commit(|extra| {
                extra.logo = logo;
                extra.url = url;
                extra.priority = priority;
            })
            .into_iter()
            .collect();
        self.refresh();
        requests
    }

    fn edit_rules(&mut self, f: impl FnOnce(&mut Vec<OrganizationPcbDesignRules>)) -> TabUpdate {
        let result = self.core.edit(
            &tr!(
                "librepcb::editor::CmdOrganizationEdit",
                "Edit Organization Properties"
            ),
            |c| f(&mut c.extra.pcb_design_rules),
        );
        self.core.run_checks_if_modified();
        self.refresh();
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        if let Err(e) = result {
            update.requests.push(super::editing::error_notification(e));
        }
        update
    }

    /// A design rules row was written by the UI (actions).
    pub fn rules_row_written(
        &mut self,
        row: usize,
        data: &ui::OrganizationPcbDesignRulesData,
    ) -> TabUpdate {
        use ui::OrganizationPcbDesignRulesAction as A;
        let Some(rules) = self.core.element().pcb_design_rules().get(row) else {
            return TabUpdate::default();
        };
        let name = rules.names().default_value().to_string();
        match data.action {
            A::Edit => {
                self.refresh();
                TabUpdate {
                    data_changed: true,
                    status: Some(tr!(
                        "librepcb::editor::MainWindow",
                        "Not available yet in this version: {0}",
                        "PCB design rules"
                    )),
                    ..TabUpdate::default()
                }
            }
            A::Rename => {
                self.refresh();
                TabUpdate {
                    data_changed: true,
                    requests: vec![TabRequest::DesignRulesName {
                        purpose: DesignRulesNamePurpose::Rename(row),
                        name,
                    }],
                    ..TabUpdate::default()
                }
            }
            A::Duplicate => {
                self.refresh();
                TabUpdate {
                    data_changed: true,
                    requests: vec![TabRequest::DesignRulesName {
                        purpose: DesignRulesNamePurpose::Duplicate(row),
                        name: tr!(CTX, "Copy of {0}", name),
                    }],
                    ..TabUpdate::default()
                }
            }
            A::Delete => self.edit_rules(|list| {
                if row < list.len() {
                    list.remove(row);
                }
            }),
            A::None => TabUpdate::default(),
        }
    }

    /// The name entered in the "PCB Design Rules Name" dialog.
    pub fn design_rules_named(&mut self, purpose: DesignRulesNamePurpose, name: &str) -> TabUpdate {
        let Ok(name) = ElementName::new(ElementName::clean(name)) else {
            return TabUpdate::default();
        };
        self.edit_rules(|list| match purpose {
            DesignRulesNamePurpose::Add => list.push(OrganizationPcbDesignRules::new(
                Uuid::new_random(),
                name,
                "",
                "",
                BoardDesignRuleCheckSettings::default(),
            )),
            DesignRulesNamePurpose::Rename(i) => {
                if let Some(r) = list.get_mut(i) {
                    let mut names = r.names().clone();
                    names.set_default_value(name);
                    r.set_names(names);
                }
            }
            DesignRulesNamePurpose::Duplicate(i) => {
                if let Some(r) = list.get(i) {
                    let mut copy = r.clone();
                    copy.set_uuid(Uuid::new_random());
                    let mut names = copy.names().clone();
                    names.set_default_value(name);
                    copy.set_names(names);
                    list.push(copy);
                }
            }
        })
    }

    /// Sets a new logo (PNG file content) chosen by the user.
    pub fn set_logo(&mut self, png: Vec<u8>) -> TabUpdate {
        self.logo = png;
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        update.requests.extend(self.commit());
        update
    }

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        self.core.tab_data()
    }

    /// The `OrganizationTabData`.
    pub fn derived_ui_data(&self) -> ui::OrganizationTabData {
        let c = &self.core;
        let org = c.element();
        ui::OrganizationTabData {
            library_index: self.library_index,
            path: c.directory_path().as_str().into(),
            logo: crate::icons::decode_png(&self.logo).unwrap_or_default(),
            name: c.name.as_str().into(),
            name_error: c.name_error.as_str().into(),
            description: c.description.as_str().into(),
            keywords: c.keywords.as_str().into(),
            author: c.author.as_str().into(),
            version: c.version.as_str().into(),
            version_error: c.version_error.as_str().into(),
            deprecated: c.deprecated,
            url: self.url.as_str().into(),
            url_error: self.url_error.as_str().into(),
            priority: self.priority,
            pcb_design_rules: model_rc(&self.rules),
            pcb_output_jobs: org.pcb_output_jobs().len() as i32,
            assembly_output_jobs: org.assembly_output_jobs().len() as i32,
            checks: c.checks_data(),
        }
    }

    /// Applies data written by the UI (upstream `setDerivedUiData()`).
    pub fn set_derived_ui_data(&mut self, data: &ui::OrganizationTabData) -> TabUpdate {
        self.core.set_metadata_from_ui(
            &data.name,
            &data.description,
            &data.keywords,
            &data.author,
            &data.version,
            data.deprecated,
        );
        self.url = data.url.to_string();
        validation::url(&self.url, &mut self.url_error, true);
        self.priority = data.priority;
        TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        }
    }

    /// A check message row was written by the UI.
    pub fn check_row_written(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> TabUpdate {
        let update = self.core.check_row_written(row, data);
        self.refresh();
        update
    }

    /// Handles a tab action (upstream `OrganizationTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        use ui::TabAction as A;
        let (logo, url, priority) = (self.logo.clone(), self.url.trim().to_owned(), self.priority);
        if let Some(update) = self.core.trigger_common(action, |core| {
            core.commit(|extra| {
                extra.logo = logo;
                extra.url = url;
                extra.priority = priority;
            })
            .into_iter()
            .collect()
        }) {
            self.refresh();
            return update;
        }
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        match action {
            A::LibraryChooseIcon => update.requests.push(TabRequest::ChooseLibraryIcon),
            A::OrganizationAddPcbDesignRules => {
                update.requests.push(TabRequest::DesignRulesName {
                    purpose: DesignRulesNamePurpose::Add,
                    name: String::new(),
                });
            }
            A::OrganizationEditPcbOutputJobs | A::OrganizationEditAssemblyOutputJobs => {
                update.status = Some(tr!(
                    "librepcb::editor::MainWindow",
                    "Not available yet in this version: {0}",
                    format!("{action:?}")
                ));
            }
            _ => return TabUpdate::default(),
        }
        update.requests.extend(std::mem::take(&mut self.pending));
        update
    }
}
