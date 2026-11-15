//! Scope of native ERC and the import evidence available to that check.
//! This report is separate from native rule-check messages and approvals.

use serde::Serialize;

use crate::library::electrical_metadata::{self, ElectricalSummary, ReviewStatus};
use crate::project::{ComponentInstanceId, Project};
use crate::types::Uuid;

/// Import evidence for an instantiated component.
#[derive(Debug, Serialize)]
pub struct ComponentElectricalCoverage {
    /// Native instance identity.
    pub component: ComponentInstanceId,
    /// Human-readable instance name.
    pub designator: String,
    /// Library component owning the evidence.
    pub library_component: Uuid,
    /// Available source metadata and any loss/staleness.
    pub metadata: ElectricalSummary,
}

/// Native ERC success must not be mistaken for a datasheet/electrical proof.
#[derive(Debug, Serialize)]
pub struct ErcCoverage {
    /// Connectivity, required native signals, forced names, gates and wiring.
    pub scope: &'static str,
    /// Native ERC does not currently check pin-role compatibility.
    pub pin_role_compatibility_checked: bool,
    /// Native ERC does not currently validate power drivers.
    pub power_sources_checked: bool,
    /// Required connections come from native flags, not inferred datasheets.
    pub required_connections: &'static str,
    /// No datasheet validation is performed by the native check.
    pub datasheet_verified: bool,
    /// Incomplete, lossy, missing or modified import evidence is present.
    pub has_metadata_gaps: bool,
    /// All instantiated imported components (including fully mapped ones).
    pub import_metadata: Vec<ComponentElectricalCoverage>,
    /// Library data unavailable to the structural check.
    pub unresolved_components: Vec<ComponentInstanceId>,
}

/// Reports evidence without changing the model or adding native findings.
pub fn erc_coverage(project: &Project) -> ErcCoverage {
    let mut import_metadata = Vec::new();
    let mut unresolved_components = Vec::new();
    for (id, component) in project.circuit().component_instances() {
        let Some(library) = project.library().component(&component.lib_component()) else {
            unresolved_components.push(*id);
            continue;
        };
        let summary = electrical_metadata::review(library).summary;
        if summary.status != ReviewStatus::Native {
            import_metadata.push(ComponentElectricalCoverage {
                component: *id,
                designator: component.name().as_str().to_owned(),
                library_component: component.lib_component(),
                metadata: summary,
            });
        }
    }
    ErcCoverage {
        scope: "native_structural_erc",
        pin_role_compatibility_checked: false,
        power_sources_checked: false,
        required_connections: "native_flags_only",
        datasheet_verified: false,
        has_metadata_gaps: !unresolved_components.is_empty()
            || import_metadata.iter().any(|c| c.metadata.has_gaps),
        import_metadata,
        unresolved_components,
    }
}
