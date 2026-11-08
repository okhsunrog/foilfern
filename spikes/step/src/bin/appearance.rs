//! Known RGB/opacity probe built from an external generated plate BRep.
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;
use step_spike::{
    materials::{Material, export_step},
    metadata::StepDocument,
    placed_solids,
};
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let input = PathBuf::from(
        args.next()
            .context("usage: appearance PLATE.step OUTPUT.step")?,
    );
    let output = PathBuf::from(args.next().context("missing output path")?);
    let document = StepDocument::parse(&std::fs::read(&input)?)?;
    let mut solids = placed_solids(&document)?;
    ensure!(
        solids.len() == 1 && solids[0].appearance.len() == 1 && solids[0].appearance[0].len() == 14,
        "expected analytic plate with fourteen faces"
    );
    let materials = &mut solids[0].appearance[0];
    materials.fill(Some(Material {
        color: [0.2, 0.4, 0.6],
        opacity: 0.35,
    }));
    for (index, color, opacity) in [
        (0, [1., 0., 0.], 1.),
        (1, [0., 1., 0.], 1.),
        (2, [0., 1., 1.], 0.25),
    ] {
        materials[index] = Some(Material { color, opacity });
    }
    std::fs::write(&output, export_step(&solids)?)?;
    let expected = serde_json::json!({"materials":[{"color":[0.2,0.4,0.6],"opacity":0.35,"faces":11},{"color":[1.,0.,0.],"opacity":1.,"faces":1},{"color":[0.,1.,0.],"opacity":1.,"faces":1},{"color":[0.,1.,1.],"opacity":0.25,"faces":1}]});
    std::fs::write(
        output.with_extension("appearance.json"),
        serde_json::to_string_pretty(&expected)?,
    )?;
    Ok(())
}
