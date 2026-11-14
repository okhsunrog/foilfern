use foilfern_canvas::kurbo::{Circle, Point, Rect};
use foilfern_canvas::{Item, SelectionMode, Style};

use crate::helpers::*;

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

#[test]
fn test_point_hits_exact_shapes() {
    let mut s = scene();
    let sq = s.insert(square(RED, 0.0, 0.0, 2.0));
    let tr = s.insert(trace(GREEN, 10.0, 0.0, 20.0, 10.0, 1.0));
    let ring = s.insert(Item::new(
        BLUE,
        Circle::new((30.0, 0.0), 3.0),
        Style::stroke(0.2),
    ));
    assert_eq!(s.hit_test(p(1.0, 1.0), 0.0), Some(sq));
    assert_eq!(s.hit_test(p(2.05, 1.0), 0.0), None);
    assert_eq!(s.hit_test(p(2.05, 1.0), 0.1), Some(sq));
    // Trace: within half the width of the center line (diagonal).
    assert_eq!(s.hit_test(p(15.0, 5.0), 0.0), Some(tr));
    assert_eq!(s.hit_test(p(15.3, 4.7), 0.0), Some(tr)); // 0.42 mm off
    assert_eq!(s.hit_test(p(15.5, 4.5), 0.0), None); // 0.71 mm off
    // Round caps.
    assert_eq!(s.hit_test(p(9.6, 0.0), 0.0), Some(tr));
    // In the bounding box but not on the trace.
    assert_eq!(s.hit_test(p(19.0, 1.0), 0.0), None);
    // Unfilled circle: only the ring is hit.
    assert_eq!(s.hit_test(p(33.05, 0.0), 0.0), Some(ring));
    assert_eq!(s.hit_test(p(30.0, 0.0), 0.0), None);
}

#[test]
fn test_topmost_wins() {
    let mut s = scene();
    let blue = s.insert(square(BLUE, 0.0, 0.0, 2.0));
    let red_z1 = s.insert(square(RED, 0.0, 0.0, 2.0).with_z(1));
    let green = s.insert(square(GREEN, 0.0, 0.0, 2.0));
    assert_eq!(s.hit_test(p(1.0, 1.0), 0.0), Some(red_z1));
    assert_eq!(s.items_at(p(1.0, 1.0), 0.0), vec![red_z1, blue, green]);
    // Same layer and z: later inserted is on top.
    let blue2 = s.insert(square(BLUE, 0.0, 0.0, 2.0));
    assert_eq!(s.items_at(p(1.0, 1.0), 0.0)[1], blue2);
    // Updating keeps the draw sequence.
    s.update(blue, square(BLUE, 0.0, 0.0, 2.5));
    assert_eq!(s.items_at(p(1.0, 1.0), 0.0)[1], blue2);
}

#[test]
fn test_hidden_layers_are_not_hit() {
    let mut s = scene();
    let red = s.insert(square(RED, 0.0, 0.0, 2.0));
    let blue = s.insert(square(BLUE, 0.0, 0.0, 2.0));
    assert_eq!(s.hit_test(p(1.0, 1.0), 0.0), Some(blue));
    s.set_layer_visible(BLUE, false);
    assert_eq!(s.hit_test(p(1.0, 1.0), 0.0), Some(red));
    assert_eq!(
        s.items_in_rect(Rect::new(-1.0, -1.0, 5.0, 5.0), SelectionMode::Contains),
        vec![red]
    );
}

#[test]
fn test_filter() {
    let mut s = scene();
    let red = s.insert(square(RED, 0.0, 0.0, 2.0));
    s.insert(square(BLUE, 0.0, 0.0, 2.0));
    assert_eq!(
        s.hit_test_with(p(1.0, 1.0), 0.0, |_, item| item.layer == RED),
        Some(red)
    );
}

#[test]
fn test_rect_selection() {
    let mut s = scene();
    let sq = s.insert(square(RED, 0.0, 0.0, 2.0));
    let tr = s.insert(trace(GREEN, -10.0, 10.0, 10.0, 10.0, 0.4));
    let ring = s.insert(Item::new(
        BLUE,
        Circle::new((30.0, 0.0), 5.0),
        Style::stroke(0.2),
    ));
    let contains = |r| s.items_in_rect(r, SelectionMode::Contains);
    let intersects = |r| s.items_in_rect(r, SelectionMode::Intersects);

    // The trace crosses the rect: intersects, not contained.
    let r = Rect::new(-1.0, 9.0, 1.0, 11.0);
    assert_eq!(intersects(r), vec![tr]);
    assert!(contains(r).is_empty());
    // Within the stroke width only.
    assert_eq!(intersects(Rect::new(0.0, 10.15, 1.0, 11.0)), vec![tr]);
    assert!(intersects(Rect::new(0.0, 10.3, 1.0, 11.0)).is_empty());
    // A rect inside a filled square intersects it.
    assert_eq!(intersects(Rect::new(0.5, 0.5, 1.0, 1.0)), vec![sq]);
    // A rect inside the ring hole does not touch the ring.
    assert!(intersects(Rect::new(29.0, -1.0, 31.0, 1.0)).is_empty());
    assert_eq!(intersects(Rect::new(34.0, -1.0, 36.0, 1.0)), vec![ring]);
    // Everything contained; rect corners in any order.
    let all = contains(Rect::new(40.0, 15.0, -15.0, -10.0));
    assert_eq!(all.len(), 3);
    // Contained needs the full painted area (stroke width included).
    assert!(contains(Rect::new(-10.0, 9.9, 10.0, 10.1)).is_empty());
}

#[test]
fn test_grab_area_and_hairline() {
    let mut s = scene();
    // No fill, no stroke: invisible but hittable inside.
    let grab = s.insert(Item::new(
        RED,
        Rect::new(0.0, 0.0, 4.0, 4.0),
        Style::default(),
    ));
    assert_eq!(s.hit_test(p(2.0, 2.0), 0.0), Some(grab));
    assert_eq!(s.group_count(), 0);
    // Hairline: hit within the tolerance only.
    let hair = s.insert(trace(GREEN, 10.0, 0.0, 20.0, 0.0, 0.0));
    assert_eq!(s.hit_test(p(15.0, 0.05), 0.0), None);
    assert_eq!(s.hit_test(p(15.0, 0.05), 0.1), Some(hair));
}

#[test]
fn test_stress_scene_queries() {
    let s = foilfern_canvas::stress::stress_scene();
    assert_eq!(s.len(), 124_001);
    let bbox = s.bounding_box().unwrap();
    assert!(bbox.width() > 299.0 && bbox.height() > 199.0);
    // Every point on the board edge hits the outline.
    assert!(s.hit_test(p(150.0, 0.0), 0.01).is_some());
    let in_rect = s.items_in_rect(
        Rect::new(100.0, 100.0, 110.0, 110.0),
        SelectionMode::Contains,
    );
    assert!(!in_rect.is_empty());
    for id in in_rect {
        let b = s.item(id).unwrap().bounding_box();
        assert!(Rect::new(100.0, 100.0, 110.0, 110.0).contains_rect(b));
    }
}
