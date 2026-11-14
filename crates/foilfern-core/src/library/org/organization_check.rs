//! Port of libs/librepcb/core/library/org/organizationcheck.{h,cpp}.

use super::organization::Organization;
use crate::library::{LibraryBaseElement, LibraryCheckMessage, run_base_element_checks};

/// Runs the organization check (upstream `OrganizationCheck::runChecks()`,
/// which only runs the checks common to all elements).
pub fn run_organization_checks(organization: &Organization, msgs: &mut Vec<LibraryCheckMessage>) {
    run_base_element_checks(organization.metadata(), msgs);
}
