//! Resolves builtin routing inputs and records which rule supplied each value.

use librepcb_autoroute::RoutingRules;
use librepcb_core::geometry::Via;
use librepcb_core::library::org::BoardDesignRuleCheckSettings;
use librepcb_core::project::board::{Board, BoardDesignRules};
use librepcb_core::project::circuit::{Circuit, NetClass};
use librepcb_core::project::{NetClassId, NetSignalId, Project};
use librepcb_core::types::{Length, PositiveLength, Uuid};
use serde::Serialize;

use super::{Autoroute, CLEARANCE_MARGIN, ExtractedProblem, positive};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Source {
    Request { field: &'static str },
    BoardDesign { field: &'static str },
    BoardDrc { field: &'static str },
    NetClass { uuid: Uuid, field: &'static str },
    BuiltinDefault,
    BuiltinAuto,
    BoardStackup,
}

#[derive(Clone, Copy, Serialize)]
struct RuleValue {
    #[serde(rename = "mm", serialize_with = "serialize_mm")]
    length: Length,
    source: Source,
}

fn serialize_mm<S: serde::Serializer>(length: &Length, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(length.to_mm())
}

fn drc(field: &'static str, length: Length) -> RuleValue {
    RuleValue {
        length,
        source: Source::BoardDrc { field },
    }
}

struct Dimension {
    base: (PositiveLength, Source),
    minimum: RuleValue,
}

impl Dimension {
    fn effective(&self) -> PositiveLength {
        positive((*self.base.0).max(self.minimum.length)).unwrap_or(self.base.0)
    }

    fn report(&self, effective: PositiveLength) -> DimensionReport {
        DimensionReport {
            effective_mm: effective.to_mm(),
            base: RuleValue {
                length: *self.base.0,
                source: self.base.1,
            },
            minimum: self.minimum,
            clamped: effective > self.base.0,
        }
    }
}

#[derive(Serialize)]
struct DimensionReport {
    effective_mm: f64,
    base: RuleValue,
    minimum: RuleValue,
    clamped: bool,
}

#[derive(Serialize)]
struct ClearanceReport {
    effective_mm: f64,
    base: RuleValue,
    margin_mm: f64,
}

impl ClearanceReport {
    fn new(effective: Length, base: RuleValue) -> Self {
        Self {
            effective_mm: effective.to_mm(),
            base,
            margin_mm: CLEARANCE_MARGIN.to_mm(),
        }
    }
}

#[derive(Serialize)]
struct NetClassDrc {
    uuid: Uuid,
    name: String,
    min_copper_width_mm: f64,
    min_via_drill_diameter_mm: f64,
}

#[derive(Serialize)]
struct ViaStorage {
    explicit: bool,
    inherited_drill: RuleValue,
    inherited_diameter_mm: f64,
}

#[derive(Serialize)]
struct GridPitchReport {
    effective_mm: Option<f64>,
    requested_mm: Option<f64>,
    source: Source,
}

#[derive(Serialize)]
struct LayerReport {
    effective: Vec<&'static str>,
    source: Source,
}

#[derive(Serialize)]
struct Report {
    trace_width: DimensionReport,
    via_drill: DimensionReport,
    via_diameter: DimensionReport,
    min_annular_ring: RuleValue,
    via_storage: ViaStorage,
    copper_clearance: ClearanceReport,
    board_copper_minimum: RuleValue,
    largest_class_copper_minimum: Option<RuleValue>,
    copper_board_clearance: ClearanceReport,
    copper_npth_clearance: ClearanceReport,
    drill_clearance: ClearanceReport,
    drill_board_clearance: ClearanceReport,
    net_class_drc: Option<NetClassDrc>,
    grid_pitch: GridPitchReport,
    trace_layers: LayerReport,
    via_cost: RuleValue,
    max_rip_ups: u32,
}

pub(super) struct Inputs<'a> {
    circuit: &'a Circuit,
    design: &'a BoardDesignRules,
    drc: &'a BoardDesignRuleCheckSettings,
    options: &'a Autoroute,
}

impl<'a> Inputs<'a> {
    pub(super) fn new(p: &'a Project, board: &'a Board, options: &'a Autoroute) -> Self {
        Self {
            circuit: p.circuit(),
            design: board.design_rules(),
            drc: &board.settings().drc_settings,
            options,
        }
    }

    fn trace_width(&self, class: Option<&NetClass>) -> Dimension {
        let base = if let Some(width) = self.options.trace_width {
            (
                width,
                Source::Request {
                    field: "trace_width",
                },
            )
        } else if let Some((class, width)) =
            class.and_then(|c| c.default_trace_width().map(|w| (c, w)))
        {
            (
                width,
                Source::NetClass {
                    uuid: class.uuid(),
                    field: "default_trace_width",
                },
            )
        } else {
            (
                self.design.default_trace_width(),
                Source::BoardDesign {
                    field: "default_trace_width",
                },
            )
        };
        Dimension {
            base,
            minimum: drc("min_copper_width", *self.drc.min_copper_width()),
        }
    }

    fn board_via_drill(&self) -> (PositiveLength, Source) {
        (
            self.design.default_via_drill_diameter(),
            Source::BoardDesign {
                field: "default_via_drill_diameter",
            },
        )
    }

    fn via_drill(&self) -> Dimension {
        let base = self.options.via_drill.map_or_else(
            || self.board_via_drill(),
            |drill| (drill, Source::Request { field: "via_drill" }),
        );
        Dimension {
            base,
            minimum: drc("min_pth_drill_diameter", *self.drc.min_pth_drill_diameter()),
        }
    }

    fn min_annular_ring(&self) -> RuleValue {
        drc("min_pth_annular_ring", *self.drc.min_pth_annular_ring())
    }

    fn via_diameter(&self, drill: PositiveLength) -> Dimension {
        let base = self.options.via_size.map_or_else(
            || {
                (
                    self.diameter_from_design_rules(drill),
                    Source::BoardDesign {
                        field: "via_annular_ring",
                    },
                )
            },
            |size| (size, Source::Request { field: "via_size" }),
        );
        let annular = self.min_annular_ring();
        Dimension {
            base,
            minimum: RuleValue {
                length: *drill + annular.length * 2,
                source: annular.source,
            },
        }
    }

    fn inherited_drill(&self, class: Option<&NetClass>) -> (PositiveLength, Source) {
        class
            .and_then(|c| {
                c.default_via_drill().map(|drill| {
                    (
                        drill,
                        Source::NetClass {
                            uuid: c.uuid(),
                            field: "default_via_drill_diameter",
                        },
                    )
                })
            })
            .unwrap_or_else(|| self.board_via_drill())
    }

    fn diameter_from_design_rules(&self, drill: PositiveLength) -> PositiveLength {
        Via::calc_size_from_rules(drill, &self.design.via_annular_ring())
    }

    pub(super) fn inherited_via(&self, class: NetClassId) -> (PositiveLength, PositiveLength) {
        let (drill, _) = self.inherited_drill(self.circuit.net_class(class));
        (drill, self.diameter_from_design_rules(drill))
    }

    fn board_copper_minimum(&self) -> RuleValue {
        drc(
            "min_copper_copper_clearance",
            *self.drc.min_copper_copper_clearance(),
        )
    }

    fn largest_class_copper_minimum(&self) -> Option<RuleValue> {
        self.circuit
            .net_classes()
            .values()
            .max_by_key(|c| c.min_copper_copper_clearance())
            .map(|c| RuleValue {
                length: *c.min_copper_copper_clearance(),
                source: Source::NetClass {
                    uuid: c.uuid(),
                    field: "min_copper_copper_clearance",
                },
            })
    }

    fn copper_clearance_base(&self, largest_class: Option<RuleValue>) -> RuleValue {
        let board = self.board_copper_minimum();
        largest_class
            .filter(|class| class.length > board.length)
            .unwrap_or(board)
    }

    fn copper_board_clearance(&self) -> RuleValue {
        drc(
            "min_copper_board_clearance",
            *self.drc.min_copper_board_clearance(),
        )
    }

    fn copper_npth_clearance(&self) -> RuleValue {
        drc(
            "min_copper_npth_clearance",
            *self.drc.min_copper_npth_clearance(),
        )
    }

    fn drill_clearance(&self) -> RuleValue {
        drc(
            "min_drill_drill_clearance",
            *self.drc.min_drill_drill_clearance(),
        )
    }

    fn drill_board_clearance(&self) -> RuleValue {
        drc(
            "min_drill_board_clearance",
            *self.drc.min_drill_board_clearance(),
        )
    }

    pub(super) fn routing_rules(&self) -> RoutingRules {
        let default = RoutingRules::default();
        let mut net_trace_widths = std::collections::BTreeMap::new();
        if self.options.trace_width.is_none() {
            for net in self.circuit.net_signals().values() {
                let class = self.circuit.net_class(net.net_class());
                if class.is_some_and(|c| c.default_trace_width().is_some()) {
                    net_trace_widths.insert(net.uuid(), self.trace_width(class).effective());
                }
            }
        }
        let clearance = self.copper_clearance_base(self.largest_class_copper_minimum());
        let via_drill = self.via_drill().effective();
        RoutingRules {
            trace_width: self.trace_width(None).effective(),
            net_trace_widths,
            clearance: clearance.length + CLEARANCE_MARGIN,
            board_clearance: self.copper_board_clearance().length + CLEARANCE_MARGIN,
            npth_clearance: self.copper_npth_clearance().length + CLEARANCE_MARGIN,
            drill_clearance: self.drill_clearance().length + CLEARANCE_MARGIN,
            drill_board_clearance: self.drill_board_clearance().length + CLEARANCE_MARGIN,
            via_drill,
            via_diameter: self.via_diameter(via_drill).effective(),
            via_cost: self.options.via_cost.unwrap_or(default.via_cost),
            allowed_layers: self.options.layers.clone(),
            grid_pitch: self.options.grid_pitch,
            search_evidence: self.options.search_evidence,
            pad_escape: self.options.pad_escape,
            congestion_history: self.options.congestion_history,
            order: self.options.order.map(Into::into).unwrap_or_default(),
            ..default
        }
    }

    pub(super) fn report(
        &self,
        extracted: &ExtractedProblem,
        net: NetSignalId,
        grid_pitch: Option<Length>,
    ) -> impl Serialize + use<> {
        let problem = &extracted.problem;
        let rules = &problem.rules;
        let class = self
            .circuit
            .net_signal(net)
            .and_then(|n| self.circuit.net_class(n.net_class()));
        let (inherited_drill, inherited_source) = self.inherited_drill(class);
        let largest_class = self.largest_class_copper_minimum();
        Report {
            trace_width: self.trace_width(class).report(rules.trace_width_of(&net.0)),
            via_drill: self.via_drill().report(rules.via_drill),
            via_diameter: self
                .via_diameter(rules.via_drill)
                .report(rules.via_diameter),
            min_annular_ring: self.min_annular_ring(),
            via_storage: ViaStorage {
                explicit: extracted
                    .via_sizes
                    .get(&net)
                    .is_some_and(|(drill, _)| drill.is_some()),
                inherited_drill: RuleValue {
                    length: *inherited_drill,
                    source: inherited_source,
                },
                inherited_diameter_mm: self.diameter_from_design_rules(inherited_drill).to_mm(),
            },
            copper_clearance: ClearanceReport::new(
                rules.clearance,
                self.copper_clearance_base(largest_class),
            ),
            board_copper_minimum: self.board_copper_minimum(),
            largest_class_copper_minimum: largest_class,
            copper_board_clearance: ClearanceReport::new(
                rules.board_clearance,
                self.copper_board_clearance(),
            ),
            copper_npth_clearance: ClearanceReport::new(
                rules.npth_clearance,
                self.copper_npth_clearance(),
            ),
            drill_clearance: ClearanceReport::new(rules.drill_clearance, self.drill_clearance()),
            drill_board_clearance: ClearanceReport::new(
                rules.drill_board_clearance,
                self.drill_board_clearance(),
            ),
            net_class_drc: class.map(|c| NetClassDrc {
                uuid: c.uuid(),
                name: c.name().as_str().to_owned(),
                min_copper_width_mm: c.min_copper_width().to_mm(),
                min_via_drill_diameter_mm: c.min_via_drill_diameter().to_mm(),
            }),
            grid_pitch: GridPitchReport {
                effective_mm: grid_pitch.map(Length::to_mm),
                requested_mm: rules.grid_pitch.map(|pitch| pitch.to_mm()),
                source: if rules.grid_pitch.is_some() {
                    Source::Request {
                        field: "grid_pitch",
                    }
                } else {
                    Source::BuiltinAuto
                },
            },
            trace_layers: LayerReport {
                effective: problem
                    .copper_layers
                    .iter()
                    .filter(|layer| {
                        rules
                            .allowed_layers
                            .as_ref()
                            .is_none_or(|allowed| allowed.contains(layer))
                    })
                    .map(|layer| layer.id())
                    .collect(),
                source: if rules.allowed_layers.is_some() {
                    Source::Request { field: "layers" }
                } else {
                    Source::BoardStackup
                },
            },
            via_cost: RuleValue {
                length: rules.via_cost,
                source: self
                    .options
                    .via_cost
                    .map_or(Source::BuiltinDefault, |_| Source::Request {
                        field: "via_cost",
                    }),
            },
            max_rip_ups: rules.max_rip_ups,
        }
    }
}
