//! Render a real project headlessly with Bevy: render_board project.lpp[z] out.png.
use foilfern_core::{
    fileio::{FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem},
    project::{AssemblyVariantId, BoardId, ProjectLoader},
};
use foilfern_render3d::{
    BoardMesh, OrbitCamera,
    renderer::{Renderer, Visibility},
};
use std::{path::Path, sync::Arc};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let input = args
        .get(1)
        .ok_or("expected project.lpp[z] output.png [bottom]")?;
    let output = args.get(2).ok_or("expected output.png")?;
    let input = FilePath::new(Path::new(input).canonicalize()?).ok_or("invalid project path")?;
    let (dir, file) = if input.suffix() == "lppz" {
        let dir = TransactionalDirectory::new_temporary()?;
        dir.file_system().load_from_zip(&input)?;
        let file = dir
            .files("")
            .into_iter()
            .find(|f| f.ends_with(".lpp"))
            .ok_or("archive has no project")?;
        (dir, file)
    } else {
        (
            TransactionalDirectory::new(
                Arc::new(TransactionalFileSystem::open_ro(
                    &input.parent_dir().ok_or("no parent")?,
                )?),
                "",
            ),
            input.file_name().to_owned(),
        )
    };
    let mut project = ProjectLoader::new().open(dir, &file)?;
    let board = BoardId(
        project
            .boards()
            .first()
            .ok_or("project has no boards")?
            .uuid(),
    );
    project.rebuild_planes(board, None)?;
    let variant = project
        .circuit()
        .assembly_variants()
        .first()
        .map(|v| AssemblyVariantId(v.uuid()));
    let mesh = BoardMesh::from_project(&project, board, variant)?;
    eprintln!(
        "{} material groups, {} triangles, bounds {:?}, warnings {:?}",
        mesh.parts.len(),
        mesh.parts
            .iter()
            .map(|p| p.positions.len() / 3)
            .sum::<usize>(),
        mesh.bounds,
        mesh.warnings
    );
    let mut camera = OrbitCamera::fit(mesh.bounds);
    if args.get(3).is_some_and(|s| s == "bottom") {
        camera.orbit([0., -210.]);
    }
    let mut renderer = Renderer::new()?;
    eprintln!("GPU: {}", renderer.adapter);
    renderer.set_mesh(&mesh);
    let frame = renderer.render(1400, 1000, camera, Visibility::default())?;
    let mut encoder = png::Encoder::new(std::fs::File::create(output)?, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&frame.rgba)?;
    Ok(())
}
