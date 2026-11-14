//! Project-associated requirements for PCB placement and routing (Foilfern extension).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{BoardId, ComponentInstanceId, NetSignalId, Project};
use crate::fileio::FileSystem;
use crate::types::{Layer, Point, PositiveLength, Uuid};

pub use super::schematic_requirements::Severity;

pub const PATH: &str = "project/foilfern-board-requirements.json";
pub const VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    File(#[from] crate::fileio::Error),
    #[error("Invalid board requirements file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unsupported board requirements version {0}; expected {VERSION}.")]
    Version(u32),
    #[error("Each board requirement needs a nonempty ID and source.")]
    EmptyMetadata,
    #[error("Duplicate board requirement ID {0:?}.")]
    Duplicate(String),
    #[error("Invalid check for board requirement {0:?}.")]
    InvalidCheck(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    pub component: ComponentInstanceId,
    pub pad: Uuid,
}

impl Pad {
    pub fn address(self) -> (ComponentInstanceId, Uuid) {
        (self.component, self.pad)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    DeviceRegion {
        component: ComponentInstanceId,
        min: Point,
        max: Point,
    },
    PadDistance {
        from: Pad,
        to: Pad,
        max_distance: PositiveLength,
    },
    TracePath {
        from: Pad,
        to: Pad,
        net: NetSignalId,
        max_length: Option<PositiveLength>,
        max_layer_changes: Option<usize>,
        layers: Option<BTreeSet<Layer>>,
    },
    NetRouting {
        net: NetSignalId,
        layers: Option<BTreeSet<Layer>>,
        allow_vias: Option<bool>,
        #[serde(default)]
        priority: bool,
    },
    ProtectedCopper {
        object: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub id: String,
    pub source: String,
    pub severity: Severity,
    pub board: BoardId,
    pub check: Check,
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
        let valid_layers = |layers: &Option<BTreeSet<Layer>>| {
            layers.as_ref().is_none_or(|layers| {
                !layers.is_empty() && layers.iter().all(|layer| layer.is_copper())
            })
        };
        for rule in &requirements {
            if rule.id.trim().is_empty() || rule.source.trim().is_empty() {
                return Err(Error::EmptyMetadata);
            }
            if !ids.insert(&rule.id) {
                return Err(Error::Duplicate(rule.id.clone()));
            }
            let valid = match &rule.check {
                Check::DeviceRegion { min, max, .. } => min.x <= max.x && min.y <= max.y,
                Check::PadDistance { from, to, .. } => from != to,
                Check::TracePath {
                    from, to, layers, ..
                } => from != to && valid_layers(layers),
                Check::NetRouting {
                    layers,
                    allow_vias,
                    priority,
                    ..
                } => {
                    valid_layers(layers) && (layers.is_some() || allow_vias.is_some() || *priority)
                }
                Check::ProtectedCopper { .. } => true,
            };
            if !valid {
                return Err(Error::InvalidCheck(rule.id.clone()));
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

    pub fn on_board(&self, board: BoardId) -> impl Iterator<Item = &Requirement> {
        self.items().iter().filter(move |rule| rule.board == board)
    }

    pub fn protects(&self, board: BoardId, object: Uuid) -> bool {
        self.on_board(board).any(|rule| {
            rule.severity == Severity::Hard
                && matches!(rule.check, Check::ProtectedCopper { object: id } if id == object)
        })
    }

    pub fn asset(&self) -> Result<Option<Vec<u8>>, Error> {
        if self.requirements.is_empty() {
            return Ok(None);
        }
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(Some(bytes))
    }
}
