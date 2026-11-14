//! Stress scene benchmark: the canvas spike's vello_cpu renderer (baseline,
//! reimplemented on the same items) versus `CpuRenderer`, plus the cost of
//! interactive updates with the frame cache and of hit testing.
//!
//! `cargo bench -p foilfern-canvas --bench stress [-- --size 2161x1351 --iters 30 --threads 8]`

mod spike_baseline;

use std::time::{Duration, Instant};

use foilfern_canvas::stress::stress_items;
use foilfern_canvas::{CpuRenderer, CpuRendererSettings, Renderer, Scene, View, stress};
use kurbo::{Affine, Point, Rect, Vec2};
use spike_baseline::Spike;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn stats(mut v: Vec<f64>) -> String {
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    format!(
        "median {:6.2} ms  mean {:6.2}  min {:6.2}  max {:6.2}",
        v[v.len() / 2],
        mean,
        v[0],
        v[v.len() - 1]
    )
}

fn time(iters: usize, mut f: impl FnMut(usize)) -> Vec<f64> {
    f(0); // warm-up
    (1..=iters)
        .map(|i| {
            let t = Instant::now();
            f(i);
            ms(t.elapsed())
        })
        .collect()
}

fn main() {
    let mut sizes = vec![(1600.0, 1000.0), (2161.0, 1351.0)];
    let mut iters = 20;
    let mut threads = CpuRendererSettings::default().threads;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--size" => {
                if let Some((w, h)) = args.next().as_deref().and_then(|s| s.split_once('x')) {
                    sizes = vec![(w.parse().unwrap_or(1600.0), h.parse().unwrap_or(1000.0))];
                }
            }
            "--iters" => iters = args.next().and_then(|s| s.parse().ok()).unwrap_or(iters),
            "--threads" => threads = args.next().and_then(|s| s.parse().ok()).unwrap_or(threads),
            _ => {} // e.g. --bench from cargo
        }
    }

    let items = stress_items(100_000, 16_000, 4_000);
    let t = Instant::now();
    let mut scene = Scene::new();
    stress::add_board_layers(&mut scene);
    let ids = scene.extend(items.clone());
    println!(
        "scene: {} items, {} groups, built in {:.1} ms; {threads} render threads",
        scene.len(),
        scene.group_count(),
        ms(t.elapsed())
    );
    let bbox = scene.bounding_box().unwrap_or(Rect::ZERO);

    for (lw, lh) in sizes {
        let mut full = View::new((lw, lh), 1.0);
        // The spike's fit: 95 % of the viewport.
        full.set_scale((lw / bbox.width()).min(lh / bbox.height()) * 0.95);
        full.center_on(bbox.center());
        let center = Point::new(lw / 2.0, lh / 2.0);
        let views: Vec<(&str, View)> = [("full", 1.0), ("zoom8", 8.0), ("zoom40", 40.0)]
            .into_iter()
            .map(|(n, z)| {
                let mut v = full;
                v.zoom_at(center, z);
                (n, v)
            })
            .collect();
        let (w, h) = full.device_size();
        let len = (w * h * 4) as usize;
        println!("\n== {w}x{h} px ==");

        let t = Instant::now();
        let mut spike = Spike::build(&items, threads, w as u16, h as u16);
        println!(
            "spike build {:.1} ms, {} groups",
            ms(t.elapsed()),
            spike.groups.len()
        );
        // Like in a window, every frame goes into a new zeroed buffer
        // (`SharedPixelBuffer::new`), the previous one is passed back.
        let mut previous: Option<Vec<u8>> = None;
        let mut previous_spike: Option<Vec<u8>> = None;
        let mut frame = |r: &mut CpuRenderer, scene: &Scene, v: &View| {
            let mut buf = vec![0u8; len];
            r.render_into(scene, v, previous.as_deref(), &mut buf).ok();
            previous = Some(buf);
        };
        let mut cold = CpuRenderer::new(CpuRendererSettings {
            threads,
            frame_cache: false,
            ..Default::default()
        });
        for (name, view) in &views {
            // Interleaved, so that load changes affect both alike.
            let (mut base, mut ours) = (Vec::new(), Vec::new());
            for i in 0..=iters {
                let mut v = *view;
                v.pan(Vec2::new((i % 2) as f64, 0.0));
                let t = Instant::now();
                let mut buf = vec![0u8; len];
                spike.render(&v, &mut buf);
                let t_spike = ms(t.elapsed());
                // The previous frame stays alive (shown) in both cases.
                drop(previous_spike.replace(std::hint::black_box(buf)));
                let t = Instant::now();
                frame(&mut cold, &scene, &v);
                let t_ours = ms(t.elapsed());
                if i > 0 {
                    base.push(t_spike);
                    ours.push(t_ours);
                }
            }
            println!("spike  {name:<6} {}", stats(base));
            println!("canvas {name:<6} {}", stats(ours));
        }

        // Interactive updates with the frame cache.
        let mut warm = CpuRenderer::new(CpuRendererSettings {
            threads,
            ..Default::default()
        });
        for (name, view) in &views {
            let mut v = *view;
            frame(&mut warm, &scene, &v);
            let pan = time(iters, |i| {
                v.pan(Vec2::new(if i % 2 == 0 { 20.0 } else { -13.0 }, 7.0));
                frame(&mut warm, &scene, &v);
            });
            println!("canvas {name:<6} pan ~20px   {}", stats(pan));
        }
        let v = views[0].1;
        frame(&mut warm, &scene, &v);
        let mut k = 0;
        let edit = time(iters, |_| {
            k += 1;
            let id = ids[ids.len() - k];
            if let Some(mut it) = scene.item(id).cloned() {
                it.geometry = it.geometry.transformed(Affine::translate((0.5, 0.0)));
                scene.update(id, it);
            }
            frame(&mut warm, &scene, &v);
        });
        println!("canvas full   move 1 item {}", stats(edit));
        let select = time(iters, |i| {
            scene.clear_selection();
            scene.set_selected(ids[i * 97], true);
            frame(&mut warm, &scene, &v);
        });
        println!("canvas full   select 1    {}", stats(select));
        scene.clear_selection();
    }

    // Hit testing: 10k random points, 2 px tolerance at the full view.
    let mut seed = 0x1234_5678_9abc_def1u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let n = 10_000;
    let tol = 2.0 / 5.0;
    let t = Instant::now();
    let mut hits = 0;
    for _ in 0..n {
        let p = Point::new(
            bbox.x0 + rnd() * bbox.width(),
            bbox.y0 + rnd() * bbox.height(),
        );
        hits += usize::from(scene.hit_test(p, tol).is_some());
    }
    println!(
        "\nhit test: {:.2} us/query ({hits}/{n} hits)",
        t.elapsed().as_secs_f64() * 1e6 / f64::from(n)
    );
}
