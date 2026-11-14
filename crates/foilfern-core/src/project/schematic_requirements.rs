//! Project-associated schematic requirements and checks (Foilfern extension).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{ComponentSignalRef, NetSignalId, Project, SymbolRef, schematic::SchematicSymbol};
use crate::fileio::FileSystem;
use crate::types::{Angle, Point};

/// Transactional project asset. All coordinates in this file are integer nanometers.
pub const PATH: &str = "project/foilfern-schematic-requirements.json";
pub const VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    File(#[from] crate::fileio::Error),
    #[error("Invalid schematic requirements file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unsupported schematic requirements version {0}; expected {VERSION}.")]
    Version(u32),
    #[error("A requirement needs a nonempty ID and source.")]
    EmptyMetadata,
    #[error("Duplicate requirement ID {0:?}.")]
    Duplicate(String),
    #[error("The lower corner of requirement {0:?} must precede its upper corner on both axes.")]
    InvertedRegion(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Hard,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub id: String,
    pub source: String,
    pub severity: Severity,
    pub check: Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    SymbolPosition {
        symbol: SymbolRef,
        position: Point,
    },
    SymbolOrientation {
        symbol: SymbolRef,
        rotation: Angle,
        mirrored: bool,
    },
    SymbolRegion {
        symbol: SymbolRef,
        min: Point,
        max: Point,
    },
    PinNet {
        pin: ComponentSignalRef,
        #[serde(deserialize_with = "Deserialize::deserialize")]
        net: Option<NetSignalId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    version: u32,
    requirements: Vec<Requirement>,
}

impl Requirements {
    pub fn new(version: u32, mut requirements: Vec<Requirement>) -> Result<Self, Error> {
        if version != VERSION {
            return Err(Error::Version(version));
        }
        let mut ids = BTreeSet::new();
        for requirement in &requirements {
            if requirement.id.trim().is_empty() || requirement.source.trim().is_empty() {
                return Err(Error::EmptyMetadata);
            }
            if !ids.insert(&requirement.id) {
                return Err(Error::Duplicate(requirement.id.clone()));
            }
            if let Check::SymbolRegion { min, max, .. } = requirement.check
                && (min.x > max.x || min.y > max.y)
            {
                return Err(Error::InvertedRegion(requirement.id.clone()));
            }
        }
        requirements.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        Ok(Self {
            version,
            requirements,
        })
    }

    pub fn load(project: &Project) -> Result<Self, Error> {
        let Some(data) = project.directory().read_if_exists(PATH)? else {
            return Self::new(VERSION, Vec::new());
        };
        let document: Self = serde_json::from_slice(&data)?;
        Self::new(document.version, document.requirements)
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn items(&self) -> &[Requirement] {
        &self.requirements
    }

    /// Returns the bytes to stage in an editor transaction. An empty set clears the asset.
    pub fn asset(&self) -> Result<Option<Vec<u8>>, Error> {
        if self.requirements.is_empty() {
            return Ok(None);
        }
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(Some(bytes))
    }

    pub fn evaluate(&self, project: &Project) -> Vec<Finding> {
        self.requirements
            .iter()
            .map(|requirement| Finding {
                requirement: requirement.clone(),
                observation: requirement.check.evaluate(project),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReference {
    Schematic,
    Symbol,
    Component,
    Signal,
    Net,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measured {
    Position(Point),
    Orientation { rotation: Angle, mirrored: bool },
    Net(Option<NetSignalId>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Pass(Measured),
    Fail(Measured),
    Unresolved(MissingReference),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub requirement: Requirement,
    pub observation: Observation,
}

impl Finding {
    pub fn blocks_apply(&self) -> bool {
        self.requirement.severity == Severity::Hard
            && !matches!(self.observation, Observation::Pass(_))
    }
}

impl Check {
    fn evaluate(self, project: &Project) -> Observation {
        let observation = match self {
            Self::SymbolPosition { symbol, position } => placed_symbol(project, symbol)
                .map(SchematicSymbol::position)
                .map(|actual| (Measured::Position(actual), actual == position)),
            Self::SymbolOrientation {
                symbol,
                rotation,
                mirrored,
            } => placed_symbol(project, symbol).map(|placed| {
                let actual = placed.rotation().mapped_to_0_360deg();
                (
                    Measured::Orientation {
                        rotation: actual,
                        mirrored: placed.mirrored(),
                    },
                    actual == rotation.mapped_to_0_360deg() && placed.mirrored() == mirrored,
                )
            }),
            Self::SymbolRegion { symbol, min, max } => placed_symbol(project, symbol)
                .map(SchematicSymbol::position)
                .map(|actual| {
                    let inside =
                        (min.x..=max.x).contains(&actual.x) && (min.y..=max.y).contains(&actual.y);
                    (Measured::Position(actual), inside)
                }),
            Self::PinNet { pin, net } => {
                pin_net(project, pin, net).map(|actual| (Measured::Net(actual), actual == net))
            }
        };
        match observation {
            Ok((measured, true)) => Observation::Pass(measured),
            Ok((measured, false)) => Observation::Fail(measured),
            Err(reference) => Observation::Unresolved(reference),
        }
    }
}

fn placed_symbol(
    project: &Project,
    symbol: SymbolRef,
) -> Result<&SchematicSymbol, MissingReference> {
    project
        .schematic(symbol.schematic)
        .ok_or(MissingReference::Schematic)?
        .symbols()
        .get(&symbol.symbol)
        .ok_or(MissingReference::Symbol)
}

fn pin_net(
    project: &Project,
    pin: ComponentSignalRef,
    expected: Option<NetSignalId>,
) -> Result<Option<NetSignalId>, MissingReference> {
    let circuit = project.circuit();
    let signal = circuit
        .component_instance(pin.component)
        .ok_or(MissingReference::Component)?
        .signal(&pin.signal)
        .ok_or(MissingReference::Signal)?;
    if expected.is_some_and(|id| circuit.net_signal(id).is_none()) {
        return Err(MissingReference::Net);
    }
    Ok(signal.net())
}
