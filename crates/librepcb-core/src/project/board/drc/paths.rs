//! Port of libs/librepcb/core/project/board/drc/boardclipperpathgenerator.{h,cpp}.
//!
//! The existing Clipper implementation and core shapes reproduce upstream's
//! offset-before-union, arc tolerance and fill-rule choices.

use super::drill_clearances::tolerance;
use super::physical_data::{Drawing, OwnedVia, Pad, Plane, Text, Trace};
use super::{BoardDrcData, DrcCopperRef, DrcObjectRef};
use crate::geometry::{NonEmptyPath, Path};
use crate::project::Result;
use crate::types::{Layer, Length, PositiveLength};
use crate::utils::clipper_helpers::{self as ch, ClipperPaths};
use clipper::{
    JoinType,
    PolyFillType::{EvenOdd, NonZero},
};

pub(super) fn unite(target: &mut ClipperPaths, source: &ClipperPaths) -> Result<()> {
    Ok(ch::unite_with(target, source, EvenOdd, NonZero)?)
}
pub(super) fn drawing(drawing: &Drawing, offset: Length) -> Result<ClipperPaths> {
    let mut result = Vec::new();
    let (path, width, fill) = if let Some((center, diameter)) = drawing.circle {
        let diameter = PositiveLength::new((*diameter + offset * 2).max(Length::new(1)))?;
        (
            Path::circle(diameter).translated(center),
            *drawing.width,
            drawing.filled,
        )
    } else {
        (
            drawing.path.clone(),
            *drawing.width + offset * 2,
            drawing.filled,
        )
    };
    if drawing.width > crate::types::UnsignedLength::ZERO && width > Length::ZERO {
        unite(
            &mut result,
            &ch::paths_to_clipper(
                &path.to_outline_strokes(PositiveLength::new(width)?),
                tolerance(),
            ),
        )?;
    }
    if fill && path.is_closed() {
        let mut area = ch::paths_to_clipper(std::slice::from_ref(&path), tolerance());
        if drawing.circle.is_none() && offset != Length::ZERO {
            ch::offset(&mut area, offset, tolerance(), JoinType::Round)?;
        }
        ch::unite_with(&mut result, &area, EvenOdd, EvenOdd)?;
    }
    Ok(result)
}
pub(super) fn text(text: &Text, offset: Length) -> Result<ClipperPaths> {
    let width = PositiveLength::new((*text.width + offset * 2).max(Length::new(1)))?;
    let mut result = Vec::new();
    for path in &text.paths {
        unite(
            &mut result,
            &ch::paths_to_clipper(&path.to_outline_strokes(width), tolerance()),
        )?;
    }
    Ok(result)
}
pub(super) fn pad(pad: &Pad, layer: Layer, offset: Length, holes: bool) -> Result<ClipperPaths> {
    let mut result = Vec::new();
    for geometry in pad.geometries.get(&layer).into_iter().flatten() {
        let geometry = geometry.with_offset(offset);
        let paths = geometry
            .to_outlines()?
            .iter()
            .map(|p| pad.transform.map(p))
            .collect::<Vec<_>>();
        unite(&mut result, &ch::paths_to_clipper(&paths, tolerance()))?;
        if holes {
            for hole in geometry.holes() {
                let paths = hole
                    .path()
                    .to_outline_strokes(hole.diameter())
                    .iter()
                    .map(|p| pad.transform.map(p))
                    .collect::<Vec<_>>();
                unite(&mut result, &ch::paths_to_clipper(&paths, tolerance()))?;
            }
        }
    }
    Ok(result)
}
pub(super) fn via(via: &OwnedVia, offset: Length) -> Result<ClipperPaths> {
    let diameter = *via.size + offset * 2;
    if diameter <= Length::ZERO {
        return Ok(Vec::new());
    }
    Ok(ch::paths_to_clipper(
        &[Path::circle(PositiveLength::new(diameter)?).translated(via.via.position())],
        tolerance(),
    ))
}
pub(super) fn trace(trace: &Trace, offset: Length) -> Result<ClipperPaths> {
    let width = *trace.width + offset * 2;
    if width <= Length::ZERO {
        return Ok(Vec::new());
    }
    Ok(ch::paths_to_clipper(
        &[Path::obround_line(
            trace.p1,
            trace.p2,
            PositiveLength::new(width)?,
        )],
        tolerance(),
    ))
}
pub(super) fn plane(plane: &Plane, offset: Length) -> Result<ClipperPaths> {
    let mut result = Vec::new();
    for path in &plane.fragments {
        ch::unite_with(
            &mut result,
            &ch::paths_to_clipper(std::slice::from_ref(path), tolerance()),
            EvenOdd,
            EvenOdd,
        )?;
    }
    if offset != Length::ZERO {
        ch::offset(&mut result, offset, tolerance(), JoinType::Round)?;
    }
    Ok(result)
}
pub(super) fn hole(path: &Path, diameter: PositiveLength, offset: Length) -> Result<ClipperPaths> {
    let width = PositiveLength::new((*diameter + offset * 2).max(Length::new(1)))?;
    let mut result = Vec::new();
    unite(
        &mut result,
        &ch::paths_to_clipper(&path.to_outline_strokes(width), tolerance()),
    )?;
    Ok(result)
}

pub(super) fn copper(data: &BoardDrcData, layer: Layer) -> Result<ClipperPaths> {
    let p = &data.physical;
    let mut result = Vec::new();
    for d in p
        .drawings
        .iter()
        .filter(|d| d.reference.device().is_none() && d.layer == layer)
    {
        unite(&mut result, &drawing(d, Length::ZERO)?)?;
    }
    for t in p
        .texts
        .iter()
        .filter(|t| t.reference.device().is_none() && t.layer == layer)
    {
        unite(&mut result, &text(t, Length::ZERO)?)?;
    }
    for plane_ in p
        .planes
        .iter()
        .filter(|p| p.layer == layer && data.mode == super::DrcMode::Full)
    {
        unite(&mut result, &plane(plane_, Length::ZERO)?)?;
    }
    for device in p.devices.keys() {
        for d in p
            .drawings
            .iter()
            .filter(|d| d.reference.device() == Some(*device) && d.layer == layer)
        {
            unite(&mut result, &drawing(d, Length::ZERO)?)?;
        }
        for t in p
            .texts
            .iter()
            .filter(|t| t.reference.device() == Some(*device) && t.layer == layer)
        {
            unite(&mut result, &text(t, Length::ZERO)?)?;
        }
        for pad_ in p
            .pads
            .iter()
            .filter(|p| p.reference.device() == Some(*device))
        {
            unite(&mut result, &pad(pad_, layer, Length::ZERO, true)?)?;
        }
    }
    for pad_ in p.pads.iter().filter(|p| p.reference.device().is_none()) {
        unite(&mut result, &pad(pad_, layer, Length::ZERO, true)?)?;
    }
    for via_ in p.vias.iter().filter(|v| v.via.is_on_layer(layer)) {
        unite(&mut result, &via(via_, Length::ZERO)?)?;
    }
    for trace_ in p.traces.iter().filter(|t| t.layer == layer) {
        unite(&mut result, &trace(trace_, Length::ZERO)?)?;
    }
    Ok(result)
}
pub(super) fn stop_mask(data: &BoardDrcData, layer: Layer, offset: Length) -> Result<ClipperPaths> {
    let p = &data.physical;
    let mut result = Vec::new();
    for d in p.drawings.iter().filter(|d| d.layer == layer) {
        unite(&mut result, &drawing(d, offset)?)?;
    }
    for t in p.texts.iter().filter(|t| t.layer == layer) {
        unite(&mut result, &text(t, offset)?)?;
    }
    for (path, diameter) in &p.masks {
        unite(
            &mut result,
            &hole(path, PositiveLength::new(*diameter)?, offset)?,
        )?;
    }
    for pad_ in &p.pads {
        unite(&mut result, &pad(pad_, layer, offset, false)?)?;
    }
    for via in &p.vias {
        let diameter = if layer.is_top() {
            via.mask_top
        } else {
            via.mask_bottom
        };
        if let Some(d) = diameter {
            unite(
                &mut result,
                &hole(&NonEmptyPath::from_point(via.via.position()), d, offset)?,
            )?;
        }
    }
    Ok(result)
}
pub(super) fn copper_name(data: &BoardDrcData, object: DrcCopperRef, net: &str) -> String {
    use librepcb_i18n::tr;
    let prefix = if net.is_empty() {
        String::new()
    } else {
        format!("'{net}' ")
    };
    let name = match object {
        DrcCopperRef::Pad(reference) => {
            if let super::DrcPadRef::Footprint { device, .. } = reference {
                let name = data
                    .physical
                    .devices
                    .get(&device)
                    .map(|d| d.name.as_str())
                    .unwrap_or_default();
                let pad = data
                    .physical
                    .pads
                    .iter()
                    .find(|p| p.reference == reference)
                    .map(|p| p.name.as_str())
                    .unwrap_or_default();
                return if pad.is_empty() {
                    format!("'{name}'")
                } else {
                    format!("'{name}:{pad}'")
                };
            }
            tr!("DrcMsgCopperCopperClearanceViolation", "pad")
        }
        DrcCopperRef::Via { .. } => tr!("DrcMsgCopperCopperClearanceViolation", "via"),
        DrcCopperRef::Graphic(reference) => match reference {
            DrcObjectRef::Trace { .. } => tr!("DrcMsgCopperCopperClearanceViolation", "trace"),
            DrcObjectRef::Plane(_) => tr!("DrcMsgCopperCopperClearanceViolation", "plane"),
            DrcObjectRef::Polygon { .. } => tr!("DrcMsgCopperCopperClearanceViolation", "polygon"),
            DrcObjectRef::Circle { .. } => tr!("DrcMsgCopperCopperClearanceViolation", "circle"),
            DrcObjectRef::StrokeText { .. } => tr!("DrcMsgCopperCopperClearanceViolation", "text"),
        },
    };
    prefix + &name
}
