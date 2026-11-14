//! Port of libs/librepcb/core/library/org/organizationpcbdesignrules.{h,cpp}.
//!
//! Differences to upstream: the URL is stored verbatim as string instead of
//! a `QUrl` (see COMPAT.md).

use super::board_design_rule_check_settings::BoardDesignRuleCheckSettings;
use crate::geometry::property;
use crate::serialization::{
    self, DeserializeObject, HasUuid, List, LocalizedDescriptionMap, LocalizedNameMap, SExpression,
    SerializeObject,
};
use crate::types::{ElementName, Uuid};

/// PCB design rules (DRC settings) offered by an organization, e.g. the
/// capabilities of a PCB manufacturer.
#[derive(Debug, Clone, PartialEq)]
pub struct OrganizationPcbDesignRules {
    uuid: Uuid,
    names: LocalizedNameMap,
    descriptions: LocalizedDescriptionMap,
    url: String,
    drc_settings: BoardDesignRuleCheckSettings,
}

impl OrganizationPcbDesignRules {
    /// Creates design rules (the sources of `settings` are removed since
    /// they are not supported in this context).
    pub fn new(
        uuid: Uuid,
        name: ElementName,
        description: impl Into<String>,
        url: impl Into<String>,
        settings: BoardDesignRuleCheckSettings,
    ) -> Self {
        let mut rules = Self {
            uuid,
            names: LocalizedNameMap::new(name),
            descriptions: LocalizedDescriptionMap::new(description.into()),
            url: url.into(),
            drc_settings: settings,
        };
        rules.drc_settings.set_sources(Vec::new());
        rules
    }

    property!(
        /// Returns the UUID.
        copy uuid: Uuid, set_uuid
    );
    property!(
        /// Returns the localized names.
        ref names: LocalizedNameMap, set_names
    );
    property!(
        /// Returns the localized descriptions.
        ref descriptions: LocalizedDescriptionMap, set_descriptions
    );
    property!(
        /// Returns the URL (e.g. to the manufacturer's capabilities page).
        ref url: String, set_url
    );

    /// Returns the DRC settings. With `clean_options`, the options with the
    /// prefix `org_` (which are intended only for this class) are removed.
    pub fn drc_settings(&self, clean_options: bool) -> BoardDesignRuleCheckSettings {
        let mut settings = self.drc_settings.clone();
        if clean_options {
            let options = settings
                .options()
                .iter()
                .filter(|(key, _)| !key.starts_with("org_"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            settings.set_options(options);
        }
        settings
    }

    /// Sets the DRC settings (without their sources, which are not
    /// supported in this context).
    pub fn set_drc_settings(&mut self, settings: BoardDesignRuleCheckSettings) {
        self.drc_settings = settings;
        self.drc_settings.set_sources(Vec::new());
    }
}

impl HasUuid for OrganizationPcbDesignRules {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl SerializeObject for OrganizationPcbDesignRules {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        self.names.serialize(root);
        root.ensure_line_break();
        self.descriptions.serialize(root);
        root.ensure_line_break();
        root.append_child("url", &self.url);
        root.ensure_line_break();
        self.drc_settings.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for OrganizationPcbDesignRules {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let mut drc_settings = BoardDesignRuleCheckSettings::deserialize(node)?;
        drc_settings.set_sources(Vec::new()); // Not supported in this context.
        Ok(Self {
            uuid: node.child_value("@0")?,
            names: LocalizedNameMap::deserialize(node)?,
            descriptions: LocalizedDescriptionMap::deserialize(node)?,
            url: node.child_value("url/@0")?,
            drc_settings,
        })
    }
}
