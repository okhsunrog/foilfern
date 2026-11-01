//! Port of manufacturing/layer checks in
//! libs/librepcb/core/project/board/drc/boarddesignrulecheck.cpp.

use super::{
    BoardDesignRuleCheck, BoardDrcData, BoardDrcMessage, DrcHoleRef, DrcMessageKind, messages,
};
use crate::geometry::Path;
use crate::library::org::AllowedSlots;
use crate::rule_check::{RuleCheckMessage, Severity};
use crate::types::{Length, PositiveLength};
use librepcb_i18n::tr;

impl BoardDesignRuleCheck {
    /// Runs only the currently ported manufacturing rules: drill diameters,
    /// slot widths/shapes, thermal spoke settings and copper layer use.
    ///
    /// This is not a full DRC. In particular, no clearance, connectivity,
    /// minimum copper width or board outline checks are performed here.
    pub fn run_manufacturing_rules(data: &BoardDrcData) -> Vec<BoardDrcMessage> {
        let mut result = Vec::new();
        let mut add = |(kind, rule)| {
            result.push(BoardDrcMessage {
                board: data.board,
                kind,
                rule,
            })
        };
        for hole in &data.holes {
            let plated = hole.reference.is_plated();
            let minimum_drill = if matches!(hole.reference, DrcHoleRef::Via { .. }) {
                data.settings
                    .min_pth_drill_diameter()
                    .max(hole.min_via_drill)
            } else if plated {
                data.settings.min_pth_drill_diameter()
            } else {
                data.settings.min_npth_drill_diameter()
            };
            let minimum_slot = if plated {
                data.settings.min_pth_slot_width()
            } else {
                data.settings.min_npth_slot_width()
            };
            let allowed = if plated {
                data.settings.allowed_pth_slots()
            } else {
                data.settings.allowed_npth_slots()
            };
            let count = hole.path.vertices().len();
            let locations = || hole.path.to_outline_strokes(hole.diameter);
            if count == 1 && *hole.diameter < *minimum_drill {
                let locations = if let Some(position) = hole.pad_position {
                    // Upstream highlights round pad drills at the pad origin,
                    // including when their hole path has an off-center origin.
                    let diameter = hole
                        .diameter
                        .max(PositiveLength::new(Length::new(50_000)).expect("positive constant"));
                    vec![Path::circle(diameter).translated(position)]
                } else {
                    locations()
                };
                add(messages::minimum_drill(hole, minimum_drill, locations));
            }
            if count > 1 && *hole.diameter < *minimum_slot {
                add(messages::minimum_slot(hole, minimum_slot, locations()));
            }
            if (hole.path.is_curved() && allowed < AllowedSlots::Any)
                || (count > 2 && allowed < AllowedSlots::MultiSegmentStraight)
                || (count > 1 && allowed < AllowedSlots::SingleSegmentStraight)
            {
                add(messages::forbidden_slot(hole, locations()));
            }
        }
        for (plane, net) in &data.planes {
            if plane.min_width() <= plane.thermal_spoke_width() {
                continue;
            }
            let mut outline = plane.outline().clone();
            outline.close();
            let locations = outline.to_outline_strokes(
                PositiveLength::new(Length::new(200_000)).expect("positive constant"),
            );
            let mut rule = RuleCheckMessage::new(
                Severity::Warning,
                tr!(
                    "DrcMsgPlaneThermalSpokeWidthIgnored",
                    "Ignored thermal spoke width of plane in '{0}'",
                    messages::net_with_fallback(net)
                ),
                tr!(
                    "DrcMsgPlaneThermalSpokeWidthIgnored",
                    "The thermal spoke width of the plane is set to a value smaller than the planes minimum width, which is not allowed. Therefore the plane filling algorithm ignores the specified spoke width and uses the specified minimum width instead."
                ) + "\n\n"
                    + &tr!(
                        "DrcMsgPlaneThermalSpokeWidthIgnored",
                        "It is recommended to explicitly set the thermal spoke width equal to or higher than the minimum width of the plane."
                    ),
                "plane_thermal_spoke_width_ignored",
                locations,
            );
            let approval = rule.approval_mut();
            approval.ensure_line_break();
            approval.append_child("plane", &plane.uuid());
            approval.ensure_line_break();
            add((
                DrcMessageKind::PlaneThermalSpokeWidthIgnored(plane.id()),
                rule,
            ));
        }
        for &layer in data.used_layers.difference(&data.copper_layers) {
            let mut rule = RuleCheckMessage::new(
                Severity::Warning,
                tr!(
                    "DrcMsgDisabledLayer",
                    "Objects on disabled layer: '{0}'",
                    layer.name_tr()
                ),
                tr!(
                    "DrcMsgDisabledLayer",
                    "The layer contains copper objects, but it is disabled in the board setup dialog and thus will be ignored in any production data exports. Either increase the layer count to get this layer exported, or remove all objects on this layer (by temporarily enabling this layer to see them)."
                ),
                "disabled_layer",
                Vec::new(),
            );
            let approval = rule.approval_mut();
            approval.ensure_line_break();
            approval.append_child("layer", &layer.id());
            approval.ensure_line_break();
            add((DrcMessageKind::DisabledLayer(layer), rule));
        }
        for &layer in data.copper_layers.difference(&data.used_layers) {
            let mut rule = RuleCheckMessage::new(
                Severity::Hint,
                tr!("DrcMsgUnusedLayer", "Unused layer: '{0}'", layer.name_tr()),
                tr!(
                    "DrcMsgUnusedLayer",
                    "The layer contains no copper objects (except the automatically generated through-hole annular rings, if any) so it is useless. This is not critical, but if your intention is to flood it with copper, you need to add a plane manually. Or if you don't need this layer, you might want to reduce the layer count in the board setup dialog to avoid unnecessary production costs. Also some PCB manufacturers might be confused by empty layers."
                ),
                "unused_layer",
                Vec::new(),
            );
            let approval = rule.approval_mut();
            approval.ensure_line_break();
            approval.append_child("layer", &layer.id());
            approval.ensure_line_break();
            add((DrcMessageKind::UnusedLayer(layer), rule));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::data::Hole;
    use super::*;
    use crate::geometry::{NonEmptyPath, Vertex};
    use crate::library::org::BoardDesignRuleCheckSettings;
    use crate::project::{BoardId, NetSegmentId};
    use crate::types::{Angle, Point, UnsignedLength, Uuid};

    fn unsigned(nm: i64) -> UnsignedLength {
        UnsignedLength::new(Length::new(nm)).unwrap()
    }
    fn data(reference: DrcHoleRef, diameter: i64, vertices: Vec<Vertex>) -> BoardDrcData {
        let mut settings = BoardDesignRuleCheckSettings::default();
        settings.set_min_npth_drill_diameter(unsigned(150_000));
        settings.set_min_pth_drill_diameter(unsigned(150_000));
        settings.set_min_npth_slot_width(unsigned(150_000));
        settings.set_min_pth_slot_width(unsigned(150_000));
        settings.set_allowed_npth_slots(AllowedSlots::Any);
        settings.set_allowed_pth_slots(AllowedSlots::Any);
        BoardDrcData {
            board: BoardId::new_random(),
            settings,
            holes: vec![Hole {
                reference,
                diameter: PositiveLength::new(Length::new(diameter)).unwrap(),
                path: NonEmptyPath::new(Path::new(vertices)).unwrap(),
                pad_position: None,
                pad_name: String::new(),
                net_name: String::new(),
                min_via_drill: UnsignedLength::ZERO,
            }],
            planes: Vec::new(),
            used_layers: Default::default(),
            copper_layers: Default::default(),
        }
    }
    #[test]
    fn diameter_and_width_limits_are_strict() {
        for plated in [false, true] {
            for slot in [false, true] {
                for diameter in [149_999, 150_000, 150_001] {
                    let reference = if plated {
                        DrcHoleRef::PadHole {
                            segment: NetSegmentId::new_random(),
                            pad: Uuid::new_random(),
                            hole: Uuid::new_random(),
                        }
                    } else {
                        DrcHoleRef::BoardHole(Uuid::new_random())
                    };
                    let mut vertices = vec![Vertex::at(Point::ORIGIN)];
                    if slot {
                        vertices.push(Vertex::at(Point::from_nm(1_000_000, 0)));
                    }
                    let data = data(reference, diameter, vertices);
                    let result = BoardDesignRuleCheck::run_manufacturing_rules(&data);
                    assert_eq!(result.len(), usize::from(diameter < 150_000));
                    if let Some(finding) = result.first() {
                        assert_eq!(
                            matches!(finding.kind(), DrcMessageKind::MinimumSlotWidth { .. }),
                            slot
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn all_slot_policies_distinguish_round_straight_multi_and_curved() {
        let round = vec![Vertex::at(Point::ORIGIN)];
        let straight = vec![
            Vertex::at(Point::ORIGIN),
            Vertex::at(Point::from_nm(1_000_000, 0)),
        ];
        let mut multi = straight.clone();
        multi.push(Vertex::at(Point::from_nm(1_000_000, 1_000_000)));
        let mut curved = straight.clone();
        curved[0].angle = Angle::DEG90;
        for (allowed, expected) in [
            (AllowedSlots::None, [false, true, true, true]),
            (
                AllowedSlots::SingleSegmentStraight,
                [false, false, true, true],
            ),
            (
                AllowedSlots::MultiSegmentStraight,
                [false, false, false, true],
            ),
            (AllowedSlots::Any, [false, false, false, false]),
        ] {
            for (vertices, forbidden) in [&round, &straight, &multi, &curved]
                .into_iter()
                .zip(expected)
            {
                let mut data = data(
                    DrcHoleRef::BoardHole(Uuid::new_random()),
                    150_000,
                    vertices.clone(),
                );
                data.settings.set_allowed_npth_slots(allowed);
                assert_eq!(
                    BoardDesignRuleCheck::run_manufacturing_rules(&data).len(),
                    usize::from(forbidden)
                );
            }
        }
    }
    #[test]
    fn via_uses_the_stricter_net_class_limit() {
        let mut data = data(
            DrcHoleRef::Via {
                segment: NetSegmentId::new_random(),
                via: Uuid::new_random(),
            },
            150_000,
            vec![Vertex::at(Point::ORIGIN)],
        );
        data.holes[0].min_via_drill = unsigned(200_000);
        let result = BoardDesignRuleCheck::run_manufacturing_rules(&data);
        assert_eq!(result.len(), 1);
        assert!(
            matches!(result[0].kind(), DrcMessageKind::MinimumDrillDiameter { minimum, .. } if *minimum == unsigned(200_000))
        );
        assert_eq!(
            result[0].rule_message().message(),
            "Via drill diameter of '(no net)': 0.15 < 0.2 mm"
        );
    }
    #[test]
    fn round_pad_drill_highlight_uses_pad_origin_and_minimum_diameter() {
        let mut data = data(
            DrcHoleRef::PadHole {
                segment: NetSegmentId::new_random(),
                pad: Uuid::new_random(),
                hole: Uuid::new_random(),
            },
            10_000,
            vec![Vertex::at(Point::from_nm(200_000, 0))],
        );
        let origin = Point::from_nm(1_000_000, 2_000_000);
        data.holes[0].pad_position = Some(origin);
        let result = BoardDesignRuleCheck::run_manufacturing_rules(&data);
        assert_eq!(
            result[0].rule_message().locations(),
            &[Path::circle(PositiveLength::new(Length::new(50_000)).unwrap()).translated(origin)]
        );
    }
}
