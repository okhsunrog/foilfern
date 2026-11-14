//! Port of libs/librepcb/core/project/board/items/bi_via.{h,cpp}:
//! derived drill diameter, copper size, mask openings and drill layer span.
//!
//! A read view replaces upstream's cached state and signals. Arithmetic
//! follows upstream's rule calculations with checked widened addition for
//! dimensions, so extreme input returns a range error instead of overflow.

use super::Board;
use crate::geometry::Via;
use crate::project::circuit::Circuit;
use crate::project::{Error, NetSignalId, Result};
use crate::types::{Layer, Length, MaskConfig, PositiveLength};

/// Manufacturing geometry of a via in its board and net-class context.
#[derive(Debug, Clone, Copy)]
pub struct ViaView<'a> {
    via: &'a Via,
    drill_diameter: PositiveLength,
    size: PositiveLength,
    stop_mask_top: Option<PositiveLength>,
    stop_mask_bottom: Option<PositiveLength>,
    drill_layer_span: Option<(Layer, Layer)>,
}

static_assertions::assert_impl_all!(ViaView<'_>: Send, Sync);

impl<'a> ViaView<'a> {
    /// Resolves inherited drill rules and computes geometry without changing
    /// the model. `net` is the owning board net segment's net.
    pub fn new(
        via: &'a Via,
        board: &Board,
        net: Option<NetSignalId>,
        circuit: &Circuit,
    ) -> Result<Self> {
        let inherited = if let Some(net) = net {
            let net = circuit
                .net_signal(net)
                .ok_or(Error::InexistentNetSignal(net.0))?;
            circuit
                .net_classes()
                .get(&net.net_class())
                .ok_or(Error::InexistentNetClass(net.net_class().0))?
                .default_via_drill()
        } else {
            None
        };
        let rules = board.design_rules();
        let drill_diameter = via
            .drill_diameter()
            .or(inherited)
            .unwrap_or(rules.default_via_drill_diameter());
        let size = match via.size() {
            Some(size) => size.max(drill_diameter),
            None => positive_dimension(
                i128::from(drill_diameter.to_nm())
                    + 2 * i128::from(rules.via_annular_ring().calc_value(*drill_diameter).to_nm()),
            )?,
        };
        let opening = match via.exposure_config() {
            MaskConfig::Manual(offset) => i128::from(size.to_nm()) + 2 * i128::from(offset.to_nm()),
            MaskConfig::Automatic => {
                i128::from(size.to_nm())
                    + 2 * i128::from(rules.stop_mask_clearance().calc_value(*size).to_nm())
            }
            MaskConfig::Off if rules.does_via_require_stop_mask_opening(*drill_diameter) => {
                i128::from(drill_diameter.to_nm())
                    + 2 * i128::from(
                        rules
                            .stop_mask_clearance()
                            .calc_value(*drill_diameter)
                            .to_nm(),
                    )
            }
            MaskConfig::Off => 0,
        };
        let opening = if opening > 0 {
            Some(positive_dimension(opening)?)
        } else {
            None
        };
        let start = via.start_layer();
        let end = via.end_layer();
        let inner_layers = board.settings().inner_layer_count as usize;
        let drill_layer_span = if start.copper_number() > inner_layers {
            None
        } else if end.is_bottom() {
            Some((start, end))
        } else {
            let last = end.copper_number().min(inner_layers);
            Layer::inner_copper(last)
                .filter(|_| start.copper_number() < last)
                .map(|last| (start, last))
        };
        Ok(Self {
            via,
            drill_diameter,
            size,
            stop_mask_top: opening.filter(|_| start.is_top()),
            stop_mask_bottom: opening.filter(|_| end.is_bottom()),
            drill_layer_span,
        })
    }

    /// Returns the persistent via properties.
    pub fn via(&self) -> &'a Via {
        self.via
    }
    /// Returns the actual drill diameter, including inherited defaults.
    pub fn drill_diameter(&self) -> PositiveLength {
        self.drill_diameter
    }
    /// Returns the actual copper diameter, at least as large as the drill.
    pub fn size(&self) -> PositiveLength {
        self.size
    }
    /// Returns the top stop-mask opening diameter, if any.
    pub fn stop_mask_diameter_top(&self) -> Option<PositiveLength> {
        self.stop_mask_top
    }
    /// Returns the bottom stop-mask opening diameter, if any.
    pub fn stop_mask_diameter_bottom(&self) -> Option<PositiveLength> {
        self.stop_mask_bottom
    }
    /// Returns the physical drill span clipped to enabled copper layers;
    /// `None` means no valid drill span on this board.
    pub fn drill_layer_span(&self) -> Option<(Layer, Layer)> {
        self.drill_layer_span
    }
}

fn positive_dimension(value: i128) -> Result<PositiveLength> {
    let nanometers = i64::try_from(value).map_err(|_| crate::types::Error::Range {
        value: value as f64,
        min: 1,
        max: i64::MAX,
    })?;
    Ok(PositiveLength::new(Length::new(nanometers))?)
}
