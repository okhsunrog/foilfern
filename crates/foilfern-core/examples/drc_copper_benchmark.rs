//! Read-only replay of the production DRC. Compare older revisions as separate binaries.
//! Usage: drc_copper_benchmark PROJECT.lpp full|layers [REPEATS]
//! Build with --release.

use foilfern_core::{
    fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem},
    project::{
        ProjectLoader,
        board::drc::{GeometryGenerator, run_drc},
    },
    types::{Length, PositiveLength},
};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let input = Path::new(
        args.get(1)
            .ok_or("Expected PROJECT.lpp full|layers [REPEATS]")?,
    );
    let mode = args.get(2).map(String::as_str).unwrap_or("layers");
    if !["full", "layers"].contains(&mode) {
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
                    let mut generator = GeometryGenerator::new(tolerance);
                    generator.add_copper(&data, layer, &[], false)?;
                    let region = generator.take_region()?;
                    println!(
                        "{}",
                        json!({"board":name,"sample":sample,"layer":format!("{layer:?}"),"union_ms":start.elapsed().as_secs_f64()*1000.,"paths":region.paths().len(),"fractional_vertices":region.paths().iter().flatten().filter(|p|!p.x.is_int()||!p.y.is_int()).count()})
                    );
                }
            } else {
                let result = run_drc(&data, &|_| {});
                println!(
                    "{}",
                    json!({"board":name,"sample":sample,"mode":mode,"full_ms":start.elapsed().as_secs_f64()*1000.,"checks_ms":result.elapsed.as_secs_f64()*1000.,"errors":result.errors,"messages":result.messages.iter().map(|m|format!("{m:?}")).collect::<Vec<_>>(),"approvals":result.messages.iter().map(|m|format!("{:?}",m.approval())).collect::<Vec<_>>()})
                );
            }
        }
    }
    Ok(())
}
