//! Port of libs/librepcb/core/project/circuit/componentsignalinstance.{h,cpp}.
//!
//! Differences to upstream: only the persistent state (the connected net)
//! is stored. The registered symbol pins and footprint pads
//! (`getRegisteredSymbolPins()`, `getRegisteredFootprintPads()`,
//! `isUsed()`, `arePinsOrPadsUsed()`) are derived from the project's
//! reverse index and the library; the pad names (`getPadNames()`) are
//! computed on demand; the `netSignalChanged`/`padNamesChanged` signals are
//! not ported.

use crate::geometry::property;
use crate::project::id::NetSignalId;

/// The instance of a component signal in the circuit: its connection to a
/// net. Keyed by the UUID of the library signal in
/// [`ComponentInstance::signals()`](super::ComponentInstance::signals).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComponentSignalInstance {
    net: Option<NetSignalId>,
}

impl ComponentSignalInstance {
    /// Creates a signal instance connected to `net` (or unconnected).
    pub fn new(net: Option<NetSignalId>) -> Self {
        Self { net }
    }

    property!(
        /// Returns the connected net, if any.
        copy net: Option<NetSignalId>, set_net
    );
}
