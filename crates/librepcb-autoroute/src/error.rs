//! Errors of the autorouter (invalid problem descriptions).

use librepcb_i18n::tr;

/// An invalid routing problem.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The board outline is empty or degenerate.
    #[error("{}", tr!("Autorouter", "The board has no valid outline."))]
    NoOutline,
    /// No copper layer may be used for routing.
    #[error("{}", tr!("Autorouter", "No copper layer is available for routing."))]
    NoRoutingLayers,
    /// A connection references a terminal which does not exist.
    #[error("{}", tr!("Autorouter", "Connection references unknown terminal {0}.", .0))]
    UnknownTerminal(usize),
    /// A connection connects terminals of different nets.
    #[error("{}", tr!("Autorouter", "Terminals {0} and {1} belong to different nets.", .0, .1))]
    NetMismatch(usize, usize),
    /// The via drill is not smaller than the via diameter.
    #[error("{}", tr!("Autorouter", "The via drill must be smaller than the via diameter."))]
    InvalidVia,
    /// A clearance rule is negative.
    #[error("{}", tr!("Autorouter", "Clearances must not be negative."))]
    NegativeClearance,
    /// The routing grid would be too large.
    #[error("{}", tr!("Autorouter", "The routing grid is too large ({0} cells), use a coarser grid.", .0))]
    GridTooLarge(u64),
}

/// Result of the autorouter.
pub type Result<T, E = Error> = std::result::Result<T, E>;
