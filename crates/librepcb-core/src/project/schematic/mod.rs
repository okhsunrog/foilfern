//! Port of libs/librepcb/core/project/schematic (schematic pages and their
//! items).
//!
//! A [`Schematic`] holds symbols ([`SchematicSymbol`], whose pins are the
//! computed [`SymbolPinView`]s), net segments ([`SchematicNetSegment`]:
//! junctions, net lines, net labels), bus segments
//! ([`SchematicBusSegment`]), polygons, texts and images. Items are plain
//! data referencing each other and the circuit by UUID; they are validated
//! and modified through the project mutations
//! ([`SchematicMutation`](crate::project::SchematicMutation)).
//!
//! Not ported: `schematicpainter` (rendering, later milestone).

mod anchor_index;
mod bus_segment;
mod change;
pub mod import_appearance;
mod net_segment;
mod net_segment_splitter;
#[allow(clippy::module_inception)] // Upstream file name.
mod schematic;
mod symbol;
mod uuid_map;

pub(crate) use anchor_index::{SegmentAnchors, check_bus_segment, check_net_segment};
pub use bus_segment::SchematicBusSegment;
pub use change::SchematicChange;
pub use net_segment::SchematicNetSegment;
pub use net_segment_splitter::{SchematicNetSegmentSplitter, SplitSegment};
pub use schematic::{Schematic, SchematicDerived, SchematicProperties};
pub(crate) use schematic::{bus_segment_reference, net_segment_reference, symbol_reference};
pub use symbol::{ResolvedSymbol, SchematicSymbol, SymbolPinView};
