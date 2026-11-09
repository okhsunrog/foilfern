use librepcb_canvas::kurbo::{Affine, Line, Point, Rect, Vec2};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{
    CpuRenderer, CpuRendererSettings, Item, Renderer, Scene, StrokeStyle, Style, View,
};

use crate::helpers::*;

fn cold() -> CpuRenderer {
    CpuRenderer::new(CpuRendererSettings {
        frame_cache: false,
        threads: 0,
        ..Default::default()
    })
}

fn warm() -> CpuRenderer {
    CpuRenderer::new(CpuRendererSettings {
        threads: 2,
        ..Default::default()
    })
}

fn pixel(frame: &[u8], view: &View, x: u32, y: u32) -> [u8; 4] {
    let w = view.device_size().0;
    let i = ((y * w + x) * 4) as usize;
    [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
}

/// Pixel at a world position.
fn at(frame: &[u8], view: &View, x: f64, y: f64) -> [u8; 4] {
    let p = view.device_transform() * Point::new(x, y);
    pixel(frame, view, p.x as u32, p.y as u32)
}

/// A 200 × 100 px view showing (0, 0)..(20, 10) mm at 10 px/mm.
fn view() -> View {
    let mut v = View::new((200.0, 100.0), 1.0);
    v.fit(Rect::new(0.0, 0.0, 20.0, 10.0), 0.0);
    v
}

const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED_PX: [u8; 4] = [255, 0, 0, 255];
const GREEN_PX: [u8; 4] = [0, 255, 0, 255];
const BLUE_PX: [u8; 4] = [0, 0, 255, 255];

#[test]
fn test_basic_rendering_y_up() {
    let mut s = scene();
    s.insert(square(RED, 2.0, 6.0, 2.0)); // top left on screen
    s.insert(dot(BLUE, 15.0, 2.0, 1.0)); // bottom right
    let v = view();
    let mut r = cold();
    let f = r.render(&s, &v).unwrap().to_vec();
    assert_eq!(f.len(), 200 * 100 * 4);
    assert_eq!(at(&f, &v, 3.0, 7.0), RED_PX);
    assert_eq!(pixel(&f, &v, 30, 30), RED_PX);
    assert_eq!(at(&f, &v, 15.0, 2.0), BLUE_PX);
    assert_eq!(pixel(&f, &v, 150, 80), BLUE_PX);
    assert_eq!(at(&f, &v, 10.0, 5.0), BLACK);
}

#[test]
fn test_strokes_hairlines_dashes_and_dots() {
    let mut s = scene();
    s.insert(trace(RED, 1.0, 8.0, 19.0, 8.0, 1.0));
    s.insert(trace(GREEN, 1.0, 5.0, 19.0, 5.0, 0.0)); // hairline
    s.insert(Item::new(
        BLUE,
        Line::new((1.0, 2.0), (19.0, 2.0)),
        Style {
            fill: None,
            stroke: Some(StrokeStyle::new(0.6).with_dash([2.0, 2.0], 0.0)),
        },
    ));
    // Zero-length round-capped stroke: a dot.
    s.insert(trace(GREEN, 10.0, 0.8, 10.0, 0.8, 1.0));
    let v = view();
    let mut r = cold();
    let f = r.render(&s, &v).unwrap().to_vec();
    assert_eq!(at(&f, &v, 10.0, 8.3), RED_PX);
    assert_eq!(at(&f, &v, 10.0, 8.7), BLACK);
    // The hairline covers about one pixel row.
    let y = (v.device_transform() * Point::new(0.0, 5.0)).y;
    let rows: Vec<u8> = (y as u32 - 2..=y as u32 + 1)
        .map(|py| pixel(&f, &v, 100, py)[1])
        .collect();
    assert!(rows.iter().any(|g| *g > 100), "{rows:?}");
    assert!(rows.iter().filter(|g| **g > 0).count() <= 2, "{rows:?}");
    // Dashes: on at 2, off at 4, on at 6 mm from the start.
    assert_eq!(at(&f, &v, 2.0, 2.0), BLUE_PX);
    assert_eq!(at(&f, &v, 4.0, 2.0), BLACK);
    assert_eq!(at(&f, &v, 6.0, 2.0), BLUE_PX);
    assert_eq!(at(&f, &v, 10.0, 0.8), GREEN_PX);
}

#[test]
fn test_layer_changes_repaint_without_encoding() {
    let mut s = scene();
    s.insert(square(RED, 2.0, 2.0, 4.0));
    let v = view();
    let mut r = warm();
    r.render(&s, &v).unwrap();
    assert!(r.stats().encoded_groups > 0);
    s.set_layer_color(RED, Color::from_rgb8(0, 255, 255));
    let f = r.render(&s, &v).unwrap().to_vec();
    assert_eq!(at(&f, &v, 4.0, 4.0), [0, 255, 255, 255]);
    assert_eq!(r.stats().encoded_groups, 0);
    assert!(r.stats().full_repaint);
    s.set_layer_visible(RED, false);
    let f = r.render(&s, &v).unwrap().to_vec();
    assert_eq!(at(&f, &v, 4.0, 4.0), BLACK);
}

#[test]
fn test_selection_draws_overlay_only() {
    let mut s = scene();
    let a = s.insert(square(RED, 2.0, 2.0, 4.0));
    let v = view();
    let mut r = warm();
    r.render(&s, &v).unwrap();
    s.set_selected(a, true);
    let f = r.render(&s, &v).unwrap().to_vec();
    let stats = r.stats();
    assert!(!stats.full_repaint);
    assert!(stats.regions.is_empty());
    assert_eq!(stats.overlay_groups, 1);
    // Layer::new() highlights with a lighter color.
    assert_eq!(at(&f, &v, 4.0, 4.0), [255, 128, 128, 255]);
    s.clear_selection();
    let f = r.render(&s, &v).unwrap().to_vec();
    assert_eq!(at(&f, &v, 4.0, 4.0), RED_PX);
}

#[test]
fn test_partial_updates_match_full_render() {
    let mut s = scene();
    let ids = populate(&mut s);
    s.insert(trace(GREEN, -5.0, -5.0, 80.0, 20.0, 0.0));
    let mut v = View::new((300.0, 180.0), 1.25);
    v.fit(s.bounding_box().unwrap(), 5.0);
    v.zoom_at(Point::new(150.0, 90.0), 1.7);
    let mut w = warm();
    let mut c = cold();
    let check = |w: &mut CpuRenderer, c: &mut CpuRenderer, s: &Scene, v: &View, what: &str| {
        let a = w.render(s, v).unwrap().to_vec();
        let b = c.render(s, v).unwrap().to_vec();
        assert_eq!(a.len(), b.len());
        let diff = a
            .iter()
            .zip(&b)
            .filter(|(x, y)| x.abs_diff(**y) > 2)
            .count();
        assert!(diff <= 8, "{what}: {diff} bytes differ");
    };
    check(&mut w, &mut c, &s, &v, "initial");
    for (i, d) in [(13.0, 0.0), (0.0, -7.0), (-21.0, 17.0), (3.4, 2.2)]
        .into_iter()
        .enumerate()
    {
        v.pan(Vec2::new(d.0, d.1));
        check(&mut w, &mut c, &s, &v, &format!("pan {i}"));
        assert!(!w.stats().full_repaint || i == 3);
    }
    let mut it = s.item(ids[4]).cloned().unwrap();
    it.geometry = it.geometry.transformed(Affine::translate((1.3, -0.7)));
    s.update(ids[4], it);
    check(&mut w, &mut c, &s, &v, "update");
    assert!(!w.stats().full_repaint);
    assert!(w.stats().encoded_groups <= 2);
    s.remove(ids[7]);
    s.insert(dot(RED, 20.0, 8.0, 2.0));
    s.set_selected(ids[1], true);
    check(&mut w, &mut c, &s, &v, "remove/insert/select");
    v.zoom_at(Point::new(10.0, 10.0), 1.3);
    check(&mut w, &mut c, &s, &v, "zoom");
    assert!(w.stats().full_repaint);
}

#[test]
fn test_render_into_reuses_previous_target() {
    let mut s = scene();
    let ids = populate(&mut s);
    let mut v = View::new((240.0, 160.0), 1.0);
    v.fit(s.bounding_box().unwrap(), 5.0);
    let (w, h) = v.device_size();
    let mut r = warm();
    let mut c = cold();
    let mut previous: Option<Vec<u8>> = None;
    let mut frame = |r: &mut CpuRenderer, s: &Scene, v: &View| {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        r.render_into(s, v, previous.as_deref(), &mut buf).unwrap();
        previous = Some(buf.clone());
        buf
    };
    type Step<'a> = Box<dyn Fn(&mut Scene, &mut View) + 'a>;
    let steps: Vec<Step> = vec![
        Box::new(|_, _| {}),
        Box::new(|_, v| v.pan(Vec2::new(9.0, -4.0))),
        Box::new(|s, _| s.set_selected(ids[2], true)),
        Box::new(|_, v| v.pan(Vec2::new(-5.0, 3.0))),
        Box::new(|s, _| {
            s.update(ids[5], square(RED, 30.0, 8.0, 2.0));
        }),
        Box::new(|s, _| s.clear_selection()),
        Box::new(|_, v| v.pan(Vec2::new(0.0, 11.0))),
    ];
    for (i, step) in steps.iter().enumerate() {
        step(&mut s, &mut v);
        let a = frame(&mut r, &s, &v);
        if i > 0 {
            assert!(!r.stats().full_repaint, "step {i}");
        }
        let b = c.render(&s, &v).unwrap();
        let diff = a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 2).count();
        assert!(diff <= 8, "step {i}: {diff} bytes differ");
    }
    // Without the previous target, the renderer repaints everything.
    let mut buf = vec![0u8; (w * h * 4) as usize];
    v.pan(Vec2::new(1.0, 0.0));
    r.render_into(&s, &v, None, &mut buf).unwrap();
    assert!(r.stats().full_repaint);
    // Wrong target sizes are errors.
    assert!(r.render_into(&s, &v, None, &mut buf[4..]).is_err());
}

#[test]
fn test_mirrored_view_and_device_pixel_ratio() {
    let mut s = scene();
    s.insert(square(RED, 2.0, 2.0, 2.0)); // left
    let mut v = view();
    v.set_device_pixel_ratio(2.0);
    assert_eq!(v.device_size(), (400, 200));
    let mut r = cold();
    let f = r.render(&s, &v).unwrap().to_vec();
    assert_eq!(pixel(&f, &v, 60, 140), RED_PX);
    v.set_mirrored(true);
    let f = r.render(&s, &v).unwrap().to_vec();
    assert_eq!(pixel(&f, &v, 60, 140), BLACK);
    assert_eq!(pixel(&f, &v, 400 - 60, 140), RED_PX);
    assert_eq!(at(&f, &v, 3.0, 3.0), RED_PX);
}

#[test]
fn test_empty_and_tiny_views() {
    let s = Scene::new();
    let mut r = warm();
    let v = View::new((0.0, 0.0), 1.0);
    assert!(r.render(&s, &v).unwrap().is_empty());
    let v = View::new((1.0, 1.0), 1.0);
    assert_eq!(r.render(&s, &v).unwrap(), &BLACK[..]);
    let v = View::new((70_000.0, 1.0), 1.0);
    assert!(r.render(&s, &v).is_err());
}

#[test]
fn test_grid_below_items() {
    use librepcb_canvas::{Grid, GridStyle};
    let mut s = scene();
    s.insert(square(RED, 0.0, 0.0, 10.0)); // covers the left half
    let v = view(); // 10 px/mm
    let mut r = cold();
    r.set_grid(Some(Grid {
        interval: 1.0,
        style: GridStyle::Dots,
        color: Color::WHITE,
    }));
    let f = r.render(&s, &v).unwrap().to_vec();
    // Minor dots on the right half are visible, points in between are
    // background.
    let dot = at(&f, &v, 15.0, 5.0);
    assert!(dot[0] > 100 && dot[0] == dot[1], "{dot:?}");
    assert_eq!(at(&f, &v, 15.5, 5.5), BLACK);
    // Covered by the square.
    assert_eq!(at(&f, &v, 5.0, 5.0), RED_PX);

    // Too dense at 10 px/mm with 0.1 mm: drawn every 1 mm instead.
    r.set_grid(Some(Grid {
        interval: 0.1,
        style: GridStyle::Lines,
        color: Color::WHITE,
    }));
    let f = r.render(&s, &v).unwrap().to_vec();
    assert_ne!(at(&f, &v, 15.0, 5.5), BLACK);
    assert_eq!(at(&f, &v, 15.5, 5.5), BLACK);
}
