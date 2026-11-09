//! Incremental scene updates: after every step of editing sequences (the
//! editor commands of `librepcb-editor`, undo and redo, derived data
//! updates), the incrementally updated schematic and board scenes must be
//! equal to freshly built ones (compared by a canonical dump of the items
//! per model object; item IDs may differ).

mod helpers;
mod perf;
mod random;
mod workflow;
