//! Port of libs/librepcb/core/job/*. Output jobs are owned data, with an
//! enum replacing the class hierarchy. Serialization is implemented here
//! because LibrePCB's field order and line breaks are part of its format.
//! Unknown jobs and arbitrary option nodes survive unchanged.

mod error;
mod graphics;
mod output_job;
mod selection;

pub use graphics::{GraphicsContent, GraphicsContentType, GraphicsJob, PageOrientation};
pub use output_job::*;
pub use selection::ObjectSet;

pub use error::{Error, Result};
