//! STEP assembly evidence extending libs/librepcb/core/3d/stepexport.cpp.
//! Uses the same fitted-part and selected-model rules as physical geometry.
//! Availability is not evidence of verified model alignment or mechanical fit.

use super::{Board, ResolvedDevice};
use crate::project::{AssemblyVariantId, ComponentInstanceId, Project, Result};
use crate::types::Uuid;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRepresentation {
    Represented,
    MissingModel,
    NotFitted,
    BoardOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAlignmentStatus {
    Unverified,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssemblyModelEntry {
    pub component: ComponentInstanceId,
    pub designator: String,
    pub package: Uuid,
    pub model: Option<Uuid>,
    pub representation: ModelRepresentation,
    pub alignment: ModelAlignmentStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepAssemblyCoverage {
    /// None explicitly means PCB-only export, not a complete assembly.
    pub assembly_variant: Option<AssemblyVariantId>,
    pub represented_count: usize,
    pub missing_model_count: usize,
    pub excluded_count: usize,
    /// Model coverage only; never a claim of mechanical verification.
    pub model_coverage_complete: bool,
    pub alignment: ModelAlignmentStatus,
    pub components: Vec<AssemblyModelEntry>,
}

impl StepAssemblyCoverage {
    pub fn capture(
        project: &Project,
        board: &Board,
        variant: Option<AssemblyVariantId>,
    ) -> Result<Self> {
        let mut components = Vec::new();
        for device in board.devices().values() {
            let resolved = ResolvedDevice::resolve(device, project.library())?;
            let component = project
                .circuit()
                .component_instance(device.component())
                .ok_or(crate::project::Error::InexistentComponent(
                    device.component().0,
                ))?;
            let representation = if let Some(variant) = variant {
                if device.parts(component, Some(variant)).is_empty() {
                    ModelRepresentation::NotFitted
                } else if device
                    .lib_model()
                    .is_some_and(|id| resolved.package.models().by_uuid(&id).is_some())
                {
                    ModelRepresentation::Represented
                } else {
                    ModelRepresentation::MissingModel
                }
            } else {
                ModelRepresentation::BoardOnly
            };
            components.push(AssemblyModelEntry {
                component: device.component(),
                designator: component.name().to_string(),
                package: resolved.device.package_uuid(),
                model: device.lib_model(),
                representation,
                alignment: if representation == ModelRepresentation::Represented {
                    ModelAlignmentStatus::Unverified
                } else {
                    ModelAlignmentStatus::NotApplicable
                },
            });
        }
        components.sort_by(|a, b| a.designator.cmp(&b.designator));
        let represented_count = components
            .iter()
            .filter(|e| e.representation == ModelRepresentation::Represented)
            .count();
        let missing_model_count = components
            .iter()
            .filter(|e| e.representation == ModelRepresentation::MissingModel)
            .count();
        Ok(Self {
            assembly_variant: variant,
            represented_count,
            missing_model_count,
            excluded_count: components.len() - represented_count - missing_model_count,
            model_coverage_complete: variant.is_some() && missing_model_count == 0,
            alignment: if represented_count > 0 {
                ModelAlignmentStatus::Unverified
            } else {
                ModelAlignmentStatus::NotApplicable
            },
            components,
        })
    }
}

static_assertions::assert_impl_all!(StepAssemblyCoverage: Send, Sync);
