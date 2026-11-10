use librepcb_render3d::{
    BoardMesh, OrbitCamera,
    renderer::{Renderer, Visibility},
};

#[test]
#[ignore = "requires a Vulkan/Metal/DX12 adapter; run explicitly on a GPU host"]
fn headless_gpu_renders_and_resizes_both_sides() {
    let mesh = BoardMesh::from_physical(&super::geometry::snapshot()).unwrap();
    let mut renderer = Renderer::new().unwrap();
    eprintln!("GPU: {}", renderer.adapter);
    renderer.set_mesh(&mesh);
    let mut camera = OrbitCamera::fit(mesh.bounds);
    let top = renderer
        .render(317, 239, camera, Visibility::default())
        .unwrap();
    assert_eq!(top.rgba.len(), 317 * 239 * 4);
    let center = (119 * 317 + 158) * 4;
    assert_ne!(&top.rgba[center..center + 3], &top.rgba[..3]);
    camera.side(true);
    let bottom = renderer
        .render(401, 307, camera, Visibility::default())
        .unwrap();
    assert_eq!(bottom.rgba.len(), 401 * 307 * 4);
    assert_ne!(&top.rgba[..], &bottom.rgba[..]);
}
