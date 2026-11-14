//! Port of libs/librepcb/core/job (output jobs).
//!
//! Output jobs describe which production data a project generates (PDFs,
//! Gerber files, BOMs, archives, ...); they are stored in
//! `project/jobs.lp` and in organization templates. This module holds the
//! plain data model and its (byte-identical) serialization; running jobs
//! (upstream `OutputJobRunner`) is not part of it.
//!
//! - [`OutputJob`]: UUID, name and forward compatibility options common to
//!   all jobs, plus the type specific settings [`OutputJobKind`], an enum a
//!   job runner can `match` on.
//! - One settings struct per job type ([`GraphicsOutputJob`],
//!   [`GerberExcellonOutputJob`], ...), implementing [`OutputJobType`].
//!   Jobs of unknown types are kept verbatim ([`UnknownOutputJob`]).
//! - [`ObjectSet`]: the boards/assembly variants a job runs for.
//! - [`OutputJobList`]: the list stored in `jobs.lp` (`job` children).
//! - [`default_output_jobs()`]: the default set of jobs offered by the
//!   editor.
//!
//! Specctra DSN export has no output job upstream (it is an editor
//! command), so there is none here either.

mod archive;
mod board_3d;
mod bom;
mod copy;
mod default_jobs;
mod gerber_excellon;
mod gerber_x3;
mod graphics;
mod interactive_html_bom;
mod lppz;
mod netlist;
mod output_job;
mod pick_place;
mod project_json;
mod unknown;

pub use archive::ArchiveOutputJob;
pub use board_3d::Board3DOutputJob;
pub use bom::BomOutputJob;
pub use copy::CopyOutputJob;
pub use default_jobs::default_output_jobs;
pub use gerber_excellon::GerberExcellonOutputJob;
pub use gerber_x3::GerberX3OutputJob;
pub use graphics::{
    GraphicsAssemblyVariantSet, GraphicsBoardSet, GraphicsContent, GraphicsContentPreset,
    GraphicsContentType, GraphicsOutputJob,
};
pub use interactive_html_bom::InteractiveHtmlBomOutputJob;
pub use lppz::LppzOutputJob;
pub use netlist::NetlistOutputJob;
pub use output_job::{
    ObjectSet, OutputJob, OutputJobKind, OutputJobList, OutputJobListTag, OutputJobType,
};
pub use pick_place::{PickPlaceOutputJob, PickPlaceTechnologies};
pub use project_json::ProjectJsonOutputJob;
pub use unknown::UnknownOutputJob;

static_assertions::assert_impl_all!(OutputJobList: Send, Sync);
