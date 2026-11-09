//! Port of libs/librepcb/core/3d/occmodel.cpp's geometry adapter boundary.
//! Uses Monstertruck's reported converters; application entry points guard
//! these lower-level routines and expose errors through the crate API.
use crate::{materials, metadata};
use anyhow::{Context, Result, ensure};
use monstertruck_io::step::load::convert::StepCompressedSolid;
use monstertruck_meshing::prelude::*;
#[cfg(any(test, feature = "diagnostics"))]
use serde_json::{Value, json};
#[cfg(any(test, feature = "diagnostics"))]
use std::collections::BTreeMap;
use std::collections::BTreeSet;

#[derive(Clone)]
pub struct PlacedSolid {
    #[cfg(feature = "diagnostics")]
    pub entity: u64,
    pub name: String,
    pub solid: StepCompressedSolid,
    pub appearance: materials::Appearance,
}
static_assertions::assert_impl_all!(PlacedSolid: Send, Sync);

pub fn transform_solid(solid: &mut StepCompressedSolid, matrix: Matrix4) {
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
    }
}

pub fn tessellate_solid(
    item: &PlacedSolid,
    tolerance: f64,
) -> Result<(PolygonMesh, materials::MaterialGroups)> {
    ensure!(
        tolerance.is_finite() && tolerance > 0.,
        "invalid tessellation tolerance"
    );
    ensure!(
        item.appearance.len() == item.solid.boundaries.len(),
        "material boundary mismatch"
    );
    let mut combined = PolygonMesh::default();
    let mut groups = Vec::new();
    for (shell, materials) in item.solid.boundaries.iter().zip(&item.appearance) {
        let meshed = shell.robust_triangulation(tolerance);
        ensure!(
            meshed.faces.len() == materials.len(),
            "material face mismatch"
        );
        for (face, material) in meshed.faces.into_iter().zip(materials) {
            let mut mesh = face.surface.context("face tessellation failed")?;
            if !face.orientation {
                mesh.invert();
            }
            ensure!(
                mesh.quad_faces().is_empty() && mesh.other_faces().is_empty(),
                "non-triangular tessellation"
            );
            groups.push((mesh.tri_faces().len(), *material));
            combined.merge(mesh);
        }
    }
    Ok((combined, groups))
}

pub fn placed_solids(document: &metadata::StepDocument) -> Result<Vec<PlacedSolid>> {
    let table = &document.table;
    let styles = document.records.surface_styles()?;
    let assembly = table.step_assy()?;
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
                let appearance = document.records.appearance(id, &styles)?;
                ensure!(
                    appearance.len() == solid.boundaries.len()
                        && appearance
                            .iter()
                            .zip(&solid.boundaries)
                            .all(|(materials, shell)| materials.len() == shell.faces.len()),
                    "appearance/topology mismatch"
                );
                transform_solid(
                    &mut solid,
                    Matrix4::from_scale(document.length_to_mm) * matrix,
                );
                solids.push(PlacedSolid {
                    #[cfg(feature = "diagnostics")]
                    entity: id,
                    name: node.attributes().name.clone(),
                    solid,
                    appearance,
                });
                reached.insert(id);
            }
        }
    }
    ensure!(!solids.is_empty(), "assembly contains no supported solids");
    ensure!(
        reached.len() == table.manifold_solid_brep.len(),
        "assembly traversal missed source solids"
    );
    Ok(solids)
}

#[cfg(any(test, feature = "diagnostics"))]
pub fn mesh_stats(mut mesh: PolygonMesh) -> Result<Value> {
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
