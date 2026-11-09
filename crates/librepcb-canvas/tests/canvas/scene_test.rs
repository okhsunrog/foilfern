use std::collections::HashMap;

use librepcb_canvas::kurbo::{Affine, BezPath, Rect, Shape};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{Damage, GroupId, Item, Layer, LayerId, Scene, StrokeStyle, Style};

use crate::helpers::*;

fn group_revs(s: &Scene) -> HashMap<GroupId, u64> {
    s.draw_groups().map(|g| (g.id(), g.rev())).collect()
}

#[test]
fn test_insert_update_remove() {
    let mut s = scene();
    assert!(s.is_empty());
    assert_eq!(s.bounding_box(), None);
    let a = s.insert(square(RED, 0.0, 0.0, 2.0));
    let b = s.insert(trace(GREEN, 0.0, 0.0, 10.0, 0.0, 1.0));
    assert_eq!(s.len(), 2);
    // The stroke extends the bounding box by half its width.
    assert_eq!(s.bounding_box(), Some(Rect::new(-0.5, -0.5, 10.5, 2.0)));

    let moved = square(RED, 20.0, 20.0, 2.0);
    assert!(s.update(a, moved.clone()));
    assert_eq!(s.item(a), Some(&moved));
    assert_eq!(s.bounding_box(), Some(Rect::new(-0.5, -0.5, 22.0, 22.0)));

    assert_eq!(s.remove(b).map(|i| i.layer), Some(GREEN));
    assert_eq!(s.remove(b), None);
    assert!(!s.update(b, moved));
    assert_eq!(s.len(), 1);
    assert_eq!(s.bounding_box(), Some(Rect::new(20.0, 20.0, 22.0, 22.0)));
    s.clear();
    assert!(s.is_empty());
    assert_eq!(s.group_count(), 0);
}

#[test]
fn test_grouping_by_z_layer_pass_and_tile() {
    let mut s = scene(); // 10 mm tiles
    s.insert(square(RED, 1.0, 1.0, 1.0));
    s.insert(square(RED, 3.0, 1.0, 1.0)); // same tile, layer, pass
    assert_eq!(s.group_count(), 1);
    s.insert(square(RED, 15.0, 1.0, 1.0)); // other tile
    assert_eq!(s.group_count(), 2);
    s.insert(square(GREEN, 1.0, 1.0, 1.0)); // other layer
    assert_eq!(s.group_count(), 3);
    s.insert(square(RED, 1.0, 1.0, 1.0).with_z(1)); // other z
    assert_eq!(s.group_count(), 4);
    s.insert(trace(RED, 1.0, 1.0, 2.0, 2.0, 0.2)); // stroke pass
    s.insert(trace(RED, 1.0, 1.0, 2.0, 2.0, 0.3)); // other width
    assert_eq!(s.group_count(), 6);
    // Fill and stroke: two groups for one item.
    s.insert(Item::new(
        GREEN,
        Rect::new(1.0, 1.0, 2.0, 2.0),
        Style::fill_and_stroke(0.2),
    ));
    assert_eq!(s.group_count(), 7);
    let solid = Style::fill().with_fill(Some(librepcb_canvas::Brush::Solid(Color::WHITE)));
    s.insert(Item::new(GREEN, Rect::new(1.0, 1.0, 2.0, 2.0), solid));
    assert_eq!(s.group_count(), 8);
}

#[test]
fn test_update_only_touches_affected_groups() {
    let mut s = scene();
    let ids = populate(&mut s);
    let before = group_revs(&s);
    let groups = s.group_count();
    // Move one square within its tile.
    let mut it = s.item(ids[3]).cloned().unwrap();
    it.geometry = it.geometry.transformed(Affine::translate((0.5, 0.0)));
    s.update(ids[3], it);
    let after = group_revs(&s);
    assert_eq!(s.group_count(), groups);
    let changed: Vec<_> = after
        .iter()
        .filter(|(g, r)| before.get(g) != Some(r))
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
}

#[test]
fn test_empty_groups_are_removed() {
    let mut s = scene();
    let a = s.insert(square(RED, 1.0, 1.0, 1.0));
    let b = s.insert(square(RED, 51.0, 1.0, 1.0));
    assert_eq!(s.group_count(), 2);
    s.remove(a);
    assert_eq!(s.group_count(), 1);
    // Moving an item to another tile moves it to another group.
    s.update(b, square(RED, 1.0, 1.0, 1.0));
    assert_eq!(s.group_count(), 1);
    let g = s.draw_groups().next().unwrap();
    assert_eq!(g.bounding_box(), Rect::new(1.0, 1.0, 2.0, 2.0));
    let mut path = BezPath::new();
    g.encode(&mut path);
    assert_eq!(path.elements().len(), 5); // move, 3 lines, close
}

#[test]
fn test_group_bbox_shrinks_when_border_item_is_removed() {
    let mut s = scene();
    let a = s.insert(square(RED, 1.0, 1.0, 1.0));
    let inner = s.insert(square(RED, 3.0, 3.0, 1.0));
    s.insert(square(RED, 5.0, 5.0, 1.0));
    let bbox = |s: &Scene| s.draw_groups().next().unwrap().bounding_box();
    assert_eq!(bbox(&s), Rect::new(1.0, 1.0, 6.0, 6.0));
    // An item strictly inside does not change the bounding box.
    s.remove(inner);
    assert_eq!(bbox(&s), Rect::new(1.0, 1.0, 6.0, 6.0));
    // An item at the border does.
    s.remove(a);
    assert_eq!(bbox(&s), Rect::new(5.0, 5.0, 6.0, 6.0));
}

#[test]
fn test_group_encoding_contains_all_items() {
    let mut s = scene();
    for i in 0..5 {
        s.insert(trace(GREEN, 0.0, f64::from(i), 5.0, f64::from(i), 0.2));
    }
    let g = s.draw_groups().next().unwrap();
    assert_eq!(g.len(), 5);
    let mut p = BezPath::new();
    g.encode(&mut p);
    assert_eq!(p.elements().len(), 10);
}

#[test]
fn test_draw_order() {
    let mut s = scene();
    s.insert(square(BLUE, 0.0, 0.0, 1.0));
    s.insert(square(GREEN, 0.0, 0.0, 1.0).with_z(1));
    s.insert(square(RED, 0.0, 0.0, 1.0));
    let layers: Vec<LayerId> = s.draw_groups().map(|g| g.layer()).collect();
    assert_eq!(layers, vec![RED, BLUE, GREEN]);
    // Reordering layers resorts.
    s.set_layer(RED, s.layer(RED).cloned().unwrap().with_order(10));
    let layers: Vec<LayerId> = s.draw_groups().map(|g| g.layer()).collect();
    assert_eq!(layers, vec![BLUE, RED, GREEN]);
    // Hidden layers are not drawn.
    s.set_layer_visible(RED, false);
    let layers: Vec<LayerId> = s.draw_groups().map(|g| g.layer()).collect();
    assert_eq!(layers, vec![BLUE, GREEN]);
}

#[test]
fn test_damage_tracking() {
    let mut s = scene();
    let a = s.insert(square(RED, 0.0, 0.0, 1.0));
    let rev = s.rev();
    assert_eq!(s.damage_since(rev), Damage::None);

    s.update(a, square(RED, 5.0, 5.0, 1.0));
    let Damage::Rects(rects) = s.damage_since(rev) else {
        panic!("expected rects");
    };
    // Old and new position.
    assert!(rects.contains(&Rect::new(0.0, 0.0, 1.0, 1.0)));
    assert!(rects.contains(&Rect::new(5.0, 5.0, 6.0, 6.0)));

    // Selection changes don't damage the base image.
    let rev = s.rev();
    let sel_rev = s.selection_rev();
    s.set_selected(a, true);
    assert!(s.is_selected(a));
    assert_eq!(s.damage_since(rev), Damage::None);
    assert_ne!(s.selection_rev(), sel_rev);

    // Layer color and visibility changes repaint everything.
    let rev = s.rev();
    s.set_layer_color(GREEN, Color::WHITE);
    assert_eq!(s.damage_since(rev), Damage::All);
    // Highlight color changes only affect the overlay.
    let rev = s.rev();
    let sel_rev = s.selection_rev();
    let layer = s.layer(RED).cloned().unwrap();
    s.set_layer(RED, layer.with_highlight_color(Color::WHITE));
    assert_eq!(s.damage_since(rev), Damage::None);
    assert_ne!(s.selection_rev(), sel_rev);

    // Removing a selected item deselects it.
    s.remove(a);
    assert_eq!(s.selection().count(), 0);
}

#[test]
fn test_damage_log_overflow() {
    let mut s = scene();
    let a = s.insert(square(RED, 0.0, 0.0, 1.0));
    let old = s.rev();
    for i in 0..10_000 {
        s.update(a, square(RED, f64::from(i % 7), 0.0, 1.0));
    }
    assert_eq!(s.damage_since(old), Damage::All);
    let recent = s.rev();
    s.update(a, square(RED, 3.0, 0.0, 1.0));
    assert!(matches!(s.damage_since(recent), Damage::Rects(r) if r.len() == 2));
}

#[test]
fn test_extend_equals_insert() {
    let items: Vec<Item> = librepcb_canvas::stress::stress_items(2_000, 300, 50);
    let mut bulk = Scene::new();
    let bulk_ids = bulk.extend(items.clone());
    let mut single = Scene::new();
    let single_ids: Vec<_> = items.iter().cloned().map(|i| single.insert(i)).collect();
    assert_eq!(bulk.group_count(), single.group_count());
    assert_eq!(bulk.bounding_box(), single.bounding_box());
    let pos = |ids: &[librepcb_canvas::ItemId], id| ids.iter().position(|x| *x == id);
    for (x, y) in [(10.0, 10.0), (150.0, 100.0), (299.0, 1.0), (42.0, 170.0)] {
        let p = librepcb_canvas::kurbo::Point::new(x, y);
        let a: Vec<_> = bulk
            .items_at(p, 2.0)
            .into_iter()
            .map(|i| pos(&bulk_ids, i))
            .collect();
        let b: Vec<_> = single
            .items_at(p, 2.0)
            .into_iter()
            .map(|i| pos(&single_ids, i))
            .collect();
        assert_eq!(a, b);
    }
    // Extending a non-empty scene with few items inserts them one by one.
    let extra = bulk.extend(items[..3].to_vec());
    assert_eq!(extra.len(), 3);
    assert_eq!(bulk.len(), items.len() + 3);
}

#[test]
fn test_filled_paths_are_normalized() {
    let mut s = scene();
    // Two overlapping squares with opposite windings: union, no hole.
    let a = Rect::new(0.0, 0.0, 2.0, 2.0).to_path(0.1);
    let b = Rect::new(1.0, 1.0, 3.0, 3.0)
        .to_path(0.1)
        .reverse_subpaths();
    let mut both = a;
    both.extend(b.elements().iter().copied());
    let id = s.insert(Item::new(RED, both, Style::fill()));
    assert_eq!(
        s.hit_test(librepcb_canvas::kurbo::Point::new(1.5, 1.5), 0.0),
        Some(id)
    );
}

#[test]
fn test_unregistered_layer_uses_defaults() {
    let mut s = Scene::new();
    let id = s.insert(square(LayerId(99), 0.0, 0.0, 1.0));
    assert!(s.is_layer_visible(LayerId(99)));
    assert_eq!(s.draw_groups().count(), 1);
    assert_eq!(s.layer(LayerId(99)), None);
    s.set_layer_visible(LayerId(99), false);
    assert_eq!(s.layer(LayerId(99)).map(|l| l.visible), Some(false));
    assert_eq!(
        s.hit_test(librepcb_canvas::kurbo::Point::new(0.5, 0.5), 0.0),
        None
    );
    s.set_layer(LayerId(99), Layer::default());
    assert_eq!(
        s.hit_test(librepcb_canvas::kurbo::Point::new(0.5, 0.5), 0.0),
        Some(id)
    );
}

#[test]
fn test_dashed_stroke_style_is_own_group() {
    let mut s = scene();
    s.insert(trace(RED, 0.0, 0.0, 5.0, 0.0, 0.2));
    let dashed = Style {
        fill: None,
        stroke: Some(StrokeStyle::new(0.2).with_dash([1.0, 0.5], 0.0)),
    };
    s.insert(Item::new(
        RED,
        librepcb_canvas::kurbo::Line::new((0.0, 1.0), (5.0, 1.0)),
        dashed,
    ));
    assert_eq!(s.group_count(), 2);
}

/// Updating items of a scene built with `extend()` must not corrupt the
/// spatial index (rstar 0.13.0 `bulk_load()` builds trees which panic
/// with "This is a bug in rstar." on later insertions).
#[test]
fn test_update_after_extend() {
    fn rect(seed: &mut u64, id: usize) -> Item {
        let mut next = || {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((*seed >> 33) % 4000) as f64 / 100.0
        };
        let (x, y) = (next(), next());
        let (w, h) = (
            next() / 10.0,
            if id.is_multiple_of(3) {
                0.0
            } else {
                next() / 10.0
            },
        );
        Item::new(
            GREEN,
            Rect::new(x, y, x + w, y + h).to_path(0.1),
            Style::stroke(0.0),
        )
    }
    for n in [100, 443, 1000] {
        for s in 0..5u64 {
            let mut seed = s * 7919 + n as u64;
            let mut scene = scene();
            let items: Vec<Item> = (0..n).map(|i| rect(&mut seed, i)).collect();
            let ids = scene.extend(items);
            for round in 0..3 {
                for (i, id) in ids.iter().enumerate() {
                    if (i + round) % 3 != 0 {
                        assert!(scene.update(*id, rect(&mut seed, i)));
                    }
                }
            }
            assert_eq!(scene.len(), n);
        }
    }
}
