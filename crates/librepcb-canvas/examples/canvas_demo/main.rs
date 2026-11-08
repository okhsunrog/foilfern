//! Canvas demo: the spike's 124k items stress board plus real LibrePCB
//! geometry and stroke texts, in a Slint window or rendered to PNGs.
//!
//! ```sh
//! cargo run --release -p librepcb-canvas --features slint --example canvas_demo
//! # Without the stress board:
//! cargo run ... --example canvas_demo -- --no-stress
//! # Headless PNGs (out/demo-full.png, out/demo-zoom8.png, ...):
//! cargo run ... --example canvas_demo -- --png out/demo.png [--size 1600x1000]
//! ```
//!
//! Window: middle or right drag pans, the wheel zooms (Shift/Ctrl scroll),
//! left click selects the topmost item, left drag selects by rectangle.
//! Keys: `f` fit, `1`-`4` toggle copper layers, `c` recolor top copper,
//! `m` mirror, `Delete` removes the selection.

mod demo_scene;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use demo_scene::Demo;
use librepcb_canvas::kurbo::{Point, Rect, Vec2};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::slint_adapter::{ImageRenderer, sync_view_size};
use librepcb_canvas::stress::board_layer;
use librepcb_canvas::{
    CpuRenderer, Item, ItemId, Layer, LayerId, Modifiers, Navigator, PointerAction, PointerButton,
    PointerKind, Renderer, SelectionMode, StrokeStyle, Style, View,
};
use slint::ComponentHandle;
use slint::platform::PointerEventButton;

slint::slint! {
    #[edition = "1.20"]
    export component DemoWindow inherits Window {
        preferred-width: 1600px;
        preferred-height: 1000px;
        title: "LibrePCB canvas demo";
        background: black;

        in property <image> frame;
        in property <string> status;

        callback resized(length, length);
        callback pointer-down(PointerEventButton, length, length);
        callback pointer-up(PointerEventButton, length, length);
        callback pointer-moved(length, length);
        callback scrolled(length, length, length, length, bool, bool);
        callback key(string);

        changed width => { root.resized(self.width, self.height); }
        changed height => { root.resized(self.width, self.height); }

        // The focus scope wraps the canvas, so that it does not cover the
        // touch area.
        fs := FocusScope {
            key-pressed(ev) => {
                root.key(ev.text);
                accept
            }

            Image {
                width: 100%;
                height: 100%;
                source: root.frame;
                image-fit: fill;
            }

            TouchArea {
                pointer-event(ev) => {
                    if ev.kind == PointerEventKind.down {
                        root.pointer-down(ev.button, self.mouse-x, self.mouse-y);
                    }
                    if ev.kind == PointerEventKind.up || ev.kind == PointerEventKind.cancel {
                        root.pointer-up(ev.button, self.mouse-x, self.mouse-y);
                    }
                }
                moved => { root.pointer-moved(self.mouse-x, self.mouse-y); }
                scroll-event(ev) => {
                    root.scrolled(self.mouse-x, self.mouse-y, ev.delta-x, ev.delta-y,
                        ev.modifiers.shift, ev.modifiers.control);
                    accept
                }
            }
        }
        init => { fs.focus(); }

        Rectangle {
            x: 0; y: 0;
            width: status-text.preferred-width + 16px;
            height: status-text.preferred-height + 8px;
            background: #000000c0;
            status-text := Text {
                x: 8px; y: 4px;
                text: root.status;
                color: #ffff80;
                font-size: 13px;
            }
        }
    }
}

/// Layer of the rubber band rectangle (on top of everything).
const RUBBER_BAND: LayerId = LayerId(1000);

struct App {
    demo: Demo,
    view: View,
    nav: Navigator,
    images: ImageRenderer<CpuRenderer>,
    left_down: Option<Point>,
    rubber_band: Option<ItemId>,
    message: String,
    fitted: bool,
    redraw_pending: bool,
}

impl App {
    fn fit(&mut self) {
        if let Some(bbox) = self.demo.scene.bounding_box() {
            self.view.fit(bbox, 20.0);
        }
    }

    fn draw(&mut self, ui: &DemoWindow) {
        // The canvas fills the window.
        let sf = ui.window().scale_factor();
        let size = ui.window().size().to_logical(sf);
        sync_view_size(&mut self.view, size.width, size.height, sf);
        if !self.fitted && size.width > 1.0 && size.height > 1.0 {
            self.fitted = true;
            self.fit();
        }
        let t = Instant::now();
        match self.images.render(&self.demo.scene, &self.view) {
            Ok(img) => ui.set_frame(img),
            Err(e) => eprintln!("render failed: {e}"),
        }
        let s = self.images.renderer().stats();
        let (w, h) = self.view.device_size();
        ui.set_status(
            format!(
                "{} items, {} groups | {w}x{h} px | {:.2} px/mm | frame {:.1} ms ({}, {} regions, {} encoded, {} drawn, {} overlay) | {}",
                self.demo.scene.len(),
                self.demo.scene.group_count(),
                self.view.scale(),
                t.elapsed().as_secs_f64() * 1e3,
                if s.full_repaint { "full" } else { "partial" },
                s.regions.len(),
                s.encoded_groups,
                s.drawn_groups,
                s.overlay_groups,
                self.message
            )
            .into(),
        );
    }

    fn select_at(&mut self, world: Point) {
        let scene = &mut self.demo.scene;
        scene.clear_selection();
        let t = Instant::now();
        let hit = scene.hit_test(world, self.view.pixels_to_world(3.0));
        let us = t.elapsed().as_secs_f64() * 1e6;
        self.message = match hit {
            Some(id) => {
                scene.set_selected(id, true);
                let label = self.demo.labels.get(&id).map_or("?", String::as_str);
                format!("hit {label} ({us:.1} µs)")
            }
            None => format!("({:.3}, {:.3}) mm: nothing ({us:.1} µs)", world.x, world.y),
        };
        println!("{}", self.message);
    }

    fn update_rubber_band(&mut self, from: Point, to: Point) {
        let a = self.view.screen_to_world(from);
        let b = self.view.screen_to_world(to);
        let item = Item::new(
            RUBBER_BAND,
            Rect::from_points(a, b),
            Style {
                fill: Some(librepcb_canvas::Brush::Solid(Color::from_rgba8(
                    0x80, 0xc0, 0xff, 0x30,
                ))),
                stroke: Some(StrokeStyle::hairline()),
            },
        )
        .with_z(i32::MAX);
        match self.rubber_band {
            Some(id) => {
                self.demo.scene.update(id, item);
            }
            None => self.rubber_band = Some(self.demo.scene.insert(item)),
        }
    }

    fn finish_rubber_band(&mut self) {
        let Some(id) = self.rubber_band.take() else {
            return;
        };
        let Some(band) = self.demo.scene.remove(id) else {
            return;
        };
        let rect = band.geometry.bounding_box();
        let scene = &mut self.demo.scene;
        scene.clear_selection();
        let ids = scene.items_in_rect(rect, SelectionMode::Intersects);
        for id in &ids {
            scene.set_selected(*id, true);
        }
        self.message = format!("{} items selected", ids.len());
        println!("{}", self.message);
    }

    fn key(&mut self, text: &str) {
        let scene = &mut self.demo.scene;
        let toggle = |scene: &mut librepcb_canvas::Scene, name: &str| {
            if let Some(id) = board_layer(name) {
                let visible = scene.is_layer_visible(id);
                scene.set_layer_visible(id, !visible);
            }
        };
        match text {
            "f" => self.fit(),
            "1" => toggle(scene, "top_cu"),
            "2" => toggle(scene, "bot_cu"),
            "3" => toggle(scene, "in1_cu"),
            "4" => toggle(scene, "in2_cu"),
            "m" => self.view.set_mirrored(!self.view.is_mirrored()),
            "c" => {
                if let Some(id) = board_layer("top_cu") {
                    let old = scene.layer(id).map(|l| l.color);
                    let new = if old == Some(Color::from_rgba8(0x20, 0xa0, 0xff, 0x96)) {
                        librepcb_canvas::stress::argb(0x96cc0802)
                    } else {
                        Color::from_rgba8(0x20, 0xa0, 0xff, 0x96)
                    };
                    scene.set_layer_color(id, new);
                }
            }
            "\u{7f}" => {
                let ids: Vec<ItemId> = scene.selection().collect();
                for id in &ids {
                    scene.remove(*id);
                }
                self.message = format!("removed {} items", ids.len());
            }
            _ => {}
        }
    }
}

fn request_redraw(app: &Rc<RefCell<App>>, ui: &slint::Weak<DemoWindow>) {
    if std::mem::replace(&mut app.borrow_mut().redraw_pending, true) {
        return;
    }
    let (app, ui) = (app.clone(), ui.clone());
    // Coalesce bursts of events into one frame.
    slint::Timer::single_shot(Duration::ZERO, move || {
        let mut app = app.borrow_mut();
        app.redraw_pending = false;
        if let Some(ui) = ui.upgrade() {
            app.draw(&ui);
        }
    });
}

fn run_window(demo: Demo) -> Result<(), slint::PlatformError> {
    let ui = DemoWindow::new()?;
    let mut demo = demo;
    demo.scene.set_layer(
        RUBBER_BAND,
        Layer::new(Color::from_rgba8(0x80, 0xc0, 0xff, 0xff)),
    );
    let app = Rc::new(RefCell::new(App {
        demo,
        view: View::new((1.0, 1.0), 1.0),
        nav: Navigator::new(),
        images: ImageRenderer::default(),
        left_down: None,
        rubber_band: None,
        message: "click an item".into(),
        fitted: false,
        redraw_pending: false,
    }));
    let weak = ui.as_weak();

    ui.on_resized({
        let (app, weak) = (app.clone(), weak.clone());
        // `draw()` picks up the new size.
        move |_, _| request_redraw(&app, &weak)
    });

    let pointer = {
        let (app, weak) = (app.clone(), weak.clone());
        move |kind: PointerKind, button: PointerButton, x: f32, y: f32| {
            let pos = Point::new(f64::from(x), f64::from(y));
            let mut guard = app.borrow_mut();
            let a = &mut *guard;
            let action = a.nav.pointer_event(&mut a.view, kind, button, pos);
            let redraw = match action {
                PointerAction::ViewChanged => true,
                PointerAction::LeftPressed(_) => {
                    a.left_down = Some(pos);
                    false
                }
                PointerAction::Moved(_) => match a.left_down {
                    Some(start) if a.rubber_band.is_some() || (pos - start).hypot() > 4.0 => {
                        a.update_rubber_band(start, pos);
                        true
                    }
                    _ => false,
                },
                PointerAction::LeftReleased(world) => {
                    a.left_down = None;
                    if a.rubber_band.is_some() {
                        a.finish_rubber_band();
                    } else {
                        a.select_at(world);
                    }
                    true
                }
                PointerAction::ContextMenu(world) => {
                    println!("context menu at ({:.3}, {:.3}) mm", world.x, world.y);
                    false
                }
                PointerAction::None => false,
            };
            drop(guard);
            if redraw {
                request_redraw(&app, &weak);
            }
        }
    };
    ui.on_pointer_down({
        let pointer = pointer.clone();
        move |b: PointerEventButton, x, y| pointer(PointerKind::Down, b.into(), x, y)
    });
    ui.on_pointer_up({
        let pointer = pointer.clone();
        move |b: PointerEventButton, x, y| pointer(PointerKind::Up, b.into(), x, y)
    });
    ui.on_pointer_moved(move |x, y| pointer(PointerKind::Move, PointerButton::Other, x, y));

    ui.on_scrolled({
        let (app, weak) = (app.clone(), weak.clone());
        move |x, y, dx, dy, shift, control| {
            let changed = {
                let mut a = app.borrow_mut();
                let a = &mut *a;
                let modifiers = Modifiers {
                    shift,
                    control,
                    ..Default::default()
                };
                a.nav.scroll_event(
                    &mut a.view,
                    Point::new(f64::from(x), f64::from(y)),
                    Vec2::new(f64::from(dx), f64::from(dy)),
                    modifiers,
                )
            };
            if changed {
                request_redraw(&app, &weak);
            }
        }
    });

    ui.on_key({
        let (app, weak) = (app.clone(), weak.clone());
        move |text| {
            app.borrow_mut().key(text.as_str());
            request_redraw(&app, &weak);
        }
    });

    request_redraw(&app, &weak);
    ui.run()
}

// ---------------------------------------------------------------------------
// Headless PNG mode
// ---------------------------------------------------------------------------

fn save_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn run_headless(
    mut demo: Demo,
    png: &Path,
    size: (f64, f64),
) -> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = CpuRenderer::default();
    let bbox = demo
        .scene
        .bounding_box()
        .unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
    let mut full = View::new(size, 1.0);
    full.fit(bbox, 20.0);
    let center = Point::new(size.0 / 2.0, size.1 / 2.0);
    let zoom = |z: f64| {
        let mut v = full;
        v.zoom_at(center, z);
        v
    };
    let mut real = View::new(size, 1.0);
    real.fit(
        Rect::from_center_size(demo_scene::real_geometry_center(), (125.0, 95.0)),
        10.0,
    );
    let mut detail = real;
    detail.fit(
        Rect::from_center_size(Point::new(412.0, 70.0), (40.0, 25.0)),
        10.0,
    );
    let mut mirrored = real;
    mirrored.set_mirrored(true);
    let mut hidpi = detail;
    hidpi.set_device_pixel_ratio(2.0);

    let stem = png
        .file_stem()
        .map_or("canvas".into(), |s| s.to_string_lossy().into_owned());
    let dir = png.parent().map(Path::to_path_buf).unwrap_or_default();
    let write = |name: &str, view: &View, renderer: &mut CpuRenderer, demo: &Demo| {
        let t = Instant::now();
        let result = renderer.render(&demo.scene, view).map(<[u8]>::to_vec);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        match result {
            Ok(rgba) => {
                let (w, h) = view.device_size();
                let out: PathBuf = dir.join(format!("{stem}-{name}.png"));
                let s = renderer.stats();
                println!(
                    "{name:<9} {w}x{h} {ms:7.1} ms ({}, {} encoded, {} drawn) -> {}",
                    if s.full_repaint { "full" } else { "partial" },
                    s.encoded_groups,
                    s.drawn_groups,
                    out.display()
                );
                if let Err(e) = save_png(&out, w, h, &rgba) {
                    eprintln!("writing {} failed: {e}", out.display());
                }
            }
            Err(e) => eprintln!("{name}: {e}"),
        }
    };
    write("full", &full, &mut renderer, &demo);
    write("zoom8", &zoom(8.0), &mut renderer, &demo);
    write("zoom40", &zoom(40.0), &mut renderer, &demo);
    write("real", &real, &mut renderer, &demo);
    write("detail", &detail, &mut renderer, &demo);
    write("hidpi", &hidpi, &mut renderer, &demo);
    write("mirrored", &mirrored, &mut renderer, &demo);

    // Selection: click on the footprint pads and a text, then a rectangle.
    for p in [
        Point::new(401.0, 58.5),
        Point::new(330.0, 77.0),
        Point::new(335.0, 57.0),
    ] {
        let hits = demo.scene.items_at(p, real.pixels_to_world(3.0));
        if let Some(id) = hits.first() {
            demo.scene.set_selected(*id, true);
            let label = demo.labels.get(id).map_or("?", String::as_str);
            println!("hit ({:.1}, {:.1}) -> {label}", p.x, p.y);
        }
    }
    let in_rect = demo.scene.items_in_rect(
        Rect::new(400.0, 70.0, 420.0, 90.0),
        SelectionMode::Intersects,
    );
    println!("rect selection: {} items", in_rect.len());
    for id in in_rect {
        demo.scene.set_selected(id, true);
    }
    write("selected", &real, &mut renderer, &demo);
    let mut panned = real;
    panned.pan(Vec2::new(-150.0, 40.0));
    write("panned", &panned, &mut renderer, &demo);
    if let Some(cu) = board_layer("top_cu") {
        demo.scene.set_layer_visible(cu, false);
    }
    write("no-top-cu", &panned, &mut renderer, &demo);
    Ok(())
}

fn main() {
    let mut png = None;
    let mut size = (1600.0, 1000.0);
    let mut stress = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--png" => png = args.next().map(PathBuf::from),
            "--no-stress" => stress = false,
            "--size" => {
                if let Some((w, h)) = args.next().as_deref().and_then(|s| s.split_once('x'))
                    && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
                {
                    size = (w, h);
                }
            }
            other => eprintln!("ignoring argument {other}"),
        }
    }
    let t = Instant::now();
    let demo = demo_scene::build(stress);
    println!(
        "scene: {} items, {} groups, built in {:.1} ms",
        demo.scene.len(),
        demo.scene.group_count(),
        t.elapsed().as_secs_f64() * 1e3
    );
    let result = match png {
        Some(png) => run_headless(demo, &png, size),
        None => run_window(demo).map_err(Into::into),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
