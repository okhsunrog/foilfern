//! Derived via geometry (BI_Via has no upstream unit test suite).

use super::*;
use foilfern_core::geometry::Via;
use foilfern_core::project::board::ViaView;
use foilfern_core::project::{BoardMutation, Mutation};

fn via(start: Layer, end: Layer, drill: Option<PositiveLength>, exposure: MaskConfig) -> Via {
    Via::new(
        Uuid::new_random(),
        start,
        end,
        Point::ORIGIN,
        drill,
        None,
        exposure,
    )
    .unwrap()
}

#[test]
fn inherited_drill_and_annular_ring() {
    let mut f = Fixture::new();
    let via = via(
        Layer::TOP_COPPER,
        Layer::BOT_COPPER,
        None,
        MaskConfig::Automatic,
    );
    let geometry = ViaView::new(&via, f.board(), None, f.p.circuit()).unwrap();
    assert_eq!(geometry.drill_diameter(), pos_mm(0.3));
    assert_eq!(geometry.size(), pos_mm(0.7));
    assert_eq!(geometry.stop_mask_diameter_top(), Some(pos_mm(0.9)));
    let net_class = f.p.circuit().net_signal(f.gnd).unwrap().net_class();
    let mut class = f.p.circuit().net_classes()[&net_class].clone();
    class.set_default_via_drill(Some(pos_mm(1.0)));
    f.p.apply(Mutation::UpdateNetClass(class)).unwrap();
    let geometry = ViaView::new(&via, f.board(), Some(f.gnd), f.p.circuit()).unwrap();
    assert_eq!(geometry.drill_diameter(), pos_mm(1.0));
    assert_eq!(geometry.size(), pos_mm(1.5));
    let explicit = Via::new(
        Uuid::new_random(),
        Layer::TOP_COPPER,
        Layer::BOT_COPPER,
        Point::ORIGIN,
        Some(pos_mm(0.2)),
        Some(pos_mm(0.8)),
        MaskConfig::Off,
    )
    .unwrap();
    let geometry = ViaView::new(&explicit, f.board(), Some(f.gnd), f.p.circuit()).unwrap();
    assert_eq!(geometry.drill_diameter(), pos_mm(0.2));
    assert_eq!(geometry.size(), pos_mm(0.8));
    assert_eq!(geometry.via(), &explicit);
}

#[test]
fn mask_exposure_and_automatic_drill_opening() {
    let f = Fixture::new();
    for (drill, exposure, expected) in [
        (0.3, MaskConfig::Off, None),
        (0.4, MaskConfig::Off, Some(0.6)), // Opening is relative to drill, not copper size.
        (
            0.3,
            MaskConfig::Manual(Length::from_mm(0.05).unwrap()),
            Some(0.8),
        ),
        (
            0.3,
            MaskConfig::Manual(Length::from_mm(-0.4).unwrap()),
            None,
        ),
        (0.3, MaskConfig::Automatic, Some(0.9)),
    ] {
        let via = via(
            Layer::TOP_COPPER,
            Layer::BOT_COPPER,
            Some(pos_mm(drill)),
            exposure,
        );
        let geometry = ViaView::new(&via, f.board(), None, f.p.circuit()).unwrap();
        assert_eq!(geometry.stop_mask_diameter_top(), expected.map(pos_mm));
        assert_eq!(geometry.stop_mask_diameter_bottom(), expected.map(pos_mm));
    }
    let blind = via(
        Layer::TOP_COPPER,
        Layer::inner_copper(1).unwrap(),
        None,
        MaskConfig::Automatic,
    );
    let geometry = ViaView::new(&blind, f.board(), None, f.p.circuit()).unwrap();
    assert!(geometry.stop_mask_diameter_top().is_some());
    assert_eq!(geometry.stop_mask_diameter_bottom(), None);
}

#[test]
fn physical_drill_span_is_clipped_to_enabled_layers() {
    let mut f = Fixture::new();
    let mut settings = f.board().settings().clone();
    settings.inner_layer_count = 2;
    f.p.apply(Mutation::Board(BoardMutation::SetSettings {
        board: f.board,
        settings: Box::new(settings),
    }))
    .unwrap();
    let inner = |i| Layer::inner_copper(i).unwrap();
    for (start, end, expected) in [
        (
            Layer::TOP_COPPER,
            Layer::BOT_COPPER,
            Some((Layer::TOP_COPPER, Layer::BOT_COPPER)),
        ),
        (
            Layer::TOP_COPPER,
            inner(4),
            Some((Layer::TOP_COPPER, inner(2))),
        ),
        (inner(1), inner(4), Some((inner(1), inner(2)))),
        (inner(2), inner(4), None),
        (inner(3), Layer::BOT_COPPER, None),
    ] {
        let via = via(start, end, None, MaskConfig::Off);
        let geometry = ViaView::new(&via, f.board(), None, f.p.circuit()).unwrap();
        assert_eq!(geometry.drill_layer_span(), expected);
    }
}

#[test]
fn overflowing_exposure_returns_a_range_error() {
    let f = Fixture::new();
    let via = via(
        Layer::TOP_COPPER,
        Layer::BOT_COPPER,
        None,
        MaskConfig::Manual(Length::new(i64::MAX)),
    );
    assert!(matches!(
        ViaView::new(&via, f.board(), None, f.p.circuit()),
        Err(foilfern_core::project::Error::Types(
            foilfern_core::types::Error::Range { .. }
        ))
    ));
}
