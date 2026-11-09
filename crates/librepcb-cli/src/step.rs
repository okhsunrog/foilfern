//! Port of apps/librepcb-cli/commandlineinterface.cpp's openStep().
//! Uses the independent pure-Rust kernel, without a window system or GPU.
use crate::{StepOptions, error::Result, support};
use librepcb_core::fileio::file_utils;
use librepcb_step::StepModel;

pub fn run(options: StepOptions) -> Result<bool> {
    let path = support::path(&options.file)?;
    println!(
        "Open STEP file '{}'...",
        support::pretty(&path, &options.file)
    );
    let bytes = file_utils::read_file(&path)?;
    // Upstream deliberately saves before validation so invalid input can be
    // inspected. FileUtils retains its atomic replacement semantics.
    if let Some(destination) = options.save_to {
        let path = support::path(&destination)?;
        println!("Save to '{}'...", support::pretty(&path, &destination));
        file_utils::write_file(&path, &bytes)?;
    }
    println!("Load model...");
    let model = StepModel::from_step(&bytes)?;
    if options.tesselate {
        println!("Tesselate model...");
        let stats = model.tessellation_stats(0.01)?;
        println!(
            " - Built {} vertices with {} different colors",
            stats.vertices, stats.colors
        );
        if stats.vertices == 0 {
            eprintln!(" - ERROR: No content found in model!");
            return Ok(false);
        }
    }
    Ok(true)
}
