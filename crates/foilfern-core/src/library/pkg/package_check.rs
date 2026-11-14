//! Port of libs/librepcb/core/library/pkg/packagecheck.{h,cpp}.
//!
//! Upstream implements the geometric checks (pad clearances, annular rings,
//! legend clearance, pad origin) with `QPainterPath::intersects()` and
//! `contains()` on the pad outlines. Here the areas are built from
//! [`PadGeometry::to_outlines()`] / [`PadGeometry::to_hole_outlines()`] and
//! compared with Clipper, reproducing the curve approximations of the Qt
//! predicates (see [`check_area`](super::check_area)).

use std::collections::{BTreeSet, HashSet};

use super::check_area::{Area, Legend};
use super::error::Error;
use super::footprint::Footprint;
use super::footprint_pad::FootprintPad;
use super::package::{AssemblyType, Package};
use super::package_check_messages::{
    FootprintRef, PackageCheckMessage as Msg, PadRef, WidthObject,
};
use crate::geometry::{Pad, PadFunction, PadGeometry, PadShape, Path, ZoneLayers, ZoneRules};
use crate::library::{LibraryCheckMessage, LibraryElement, run_element_checks};
use crate::types::{Layer, Length, Point, PositiveLength, Tag};
use crate::utils::painter_path;

type MsgList = Vec<LibraryCheckMessage>;

/// Runs the package check, including the checks common to all elements
/// (upstream `PackageCheck::runChecks()`).
pub fn run_package_checks(package: &Package, msgs: &mut MsgList) -> Result<(), Error> {
    run_element_checks(package.element_metadata(), msgs);
    let checker = Checker { package };
    checker.check_assembly_type(msgs);
    checker.check_duplicate_pad_names(msgs);
    checker.check_missing_footprint(msgs);
    checker.check_ambiguous_footprint_tags(msgs);
    checker.check_missing_texts(msgs);
    checker.check_wrong_text_layers(msgs);
    checker.check_package_outlines(msgs);
    checker.check_courtyards(msgs);
    checker.check_origin_in_center(msgs);
    checker.check_pads_package_pad_uuid(msgs);
    checker.check_pads_clearance_to_pads(msgs)?;
    checker.check_pads_clearance_to_legend(msgs)?;
    checker.check_pads_annular_ring(msgs)?;
    checker.check_pads_connection_point(msgs)?;
    checker.check_custom_pad_outline(msgs);
    checker.check_stop_mask_on_pads(msgs);
    checker.check_solder_paste_on_pads(msgs);
    checker.check_copper_clearance_on_pads(msgs);
    checker.check_pad_functions(msgs);
    checker.check_holes_stop_mask(msgs);
    checker.check_line_widths(msgs);
    checker.check_zones(msgs);
    checker.check_footprint_models(msgs);
    Ok(())
}

/// Tolerance to avoid rounding issues (0.01 µm).
const TOLERANCE: Length = Length::new(10);

struct Checker<'a> {
    package: &'a Package,
}

fn footprint_ref(footprint: &Footprint) -> FootprintRef {
    FootprintRef {
        uuid: footprint.uuid(),
        name: footprint.names().default_value().clone(),
    }
}

fn push(msgs: &mut MsgList, msg: Msg) {
    msgs.push(msg.into());
}

/// Returns the area of the outlines of a pad geometry, placed like the pad
/// (upstream `Transform(pad).mapPx(geometry.toFilledQPainterPathPx())`).
fn placed_area(pad: &Pad, geometry: &PadGeometry) -> Result<Area, Error> {
    Ok(Area::from_outlines(
        &geometry.to_outlines()?,
        Some((pad.position(), pad.rotation())),
        false,
    ))
}

impl<'a> Checker<'a> {
    fn footprints(&self) -> impl Iterator<Item = &'a Footprint> + 'a {
        self.package.footprints().iter()
    }

    /// Returns the reference to a pad of a footprint, including the name of
    /// the connected package pad (empty if none).
    fn pad_ref(&self, footprint: &Footprint, pad: &FootprintPad) -> PadRef {
        let package_pad_name = pad
            .package_pad_uuid()
            .and_then(|uuid| self.package.pads().by_uuid(&uuid))
            .map(|p| p.name().to_string())
            .unwrap_or_default();
        PadRef {
            footprint: footprint_ref(footprint),
            pad: pad.uuid(),
            package_pad_name,
        }
    }

    /// Returns the reference to a pad for messages without package pad name.
    fn pad_ref_unnamed(&self, footprint: &Footprint, pad: &FootprintPad) -> PadRef {
        PadRef {
            footprint: footprint_ref(footprint),
            pad: pad.uuid(),
            package_pad_name: String::new(),
        }
    }

    /// Calls `f` for every pad of every footprint.
    fn for_each_pad(&self, mut f: impl FnMut(&'a Footprint, &'a FootprintPad)) {
        for footprint in self.footprints() {
            for pad in footprint.pads() {
                f(footprint, pad);
            }
        }
    }

    fn check_assembly_type(&self, msgs: &mut MsgList) {
        let assembly_type = self.package.assembly_type();
        // Check for deprecated assembly type.
        if assembly_type == AssemblyType::Auto {
            push(msgs, Msg::DeprecatedAssemblyType);
        }

        // Check if the assembly type looks reasonable (only possible if there
        // is at least one footprint).
        if !self.package.footprints().is_empty()
            && assembly_type != AssemblyType::Auto
            && assembly_type != self.package.guess_assembly_type()
        {
            push(msgs, Msg::SuspiciousAssemblyType);
        }
    }

    fn check_duplicate_pad_names(&self, msgs: &mut MsgList) {
        let mut names = HashSet::new();
        for pad in self.package.pads() {
            if !names.insert(pad.name()) {
                push(
                    msgs,
                    Msg::DuplicatePadName {
                        name: pad.name().clone(),
                    },
                );
            }
        }
    }

    fn check_missing_footprint(&self, msgs: &mut MsgList) {
        if self.package.footprints().is_empty() {
            push(msgs, Msg::MissingFootprint);
        }
    }

    fn check_ambiguous_footprint_tags(&self, msgs: &mut MsgList) {
        let mut tag_sets: HashSet<&BTreeSet<Tag>> = HashSet::new();
        for footprint in self.footprints() {
            if !tag_sets.insert(footprint.tags()) {
                push(msgs, Msg::AmbiguousFootprintTags);
                return;
            }
        }
    }

    fn check_missing_texts(&self, msgs: &mut MsgList) {
        for footprint in self.footprints() {
            let has_text = |text: &str| footprint.stroke_texts().iter().any(|t| t.text() == text);
            if !has_text("{{NAME}}") {
                push(msgs, Msg::MissingFootprintName(footprint_ref(footprint)));
            }
            if !has_text("{{VALUE}}") {
                push(msgs, Msg::MissingFootprintValue(footprint_ref(footprint)));
            }
        }
    }

    fn check_wrong_text_layers(&self, msgs: &mut MsgList) {
        for footprint in self.footprints() {
            for text in footprint.stroke_texts() {
                let expected_layer = match text.text().as_str() {
                    "{{NAME}}" => Layer::TOP_NAMES,
                    "{{VALUE}}" => Layer::TOP_VALUES,
                    _ => continue,
                };
                if text.layer() != expected_layer {
                    push(
                        msgs,
                        Msg::WrongFootprintTextLayer {
                            footprint: footprint_ref(footprint),
                            text: text.uuid(),
                            text_value: text.text().clone(),
                            expected_layer,
                        },
                    );
                }
            }
        }
    }

    /// Returns whether the footprint has a polygon or circle on a layer
    /// matching `filter`.
    fn has_layer(footprint: &Footprint, filter: impl Fn(Layer) -> bool) -> bool {
        footprint.polygons().iter().any(|p| filter(p.layer()))
            || footprint.circles().iter().any(|c| filter(c.layer()))
    }

    fn check_package_outlines(&self, msgs: &mut MsgList) {
        if self.package.assembly_type() != AssemblyType::None {
            for footprint in self.footprints() {
                if !Self::has_layer(footprint, Layer::is_package_outline) {
                    push(msgs, Msg::MissingPackageOutline(footprint_ref(footprint)));
                }
            }
        }
    }

    fn check_courtyards(&self, msgs: &mut MsgList) {
        if self.package.assembly_type() != AssemblyType::None {
            for footprint in self.footprints() {
                if !Self::has_layer(footprint, Layer::is_package_courtyard) {
                    push(msgs, Msg::MissingCourtyard(footprint_ref(footprint)));
                }
            }
        }
    }

    fn check_origin_in_center(&self, msgs: &mut MsgList) {
        /// Returns the corners (top left, bottom right) of the bounding
        /// rectangle of the closed path (upstream
        /// `path.toClosedPath().toQPainterPathPx().boundingRect()`).
        fn bounding_rect(path: &Path) -> (Point, Point) {
            let mut closed = path.clone();
            closed.close();
            let rect = painter_path::bounding_rect_px(&[closed]);
            // The rectangle is built from coordinates of valid points, so the
            // conversion back cannot overflow.
            let top_left = Point::from_px(rect.left(), rect.top()).unwrap_or_default();
            let bot_right = Point::from_px(rect.right(), rect.bottom()).unwrap_or_default();
            (top_left, bot_right)
        }

        for footprint in self.footprints() {
            // Specify a relative and an absolute tolerance.
            let mut tolerance_factor: f64 = 0.01;
            let mut tolerance_abs = Length::new(100_000);

            // Allow higher tolerance for THT parts.
            if self.package.assembly_type() != AssemblyType::Smt {
                tolerance_factor = 0.05;
                tolerance_abs = Length::new(635_000);
            }

            // Consider mainly the package outlines since this is the most
            // reliable source for this check.
            let mut x = BTreeSet::new();
            let mut y = BTreeSet::new();
            let add_rect =
                |x: &mut BTreeSet<Length>, y: &mut BTreeSet<Length>, p1: Point, p2: Point| {
                    x.insert(p1.x);
                    x.insert(p2.x);
                    y.insert(p1.y);
                    y.insert(p2.y);
                };
            let circle_rect = |center: Point, diameter: PositiveLength| {
                let r = *diameter / 2;
                (
                    Point::new(center.x - r, center.y - r),
                    Point::new(center.x + r, center.y + r),
                )
            };
            for c in footprint.circles() {
                if c.layer() == Layer::TOP_PACKAGE_OUTLINES {
                    let (p1, p2) = circle_rect(c.center(), c.diameter());
                    add_rect(&mut x, &mut y, p1, p2);
                }
            }
            for p in footprint.polygons() {
                if p.layer() != Layer::TOP_PACKAGE_OUTLINES {
                    continue;
                }
                if p.path().is_curved() {
                    let (p1, p2) = bounding_rect(p.path());
                    add_rect(&mut x, &mut y, p1, p2);
                    // Relax check due to complex shape.
                    tolerance_factor = tolerance_factor.max(0.15);
                } else {
                    for v in p.path().vertices() {
                        add_rect(&mut x, &mut y, v.pos, v.pos);
                    }
                    let mut path = p.path().clone();
                    path.open();
                    path.clean();
                    if path.vertices().len() != 4 {
                        // Relax check due to complex shape.
                        tolerance_factor = tolerance_factor.max(0.1);
                    }
                }
            }

            // Only if we didn't find a package body, take more objects into
            // account but allow much larger tolerance.
            if x.is_empty() || y.is_empty() {
                tolerance_factor = tolerance_factor.max(0.3);
                for pad in footprint.pads() {
                    let pos = pad.pad().position();
                    add_rect(&mut x, &mut y, pos, pos);
                }
                for c in footprint.circles() {
                    if c.layer() == Layer::TOP_DOCUMENTATION {
                        let (p1, p2) = circle_rect(c.center(), c.diameter());
                        add_rect(&mut x, &mut y, p1, p2);
                    }
                }
                for p in footprint.polygons() {
                    if p.layer() == Layer::TOP_DOCUMENTATION {
                        let (p1, p2) = bounding_rect(p.path());
                        add_rect(&mut x, &mut y, p1, p2);
                    }
                }
            }

            // If there is no boundary, abort.
            let (Some(&min_x), Some(&max_x), Some(&min_y), Some(&max_y)) =
                (x.first(), x.last(), y.first(), y.last())
            else {
                continue;
            };

            // Calculate and check center.
            let center = Point::new((min_x + max_x) / 2, (min_y + max_y) / 2);
            let width = max_x - min_x;
            let height = max_y - min_y;
            let tol_x = width.scaled(tolerance_factor).max(tolerance_abs);
            let tol_y = height.scaled(tolerance_factor).max(tolerance_abs);
            if center.x.abs() > tol_x || center.y.abs() > tol_y {
                push(
                    msgs,
                    Msg::FootprintOriginNotInCenter {
                        footprint: footprint_ref(footprint),
                        center,
                    },
                );
            }
        }
    }

    fn check_pads_package_pad_uuid(&self, msgs: &mut MsgList) {
        self.for_each_pad(|footprint, pad| {
            if let Some(uuid) = pad.package_pad_uuid()
                && !self.package.pads().contains_uuid(&uuid)
            {
                push(
                    msgs,
                    Msg::InvalidPadConnection(self.pad_ref_unnamed(footprint, pad)),
                );
            }
        });
    }

    fn check_pads_clearance_to_pads(&self, msgs: &mut MsgList) -> Result<(), Error> {
        let clearance = *self.package.min_copper_clearance();

        // Copper area and clearance area of a pad, in footprint coordinates.
        let areas = |pad: &Pad| -> Result<(Area, Area), Error> {
            let pad_clearance = clearance.max(*pad.copper_clearance()) - TOLERANCE;
            let geometry = pad.geometry();
            Ok((
                placed_area(pad, &geometry)?,
                placed_area(pad, &geometry.with_offset(pad_clearance))?,
            ))
        };

        // Check all footprints.
        for footprint in self.footprints() {
            let pads: Vec<&FootprintPad> = footprint.pads().iter().collect();
            let pad_areas = pads
                .iter()
                .map(|pad| areas(pad.pad()))
                .collect::<Result<Vec<_>, _>>()?;

            // Compare with all pads *after* pad1 to avoid duplicate messages!
            for (i, (pad1, (copper1, clearance1))) in pads.iter().zip(&pad_areas).enumerate() {
                for (pad2, (copper2, clearance2)) in pads.iter().zip(&pad_areas).skip(i + 1) {
                    let (p1, p2) = (pad1.pad(), pad2.pad());
                    // Only warn if both pads have copper on the same board
                    // side.
                    if !(p1.component_side() == p2.component_side() || p1.is_tht() || p2.is_tht()) {
                        continue;
                    }
                    // Only warn if both pads have different net signal, or
                    // one of them is unconnected (an unconnected pad is
                    // considered as a different net signal).
                    let (net1, net2) = (pad1.package_pad_uuid(), pad2.package_pad_uuid());
                    if !(net1 != net2 || net1.is_none() || net2.is_none()) {
                        continue;
                    }
                    // Now check if the clearance is really too small.
                    if copper1.intersects(copper2) {
                        push(
                            msgs,
                            Msg::OverlappingPads {
                                pad1: self.pad_ref(footprint, pad1),
                                pad2: self.pad_ref(footprint, pad2),
                            },
                        );
                    } else if clearance1.intersects(copper2) || copper1.intersects(clearance2) {
                        push(
                            msgs,
                            Msg::PadClearanceViolation {
                                pad1: self.pad_ref(footprint, pad1),
                                pad2: self.pad_ref(footprint, pad2),
                                clearance,
                            },
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn check_pads_clearance_to_legend(&self, msgs: &mut MsgList) -> Result<(), Error> {
        let clearance = Length::new(150_000); // 150 µm
        for footprint in self.footprints() {
            let mut top_legend = Legend::default();
            let mut bot_legend = Legend::default();
            for polygon in footprint.polygons() {
                let legend = if polygon.layer() == Layer::TOP_LEGEND {
                    &mut top_legend
                } else if polygon.layer() == Layer::BOT_LEGEND {
                    &mut bot_legend
                } else {
                    continue;
                };
                legend.add_polygon(
                    polygon.path(),
                    polygon.line_width(),
                    polygon.is_filled() && polygon.path().is_closed(),
                );
            }

            for pad in footprint.pads() {
                let p = pad.pad();
                let stop_mask = placed_area(p, &p.geometry().with_offset(clearance - TOLERANCE))?;
                if (p.is_on_layer(Layer::TOP_COPPER) && top_legend.intersects(&stop_mask))
                    || (p.is_on_layer(Layer::BOT_COPPER) && bot_legend.intersects(&stop_mask))
                {
                    push(
                        msgs,
                        Msg::PadOverlapsWithLegend {
                            pad: self.pad_ref(footprint, pad),
                            clearance,
                        },
                    );
                }
            }
        }
        Ok(())
    }

    fn check_pads_annular_ring(&self, msgs: &mut MsgList) -> Result<(), Error> {
        let annular_ring = Length::new(150_000); // 150 µm
        let annular_width =
            PositiveLength::new(annular_ring * 2 - TOLERANCE).expect("constant is positive");

        for footprint in self.footprints() {
            for pad in footprint.pads() {
                let p = pad.pad();
                let pad_area = Area::from_outlines(&p.geometry().to_outlines()?, None, false);
                // Upstream unites the outlines of a hole (`Path::
                // toQPainterPathPx(paths, true)`); a single outline keeps
                // its odd-even fill, the union of multiple outlines is
                // emulated with winding fill.
                let hole_area = |outlines: Vec<Path>| {
                    let winding = outlines.len() > 1;
                    Area::from_outlines(&outlines, None, winding)
                };
                let holes: Vec<(Area, Area)> = p
                    .holes()
                    .iter()
                    .map(|hole| {
                        let path = hole.path();
                        let diameter = hole.diameter();
                        (
                            hole_area(path.to_outline_strokes(diameter)),
                            hole_area(path.to_outline_strokes(diameter + annular_width)),
                        )
                    })
                    .collect();

                // Check all holes.
                let mut emit_error = false;
                let mut emit_warning = false;
                for (i, (hole1, hole1_with_annular)) in holes.iter().enumerate() {
                    // Check annular rings.
                    if !pad_area.contains(hole1) {
                        emit_error = true;
                    } else if !pad_area.contains(hole1_with_annular) {
                        emit_warning = true;
                    } else {
                        // Compare with all holes *after* hole1 to avoid
                        // redundant checks.
                        for (hole2, _) in holes.iter().skip(i + 1) {
                            // Now check if the annular ring is really too
                            // small.
                            if hole1.intersects(hole2) {
                                emit_error = true;
                            } else if hole1_with_annular.intersects(hole2) {
                                emit_warning = true;
                            }
                        }
                    }
                }

                // Only show one message even if there are multiple violations.
                if emit_error {
                    push(
                        msgs,
                        Msg::PadHoleOutsideCopper(self.pad_ref(footprint, pad)),
                    );
                } else if emit_warning {
                    push(
                        msgs,
                        Msg::PadAnnularRingViolation {
                            pad: self.pad_ref(footprint, pad),
                            annular_ring,
                        },
                    );
                }
            }
        }
        Ok(())
    }

    fn check_pads_connection_point(&self, msgs: &mut MsgList) -> Result<(), Error> {
        for footprint in self.footprints() {
            for pad in footprint.pads() {
                let p = pad.pad();
                let geometry = p.geometry();
                // THT: hole outlines with winding fill, SMT: pad outlines with
                // odd-even fill.
                let allowed = if p.is_tht() {
                    Area::from_outlines(&geometry.to_hole_outlines(), None, true).contains_origin()
                } else {
                    Area::from_outlines(&geometry.to_outlines()?, None, false).contains_origin()
                };
                if !allowed {
                    push(
                        msgs,
                        Msg::PadOriginOutsideCopper(self.pad_ref(footprint, pad)),
                    );
                }
            }
        }
        Ok(())
    }

    fn check_custom_pad_outline(&self, msgs: &mut MsgList) {
        self.for_each_pad(|footprint, pad| {
            let p = pad.pad();
            if p.shape() == PadShape::Custom
                && !PadGeometry::is_valid_custom_outline(p.custom_shape_outline())
            {
                push(
                    msgs,
                    Msg::InvalidCustomPadOutline(self.pad_ref(footprint, pad)),
                );
            } else if p.shape() != PadShape::Custom
                && !p.custom_shape_outline().vertices().is_empty()
            {
                push(
                    msgs,
                    Msg::UnusedCustomPadOutline(self.pad_ref(footprint, pad)),
                );
            }
        });
    }

    fn check_stop_mask_on_pads(&self, msgs: &mut MsgList) {
        self.for_each_pad(|footprint, pad| {
            let p = pad.pad();
            if !p.stop_mask_config().is_enabled() {
                push(msgs, Msg::PadStopMaskOff(self.pad_ref(footprint, pad)));
            } else if p.function_is_fiducial() && p.stop_mask_config().offset().is_none() {
                push(
                    msgs,
                    Msg::FiducialStopMaskNotSet(self.pad_ref_unnamed(footprint, pad)),
                );
            }
        });
    }

    fn check_solder_paste_on_pads(&self, msgs: &mut MsgList) {
        self.for_each_pad(|footprint, pad| {
            let p = pad.pad();
            let paste = p.solder_paste_config().is_enabled();
            let msg = if !p.is_tht() && p.function_needs_soldering() && !paste {
                Msg::SmtPadWithoutSolderPaste
            } else if !p.is_tht() && !p.function_needs_soldering() && paste {
                Msg::SmtPadWithSolderPaste
            } else if p.is_tht() && paste {
                Msg::ThtPadWithSolderPaste
            } else {
                return;
            };
            push(msgs, msg(self.pad_ref(footprint, pad)));
        });
    }

    fn check_copper_clearance_on_pads(&self, msgs: &mut MsgList) {
        self.for_each_pad(|footprint, pad| {
            let p = pad.pad();
            let stop_mask_offset = p.stop_mask_config().offset();
            if !p.function_is_fiducial() && *p.copper_clearance() > Length::ZERO {
                push(
                    msgs,
                    Msg::PadWithCopperClearance(self.pad_ref(footprint, pad)),
                );
            } else if p.function_is_fiducial()
                && stop_mask_offset.is_some_and(|offset| *p.copper_clearance() < offset)
            {
                push(
                    msgs,
                    Msg::FiducialClearanceLessThanStopMask(self.pad_ref_unnamed(footprint, pad)),
                );
            }
        });
    }

    fn check_pad_functions(&self, msgs: &mut MsgList) {
        let tht_funcs = [PadFunction::StandardPad, PadFunction::PressFitPad];
        let smt_funcs = [
            PadFunction::StandardPad,
            PadFunction::ThermalPad,
            PadFunction::BgaPad,
            PadFunction::EdgeConnectorPad,
            PadFunction::TestPad,
            PadFunction::LocalFiducial,
            PadFunction::GlobalFiducial,
        ];
        self.for_each_pad(|footprint, pad| {
            let p = pad.pad();
            let is_tht = p.is_tht();
            let is_connected = pad.package_pad_uuid().is_some();
            if p.function() == PadFunction::Unspecified {
                push(
                    msgs,
                    Msg::UnspecifiedPadFunction(self.pad_ref(footprint, pad)),
                );
            } else if (is_tht && !tht_funcs.contains(&p.function()))
                || (!is_tht && !smt_funcs.contains(&p.function()))
                || (is_connected == p.function_is_fiducial())
            {
                push(
                    msgs,
                    Msg::SuspiciousPadFunction(self.pad_ref(footprint, pad)),
                );
            }
        });
    }

    fn check_holes_stop_mask(&self, msgs: &mut MsgList) {
        for footprint in self.footprints() {
            for hole in footprint.holes() {
                if !hole.stop_mask_config().is_enabled() {
                    push(
                        msgs,
                        Msg::HoleWithoutStopMask {
                            footprint: footprint_ref(footprint),
                            hole: hole.uuid(),
                            diameter: hole.diameter(),
                        },
                    );
                }
            }
        }
    }

    fn check_line_widths(&self, msgs: &mut MsgList) {
        let silkscreen_layers = [
            Layer::TOP_LEGEND,
            Layer::BOT_LEGEND,
            Layer::TOP_NAMES,
            Layer::BOT_NAMES,
            Layer::TOP_VALUES,
            Layer::BOT_VALUES,
        ];
        // See BoardDesignRuleCheckSettings.
        let min_width = Length::new(150_000);
        for footprint in self.footprints() {
            let mut violation = |object: WidthObject, layer: Layer| {
                push(
                    msgs,
                    Msg::MinimumWidthViolation {
                        footprint: footprint_ref(footprint),
                        object,
                        layer,
                        min_width,
                    },
                );
            };
            for polygon in footprint.polygons() {
                if silkscreen_layers.contains(&polygon.layer())
                    && *polygon.line_width() < min_width
                    && !polygon.is_filled()
                {
                    violation(WidthObject::Polygon(polygon.uuid()), polygon.layer());
                }
            }
            for circle in footprint.circles() {
                if silkscreen_layers.contains(&circle.layer())
                    && *circle.line_width() < min_width
                    && !circle.is_filled()
                {
                    violation(WidthObject::Circle(circle.uuid()), circle.layer());
                }
            }
            for text in footprint.stroke_texts() {
                if silkscreen_layers.contains(&text.layer()) && *text.stroke_width() < min_width {
                    violation(WidthObject::StrokeText(text.uuid()), text.layer());
                }
            }
        }
    }

    fn check_zones(&self, msgs: &mut MsgList) {
        for footprint in self.footprints() {
            for zone in footprint.zones() {
                let (layers, rules) = (zone.layers(), zone.rules());
                if layers.is_empty()
                    || rules.is_empty()
                    || (layers == ZoneLayers::INNER
                        && !rules.contains(ZoneRules::NO_COPPER)
                        && !rules.contains(ZoneRules::NO_PLANES))
                {
                    push(
                        msgs,
                        Msg::UselessZone {
                            footprint: footprint_ref(footprint),
                            zone: zone.uuid(),
                        },
                    );
                }
            }
        }
    }

    fn check_footprint_models(&self, msgs: &mut MsgList) {
        if self.package.resolved_assembly_type() != AssemblyType::None {
            let package_models: HashSet<_> =
                self.package.models().iter().map(|m| m.uuid()).collect();
            for footprint in self.footprints() {
                if !footprint
                    .models()
                    .iter()
                    .any(|uuid| package_models.contains(uuid))
                {
                    push(msgs, Msg::MissingFootprintModel(footprint_ref(footprint)));
                }
            }
        }
    }
}
