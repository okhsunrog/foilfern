//! Port of the default set of output jobs, which upstream creates in the
//! editor (libs/librepcb/editor/project/outputjobsdialog/outputjobsdialog.cpp,
//! "add a default set of jobs"). New projects start without output jobs.

use super::{
    ArchiveOutputJob, BomOutputJob, GerberExcellonOutputJob, GraphicsOutputJob, LppzOutputJob,
    OutputJob, OutputJobList, OutputJobType, PickPlaceOutputJob,
};
use crate::types::Uuid;

/// Returns the default set of output jobs: schematic PDF, board assembly
/// PDF, Gerber/Excellon (with the settings `gerber`), pick&place CSV, BOM
/// CSV (with the columns `custom_bom_attributes`), a ZIP archive of the
/// Gerber files and the project archive.
///
/// Upstream passes the legacy fabrication output settings of the first
/// board as `gerber` if there is a board
/// ([`GerberExcellonOutputJob::from_fabrication_settings()`]), otherwise the
/// default style, and the project's custom BOM attributes.
pub fn default_output_jobs(
    gerber: GerberExcellonOutputJob,
    custom_bom_attributes: Vec<String>,
) -> OutputJobList {
    let gerber = OutputJob::new(
        Uuid::new_random(),
        GerberExcellonOutputJob::default_name(),
        gerber,
    );
    let bom = BomOutputJob {
        custom_attributes: custom_bom_attributes,
        ..BomOutputJob::default()
    };
    let archive = ArchiveOutputJob {
        input_jobs: [(gerber.uuid(), String::new())].into(),
        ..ArchiveOutputJob::default()
    };
    [
        GraphicsOutputJob::schematic_pdf(),
        GraphicsOutputJob::board_assembly_pdf(),
        gerber,
        OutputJob::new_default::<PickPlaceOutputJob>(),
        OutputJob::new(Uuid::new_random(), BomOutputJob::default_name(), bom),
        OutputJob::new(
            Uuid::new_random(),
            ArchiveOutputJob::default_name(),
            archive,
        ),
        OutputJob::new_default::<LppzOutputJob>(),
    ]
    .into_iter()
    .collect()
}
