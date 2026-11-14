//! Timing test: the full-view stress scene must not render slower than the
//! canvas spike's vello_cpu renderer. Timing depends on the machine and
//! load, so it is ignored by default; run it with
//! `cargo test --release -p foilfern-canvas -- --ignored`.

#[path = "../../benches/spike_baseline.rs"]
mod spike_baseline;

use std::time::Instant;

use foilfern_canvas::kurbo::Vec2;
use foilfern_canvas::{CpuRenderer, CpuRendererSettings, Renderer, Scene, View, stress};
use spike_baseline::Spike;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
#[ignore = "timing test, run in release mode"]
fn test_full_view_not_slower_than_spike() {
    let items = stress::stress_items(100_000, 16_000, 4_000);
    let mut scene = Scene::new();
    stress::add_board_layers(&mut scene);
    scene.extend(items.clone());
    let bbox = scene.bounding_box().unwrap();
    // The spike's window size and fit.
    let (lw, lh) = (2161.0, 1351.0);
    let mut view = View::new((lw, lh), 1.0);
    view.set_scale((lw / bbox.width()).min(lh / bbox.height()) * 0.95);
    view.center_on(bbox.center());
    let (w, h) = view.device_size();
    let len = (w * h * 4) as usize;
    let threads = CpuRendererSettings::default().threads;
    let mut spike = Spike::build(&items, threads, w as u16, h as u16);
    // Frame cache on: a 1 px zoom-free pan would be cheap, so every frame
    // changes the zoom slightly to force a full repaint.
    let mut canvas = CpuRenderer::new(CpuRendererSettings {
        threads,
        ..Default::default()
    });
    let (mut base, mut ours) = (Vec::new(), Vec::new());
    let mut previous: Option<Vec<u8>> = None;
    let mut previous_spike: Option<Vec<u8>> = None;
    for i in 0..=15 {
        let mut v = view;
        v.pan(Vec2::new(f64::from(i % 2), 0.0));
        v.set_scale(view.scale() * (1.0 + 1e-6 * f64::from(i)));
        // Like in a window, every frame goes into a new buffer while the
        // previous one is still shown.
        let t = Instant::now();
        let mut buf = vec![0u8; len];
        spike.render(&v, &mut buf);
        let t_spike = t.elapsed().as_secs_f64() * 1e3;
        drop(previous_spike.replace(std::hint::black_box(buf)));
        let t = Instant::now();
        let mut buf = vec![0u8; len];
        canvas
            .render_into(&scene, &v, previous.as_deref(), &mut buf)
            .unwrap();
        let t_ours = t.elapsed().as_secs_f64() * 1e3;
        assert!(canvas.stats().full_repaint);
        previous = Some(buf);
        if i > 0 {
            base.push(t_spike);
            ours.push(t_ours);
        }
    }
    let (base, ours) = (median(base), median(ours));
    println!("full view {w}x{h}: spike {base:.1} ms, canvas {ours:.1} ms");
    // 10 % allowance for measurement noise.
    assert!(
        ours <= base * 1.1,
        "canvas {ours:.1} ms, spike {base:.1} ms"
    );
}
