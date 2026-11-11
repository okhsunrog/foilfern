//! Read-only replay for the experimental rational copper union.
//! Usage: drc_copper_benchmark PROJECT.lpp legacy|exact|layers [REPEATS]
//! Build with --release --features experimental-exact-copper.

use clipper::{ClipType, Clipper, PolyFillType::EvenOdd, PolyType};
use librepcb_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem},
    project::{
        ProjectLoader,
        board::drc::{BoardClipperPathGenerator, run_drc, run_drc_with_exact_copper},
    },
    types::{Length, PositiveLength},
    utils::rational_union,
};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let input = Path::new(
        args.get(1)
            .ok_or("Expected PROJECT.lpp legacy|exact|layers [REPEATS]")?,
    );
    let mode = args.get(2).map(String::as_str).unwrap_or("layers");
    if !["legacy", "exact", "layers"].contains(&mode) {
        return Err("Unknown benchmark mode".into());
    }
    let repeat: usize = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(1);
    let directory = input.parent().ok_or("Project has no parent directory")?;
    let file = input
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Invalid project filename")?;
    let directory = FilePath::new(directory).ok_or("Invalid project directory")?;
    let fs = TransactionalFileSystem::open_ro(&directory)?;
    let mut project =
        ProjectLoader::new().open(TransactionalDirectory::new(Arc::new(fs), ""), file)?;
    let boards: Vec<_> = project
        .boards_with_ids()
        .map(|(id, b)| (id, b.properties().name.to_string()))
        .collect();
    for (id, name) in boards {
        for sample in 0..repeat {
            let start = Instant::now();
            let data = project
                .drc_job(id, None, false)?
                .run()
                .apply(&mut project)?;
            if mode == "layers" {
                for &layer in &data.copper_layers {
                    let tolerance = PositiveLength::new(Length::new(5000))?;
                    let start = Instant::now();
                    let mut legacy = BoardClipperPathGenerator::new(tolerance);
                    legacy.add_copper(&data, layer, &[], false)?;
                    let legacy_ms = start.elapsed().as_secs_f64() * 1000.;
                    let start = Instant::now();
                    let operands =
                        BoardClipperPathGenerator::copper_operands(&data, layer, tolerance)?;
                    let collect_ms = start.elapsed().as_secs_f64() * 1000.;
                    let start = Instant::now();
                    let (exact, stats) = rational_union::union(&operands)?;
                    let exact_ms = start.elapsed().as_secs_f64() * 1000.;
                    let rounded = exact.rounded_paths()?;
                    let mut comparison = Clipper::new();
                    comparison.add_paths(legacy.paths(), PolyType::Subject, true)?;
                    comparison.add_paths(&rounded, PolyType::Clip, true)?;
                    let difference = comparison.execute(ClipType::Xor, EvenOdd, EvenOdd)?;
                    println!(
                        "{}",
                        json!({"board":name,"sample":sample,"layer":format!("{layer:?}"),"legacy_ms":legacy_ms,"collect_ms":collect_ms,"exact_ms":exact_ms,"rounded_area_matches":difference.is_empty(),"difference_paths":difference.len(),"exact_paths":exact.paths().len(),"fractional_vertices":exact.paths().iter().flatten().filter(|p|!p.x.is_int()||!p.y.is_int()).count(),"segments":stats.input_segments,"candidates":stats.candidate_pairs,"intersections":stats.intersections,"atomic_edges":stats.atomic_edges,"boundary_edges":stats.boundary_edges})
                    );
                }
            } else {
                let result = if mode == "exact" {
                    run_drc_with_exact_copper(&data, &|_| {})
                } else {
                    run_drc(&data, &|_| {})
                };
                println!(
                    "{}",
                    json!({"board":name,"sample":sample,"mode":mode,"full_ms":start.elapsed().as_secs_f64()*1000.,"checks_ms":result.elapsed.as_secs_f64()*1000.,"errors":result.errors,"messages":result.messages.iter().map(|m|format!("{m:?}")).collect::<Vec<_>>(),"approvals":result.messages.iter().map(|m|format!("{:?}",m.approval())).collect::<Vec<_>>()})
                );
            }
        }
    }
    Ok(())
}
