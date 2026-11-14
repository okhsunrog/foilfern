//! Port of libs/librepcb/core/project/circuit (the netlist of a project).
//!
//! The circuit owns the assembly variants, net classes, net signals, buses
//! and component instances. All are plain data; the reverse relations
//! upstream keeps as registration lists live in the project's
//! [`RefIndex`](super::ref_index::RefIndex), and all modifications go
//! through [`Project::apply()`](super::Project::apply).

mod assembly_variant;
mod bus;
#[allow(clippy::module_inception)] // Upstream file name.
mod circuit;
mod component_assembly_option;
mod component_instance;
mod component_signal_instance;
mod net_class;
mod net_signal;

pub use assembly_variant::{AssemblyVariant, AssemblyVariantList, AssemblyVariantListTag};
pub use bus::Bus;
pub use circuit::Circuit;
pub use component_assembly_option::{
    ComponentAssemblyOption, ComponentAssemblyOptionList, ComponentAssemblyOptionListTag,
};
pub use component_instance::{ComponentInstance, ComponentInstanceProperties};
pub use component_signal_instance::ComponentSignalInstance;
pub use net_class::NetClass;
pub use net_signal::NetSignal;
