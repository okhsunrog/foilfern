//! Electrical import provenance, separate from native LibrePCB signal roles.
//!
//! Native roles cannot represent unknown types or all foreign pin semantics.
//! Keep the original types in an optional component-directory sidecar, copied
//! with the normal library element transaction. Native files and ERC approvals
//! are unchanged. Missing or corrupt import provenance must never imply proof
//! of electrical correctness. No datasheet requirements are inferred here.

use std::collections::{BTreeMap, BTreeSet};

use crate::fileio::FileSystem;
use serde::{Deserialize, Serialize};

use super::cmp::Component;
use super::{LibraryBaseElement, LibraryElement};
use crate::types::Uuid;

/// Optional, versioned import evidence retained with the library component.
pub const FILE: &str = "foilfern-electrical.json";

/// Errors reading or writing electrical import evidence.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Sidecar I/O failed.
    #[error(transparent)]
    File(#[from] crate::fileio::Error),
    /// Invalid JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Unsupported schema or evidence for another component.
    #[error("Invalid electrical import metadata: {0}")]
    Invalid(&'static str),
}

/// Fidelity of the source type's native representation, not ERC coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleMapping {
    /// Same role semantics.
    Exact,
    /// A native role preserves only some source semantics.
    Approximate,
    /// The source explicitly supplies no electrical type.
    Unspecified,
    /// No corresponding native role exists.
    Unsupported,
}

/// One imported symbol pin and its corresponding native component signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedPin {
    /// Native component signal UUID.
    pub signal: Uuid,
    /// Native gate UUID, scoped by the component's symbol variant.
    pub gate: Uuid,
    /// Library symbol pin UUID.
    pub pin: Uuid,
    /// Source pin number.
    pub number: String,
    /// Source pin name, before native naming cleanup.
    pub name: String,
    /// Source format's original electrical type token.
    pub source_type: String,
    /// Native role written by the importer, used to detect subsequent edits.
    pub imported_role: String,
    /// Fidelity of that role mapping.
    pub mapping: RoleMapping,
}

/// Evidence of conversion; it never asserts datasheet verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricalMetadata {
    /// Sidecar schema version (currently 1).
    pub version: u32,
    /// Owning native library component UUID.
    pub component: Uuid,
    /// Source format, e.g. kicad or easyeda_std.
    pub source_format: String,
    /// Supplier snapshot hash, when the acquisition flow supplies one.
    pub source_sha256: Option<String>,
    /// Source-to-native pin identities and electrical types.
    pub pins: Vec<ImportedPin>,
}

impl ElectricalMetadata {
    /// Reads optional evidence without changing any native data.
    pub fn read(component: &Component) -> Result<Option<Self>, Error> {
        if !component.directory().file_exists(FILE) {
            return Ok(None);
        }
        let value: Self = serde_json::from_slice(&component.directory().read(FILE)?)?;
        value.validate(component)?;
        Ok(Some(value))
    }

    fn validate(&self, component: &Component) -> Result<(), Error> {
        if self.version != 1 || self.component != component.metadata().uuid() {
            return Err(Error::Invalid(
                "schema version or component identity mismatch",
            ));
        }
        if self
            .source_sha256
            .as_ref()
            .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(Error::Invalid("invalid source SHA-256"));
        }
        let mut seen = BTreeSet::new();
        for pin in &self.pins {
            if !seen.insert((pin.gate, pin.pin))
                || pin
                    .imported_role
                    .parse::<crate::types::SignalRole>()
                    .is_err()
            {
                return Err(Error::Invalid("duplicate pin or invalid native role"));
            }
        }
        Ok(())
    }

    /// Stages evidence in the same component transaction as the native file.
    pub fn write(&self, component: &mut Component) -> Result<(), Error> {
        self.validate(component)?;
        component
            .directory_mut()
            .write(FILE, &serde_json::to_vec(self)?)?;
        Ok(())
    }
}

/// Availability and freshness of recorded electrical import evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Authored native roles; no supported importer marker or sidecar.
    Native,
    /// Source evidence is recorded with the component.
    Recorded,
    /// A recognized import has no source evidence.
    Missing,
    /// Recorded evidence could not be read or validated.
    Invalid,
    /// Native roles or pin mappings changed after import.
    Modified,
}

/// Source mandatory-connection intent, independently of native required flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredConnectionEvidence {
    /// Native authoring flags are used, without a source/datasheet assertion.
    NativeFlagsOnly,
    /// Supported source data supplies no mandatory-connection intent.
    NotProvided,
    /// Source evidence is unavailable.
    Unknown,
}

/// Compact evidence summary for library, component and ERC responses.
#[derive(Debug, Clone, Serialize)]
pub struct ElectricalSummary {
    /// native, recorded, missing, invalid, or modified.
    pub status: ReviewStatus,
    /// Whether unknown/lost/stale import semantics need review.
    pub has_gaps: bool,
    /// Recorded source format, if known.
    pub source_format: Option<String>,
    /// Acquired source identity, when recorded.
    pub source_sha256: Option<String>,
    /// Whether imported mandatory-connection intent was supplied (not inferred).
    pub source_required_connections: RequiredConnectionEvidence,
    /// Number of native signals.
    pub signal_count: usize,
    /// Counts of current native roles; passive does not mean unknown.
    pub native_roles: BTreeMap<String, usize>,
    /// Source pins whose electrical types are explicitly unspecified.
    pub unspecified_pins: usize,
    /// Source pins represented with reduced semantics.
    pub approximated_pins: usize,
    /// Source pin types with no corresponding native role.
    pub unsupported_pins: usize,
    /// Native signals without corresponding source evidence.
    pub unrecorded_signals: usize,
    /// Signals edited since the recorded import or no longer mapped to the pin.
    pub modified_signals: BTreeSet<Uuid>,
    /// Provenance read failure, without preventing structural ERC.
    pub error: Option<String>,
}

/// Source records are exposed alongside each signal by callers needing detail.
#[derive(Debug)]
pub struct ElectricalReview {
    /// Assessment of available evidence, not circuit correctness.
    pub summary: ElectricalSummary,
    /// Original records, even when native roles have subsequently been edited.
    pub pins: Vec<ImportedPin>,
}

static_assertions::assert_impl_all!(ElectricalMetadata: Send, Sync);
static_assertions::assert_impl_all!(ElectricalReview: Send, Sync);

/// Reviews native and imported evidence without guessing roles from names.
pub fn review(component: &Component) -> ElectricalReview {
    let generated = component.element_metadata().generated_by();
    let legacy_import =
        generated.starts_with("FoilfernEasyEDA::") || generated.starts_with("KiCadImport::");
    let mut summary = ElectricalSummary {
        status: if legacy_import {
            ReviewStatus::Missing
        } else {
            ReviewStatus::Native
        },
        has_gaps: legacy_import,
        source_format: None,
        source_sha256: None,
        source_required_connections: if legacy_import {
            RequiredConnectionEvidence::Unknown
        } else {
            RequiredConnectionEvidence::NativeFlagsOnly
        },
        signal_count: component.signals().len(),
        native_roles: BTreeMap::new(),
        unspecified_pins: 0,
        approximated_pins: 0,
        unsupported_pins: 0,
        unrecorded_signals: if legacy_import {
            component.signals().len()
        } else {
            0
        },
        modified_signals: BTreeSet::new(),
        error: None,
    };
    for signal in component.signals().iter() {
        *summary
            .native_roles
            .entry(signal.role().to_str().to_owned())
            .or_default() += 1;
    }
    let pins = match ElectricalMetadata::read(component) {
        Ok(Some(metadata)) => {
            summary.status = ReviewStatus::Recorded;
            summary.has_gaps = false;
            summary.source_format = Some(metadata.source_format);
            summary.source_sha256 = metadata.source_sha256;
            summary.source_required_connections = RequiredConnectionEvidence::NotProvided;
            let recorded: BTreeSet<_> = metadata.pins.iter().map(|p| p.signal).collect();
            summary.unrecorded_signals = component
                .signals()
                .iter()
                .filter(|s| !recorded.contains(&s.uuid()))
                .count();
            for pin in &metadata.pins {
                match pin.mapping {
                    RoleMapping::Exact => {}
                    RoleMapping::Approximate => summary.approximated_pins += 1,
                    RoleMapping::Unspecified => summary.unspecified_pins += 1,
                    RoleMapping::Unsupported => summary.unsupported_pins += 1,
                }
                let mapped = component
                    .symbol_variants()
                    .iter()
                    .flat_map(|v| v.symbol_items().iter())
                    .any(|g| {
                        g.uuid() == pin.gate
                            && g.pin_signal_map().iter().any(|p| {
                                p.pin_uuid() == pin.pin && p.signal_uuid() == Some(pin.signal)
                            })
                    });
                if !mapped
                    || !component
                        .signals()
                        .by_uuid(&pin.signal)
                        .is_some_and(|s| s.role().to_str() == pin.imported_role)
                {
                    summary.modified_signals.insert(pin.signal);
                }
            }
            if !summary.modified_signals.is_empty() {
                summary.status = ReviewStatus::Modified;
            }
            summary.has_gaps = summary.unspecified_pins
                + summary.approximated_pins
                + summary.unsupported_pins
                + summary.unrecorded_signals
                > 0
                || !summary.modified_signals.is_empty();
            metadata.pins
        }
        Ok(None) => Vec::new(),
        Err(error) => {
            summary.status = ReviewStatus::Invalid;
            summary.has_gaps = true;
            summary.source_required_connections = RequiredConnectionEvidence::Unknown;
            summary.unrecorded_signals = component.signals().len();
            summary.error = Some(error.to_string());
            Vec::new()
        }
    };
    ElectricalReview { summary, pins }
}
