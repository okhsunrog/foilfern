//! Analytic plate probe: a true BRep extrusion with circular and rectangular holes.
use anyhow::{Context, Result, ensure};
use monstertruck_io::step::save;
use monstertruck_modeling::*;
use std::path::PathBuf;

fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64) -> Wire {
    let vertices = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .map(|(x, y)| builder::vertex(Point3::new(x, y, 0.)));
    (0..4)
        .map(|i| builder::line(&vertices[i], &vertices[(i + 1) % 4]))
        .collect()
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output = PathBuf::from(
        args.next()
            .context("usage: plate OUTPUT.step [COORDINATE_SCALE]")?,
    );
    let scale = args
        .next()
        .map(|value| value.to_string_lossy().parse::<f64>())
        .transpose()?
        .unwrap_or(1.);
    ensure!(scale.is_finite() && scale > 0., "invalid coordinate scale");
    let circle = builder::revolve(
        &builder::vertex(Point3::new(5.5, 5., 0.)),
        Point3::new(4., 5., 0.),
        Vector3::unit_z(),
        builder::SweepAngle::Closed,
        4,
    );
    let solid: Solid = profile::solid_from_planar_profile(
        vec![
            rectangle(0., 0., 20., 10.),
            circle,
            rectangle(12., 4., 16., 6.),
        ],
        Vector3::new(0., 0., 1.6),
    )?;
    ensure!(solid.is_geometric_consistent(), "inconsistent plate BRep");
    let compressed = builder::transformed(&solid, Matrix4::from_scale(scale)).compress();
    let step = save::CompleteStepDisplay::new(
        save::StepModel::from(&compressed),
        save::StepHeaderDescriptor::default(),
    );
    std::fs::write(&output, step.to_string())?;
    let expected = serde_json::json!({"volume":(200. - 8. - std::f64::consts::PI * 2.25) * 1.6 * scale.powi(3),"bounds":[[0.,0.,0.],[20.*scale,10.*scale,1.6*scale]],"solids":1});
    std::fs::write(
        output.with_extension("expected.json"),
        serde_json::to_string_pretty(&expected)?,
    )?;
    Ok(())
}
