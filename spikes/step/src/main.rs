//! Feasibility probe for Monstertruck STEP geometry. Not a production importer.
//! Uses the reported conversion API: missing geometry must stay visible.
use anyhow::{Context, Result, ensure};
use monstertruck_io::step::{load::Table, save};
use monstertruck_meshing::prelude::*;
use monstertruck_topology::compress::CompressedSolid;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Instant,
};

fn assembly_report(table: &Table, output: &Path, tolerance: f64) -> Result<Value> {
    let assembly = table.step_assy()?;
    let mut instances = Vec::new();
    let mut solids = Vec::new();
    let mut reached = BTreeSet::new();
    for root in assembly.top_nodes() {
        for path in assembly.paths_iter(root.index()) {
            let node = path.terminal_node();
            let mut matrix = Matrix4::identity();
            for edge in path.edges() {
                matrix =
                    matrix * Matrix4::try_from(edge.matrix()).map_err(anyhow::Error::from_boxed)?;
            }
            let mut ids: BTreeSet<_> = node
                .shape()
                .iter()
                .copied()
                .filter(|id| table.manifold_solid_brep.contains_key(id))
                .collect();
            if let Some(representation) = table.shape_representation_of_node(node.entity()) {
                let (related, report) = table.solids_via_shape_relationship(representation);
                report.require_lossless()?;
                ids.extend(related);
            }
            for id in ids {
                let (mut solid, reports) =
                    table.to_compressed_solid_reported(&table.manifold_solid_brep[&id])?;
                for report in reports {
                    report.require_lossless()?;
                }
                let mut stats = Vec::new();
                for shell in &mut solid.boundaries {
                    for vertex in &mut shell.vertices {
                        *vertex = matrix.transform_point(*vertex);
                    }
                    for edge in &mut shell.edges {
                        edge.curve.transform_by(matrix);
                    }
                    for face in &mut shell.faces {
                        face.surface.transform_by(matrix);
                    }
                    let meshed = shell.robust_triangulation(tolerance);
                    ensure!(
                        meshed.faces.iter().all(|face| face.surface.is_some()),
                        "assembly has untessellated face"
                    );
                    stats.push(mesh_stats(meshed.to_polygon())?);
                }
                instances.push(json!({"entity":id,"name":node.attributes().name,"shells":stats}));
                solids.push(solid);
                reached.insert(id);
            }
        }
    }
    ensure!(!solids.is_empty(), "assembly contains no supported solids");
    ensure!(
        reached.len() == table.manifold_solid_brep.len(),
        "assembly traversal missed source solids"
    );
    let models: save::StepModels<_, _, _> = solids.iter().collect();
    let text =
        save::CompleteStepDisplay::new(models, save::StepHeaderDescriptor::default()).to_string();
    let path = output.join("assembly.step");
    std::fs::write(&path, text)?;
    Ok(
        json!({"instances":instances,"solid_count":solids.len(),"roundtrip_file":path,"metadata_preserved":false}),
    )
}

fn mesh_stats(mut mesh: PolygonMesh) -> Result<Value> {
    mesh.put_together_same_attrs(1e-8).remove_unused_attrs();
    ensure!(
        mesh.quad_faces().is_empty() && mesh.other_faces().is_empty(),
        "non-triangular output"
    );
    ensure!(!mesh.tri_faces().is_empty(), "empty mesh");
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    let mut volume = 0.;
    let mut edges = BTreeMap::<(usize, usize), (usize, i32)>::new();
    for triangle in mesh.tri_faces() {
        for vertex in triangle {
            let p = mesh
                .positions()
                .get(vertex.pos)
                .context("invalid vertex index")?;
            for axis in 0..3 {
                ensure!(p[axis].is_finite(), "non-finite mesh coordinate");
                low[axis] = low[axis].min(p[axis]);
                high[axis] = high[axis].max(p[axis]);
            }
        }
        let [a, b, c] = triangle.map(|v| mesh.positions()[v.pos]);
        volume += a.to_vec().dot(b.to_vec().cross(c.to_vec())) / 6.;
        for i in 0..3 {
            let (a, b) = (triangle[i].pos, triangle[(i + 1) % 3].pos);
            let key = (a.min(b), a.max(b));
            let edge = edges.entry(key).or_default();
            edge.0 += 1;
            edge.1 += if a < b { 1 } else { -1 };
        }
    }
    Ok(
        json!({"vertices":mesh.positions().len(),"triangles":mesh.tri_faces().len(),"bounds":[low,high],"signed_volume":volume,
        "boundary_edges":edges.values().filter(|(count,_)| *count==1).count(),
        "nonmanifold_edges":edges.values().filter(|(count,_)| *count>2).count(),
        "misoriented_edges":edges.values().filter(|(count,balance)| *count==2 && *balance!=0).count()}),
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
