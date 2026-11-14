//! Port of libs/librepcb/core/library/org (organizations, e.g. PCB
//! manufacturers).

mod board_design_rule_check_settings;
mod organization;
mod organization_check;
mod organization_pcb_design_rules;

pub use board_design_rule_check_settings::{
    AllowedSlots, BoardDesignRuleCheckSettings, DrcSettingsSource, LengthPair,
};
pub use organization::Organization;
pub use organization_check::run_organization_checks;
pub use organization_pcb_design_rules::OrganizationPcbDesignRules;
