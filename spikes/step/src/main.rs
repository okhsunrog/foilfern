//! Feasibility probe for Monstertruck STEP geometry. Not a production importer.
//! Uses the reported conversion API: missing geometry must stay visible.
use anyhow::{Context, Result, ensure};
use monstertruck_io::step::{load::Table, save};
use monstertruck_meshing::prelude::*;
use monstertruck_topology::compress::CompressedSolid;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};
use step_spike::{mesh_stats, placed_solids};

fn assembly_report(table: &Table, output: &Path, tolerance: f64) -> Result<Value> {
    let placed = placed_solids(table)?;
    let mut instances = Vec::new();
    for item in &placed {
        let mut stats = Vec::new();
        for shell in &item.solid.boundaries {
            let meshed = shell.robust_triangulation(tolerance);
            ensure!(
                meshed.faces.iter().all(|face| face.surface.is_some()),
                "assembly has untessellated face"
            );
            stats.push(mesh_stats(meshed.to_polygon())?);
        }
        instances.push(json!({"entity":item.entity,"name":item.name,"shells":stats}));
    }
    let solids: Vec<_> = placed.into_iter().map(|item| item.solid).collect();
    let models: save::StepModels<_, _, _> = solids.iter().collect();
    let text =
        save::CompleteStepDisplay::new(models, save::StepHeaderDescriptor::default()).to_string();
    let path = output.join("assembly.step");
    std::fs::write(&path, text)?;
    Ok(
        json!({"instances":instances,"solid_count":solids.len(),"roundtrip_file":path,"metadata_preserved":false}),
    )
}

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let input = PathBuf::from(
        arguments
            .next()
            .context("usage: step-spike INPUT.step OUTPUT_DIR [TOLERANCE_MM]")?,
    );
    let output = PathBuf::from(arguments.next().context("missing output directory")?);
    let tolerance = arguments
        .next()
        .map(|v| v.to_string_lossy().parse::<f64>())
        .transpose()?
        .unwrap_or(0.01);
    ensure!(tolerance.is_finite() && tolerance > 0., "invalid tolerance");
    std::fs::create_dir_all(&output)?;
    let started = Instant::now();
    let bytes = std::fs::read(&input)?;
    let table = Table::from_step_bytes(&bytes)?;
    let mut shells: Vec<_> = table.shell.iter().collect();
    shells.sort_by_key(|(id, _)| **id);
    let mut results = Vec::new();
    for (id, step_shell) in shells {
        let before = Instant::now();
        let result: Result<Value> = (|| {
            let (shell, report) = table.to_compressed_shell_reported(step_shell)?;
            let meshed = shell.robust_triangulation(tolerance);
            let missing_mesh_faces = meshed.faces.iter().filter(|f| f.surface.is_none()).count();
            let stats = mesh_stats(meshed.to_polygon())?;
            let solid = CompressedSolid {
                boundaries: vec![shell.clone()],
                id_allocator: None,
                attributes: None,
            };
            let text = save::CompleteStepDisplay::new(
                save::StepModel::from(&solid),
                save::StepHeaderDescriptor {
                    organization_system: "librepcb-step-probe".into(),
                    ..Default::default()
                },
            )
            .to_string();
            let roundtrip_path = output.join(format!("shell-{id}.step"));
            std::fs::write(&roundtrip_path, &text)?;
            let second = Table::from_step_bytes(text.as_bytes())?;
            let holder = second
                .shell
                .values()
                .next()
                .context("round trip has no shell")?;
            let (again, roundtrip_report) = second.to_compressed_shell_reported(holder)?;
            let roundtrip_mesh = again.robust_triangulation(tolerance);
            let roundtrip_missing_mesh_faces = roundtrip_mesh
                .faces
                .iter()
                .filter(|f| f.surface.is_none())
                .count();
            let roundtrip = mesh_stats(roundtrip_mesh.to_polygon())?;
            Ok(
                json!({"entity":id,"source_faces":shell.faces.len(),"losses":report.total_lost(),"loss_report":report.to_string(),
                "missing_mesh_faces":missing_mesh_faces,"mesh":stats,"roundtrip":roundtrip,"roundtrip_losses":roundtrip_report.total_lost(),
                "roundtrip_missing_mesh_faces":roundtrip_missing_mesh_faces,
                "roundtrip_file":roundtrip_path,"metadata_preserved":false,"elapsed_ms":before.elapsed().as_millis()}),
            )
        })();
        results.push(match result { Ok(report)=>report, Err(error)=>json!({"entity":id,"error":format!("{error:#}"),"elapsed_ms":before.elapsed().as_millis()}) });
    }
    let assembly = match assembly_report(&table, &output, tolerance) {
        Ok(report) => report,
        Err(error) => json!({"error":format!("{error:#}")}),
    };
    let report = json!({"input":input,"input_bytes":bytes.len(),"tolerance":tolerance,"shells":results,"assembly":assembly,
        "manifold_solids":table.manifold_solid_brep.len(),"unmapped_entities":table.dummy.len(),"entity_losses":table.entity_report.total(),
        "entity_report":table.entity_report.to_string(),"elapsed_ms":started.elapsed().as_millis()});
    let report = serde_json::to_string_pretty(&report)?;
    std::fs::write(output.join("report.json"), &report)?;
    println!("{report}");
    Ok(())
}
