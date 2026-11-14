//! Port of libs/librepcb/core/algorithm (air wires and net segment
//! simplification).

mod air_wires_builder;
mod net_segment_simplifier;

pub use air_wires_builder::{AirWire, AirWiresBuilder};
pub use net_segment_simplifier::{
    AnchorType, NetSegmentLine, NetSegmentSimplifier, SimplifyResult,
};
