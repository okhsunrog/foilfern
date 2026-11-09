//! Automatic fixes of library check messages: port of the `autoFix()`
//! specializations of libs/librepcb/editor/library/{sym/symboltab,
//! pkg/packagetab,cmp/componenttab,dev/devicetab}.cpp.
//!
//! [`LibraryElementEditor::run_checks()`](super::LibraryElementEditor::run_checks)
//! returns the messages; `can_auto_fix()` tells whether a message has a
//! fix and `auto_fix()` applies it as an undo group. Fixes which need the
//! user (upstream dialogs, menus or switching to a tool) take their answer
//! from [`FixParams`] or return a [`FixRequest`] for the application.

use std::collections::BTreeSet;

use librepcb_core::geometry::{PadFunction, PadShape, Path};
use librepcb_core::library::cmp::{Component, ComponentCheckMessage, ComponentSymbolVariant};
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::{Package, PackageCheckMessage, WidthObject};
use librepcb_core::library::sym::{Symbol, SymbolCheckMessage};
use librepcb_core::library::{
    LibraryBaseElementCheckMessage, LibraryCheckMessage, LibraryElementCheckMessage,
    title_case_fixed_name,
};
use librepcb_core::types::{
    CircuitIdentifier, ElementName, MaskConfig, PositiveLength, UnsignedLength, Uuid,
};
use librepcb_i18n::tr;

use super::commands::{
    AddFootprint, EditElementMetadata, EditPackage, EditSymbolPin, FootprintObject,
    GenerateCourtyard, GeneratePackageOutline, SymbolObject, TransformFootprintItems, TransformOp,
    TransformSymbolItems, UpdateFootprintObject, UpdateSymbolObject, all_footprint_items,
    all_symbol_items,
};
use super::{EditableElement, LibraryElementEditor};
use crate::error::{Error, Result};
use crate::fsm::library::LibraryTool;

/// Answers to questions a fix may ask (upstream: dialogs and menus).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixParams {
    /// The user name of the workspace settings (missing author).
    pub author: Option<String>,
    /// The new line width (minimum width violations; upstream dialog).
    pub line_width: Option<UnsignedLength>,
    /// The courtyard excess (missing courtyard).
    pub courtyard_offset: Option<PositiveLength>,
    /// The pad function and whether to apply it to all unspecified pads
    /// (unspecified/suspicious pad function; upstream menu).
    pub pad_function: Option<(PadFunction, bool)>,
    /// Whether the component is a manufacturer specific one (missing
    /// default value; upstream question).
    pub specific_component: Option<bool>,
}

/// What the application has to do to fix a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixRequest {
    /// Ask for the missing parameter (see [`FixParams`]) and call
    /// `auto_fix()` again.
    Parameter(FixParam),
    /// Let the user choose categories (upstream: metadata page with the
    /// category chooser).
    ChooseCategories,
    /// Start a tool of the FSM, in `footprint` for packages (upstream: e.g.
    /// "add names" for a missing `{{NAME}}` text).
    StartTool {
        /// The footprint (packages).
        footprint: Option<Uuid>,
        /// The tool.
        tool: LibraryTool,
    },
    /// Show the 3D models of a footprint (missing 3D model).
    ShowModels {
        /// The footprint.
        footprint: Uuid,
    },
}

/// A parameter a fix needs (see [`FixParams`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixParam {
    /// [`FixParams::author`].
    Author,
    /// [`FixParams::line_width`].
    LineWidth,
    /// [`FixParams::courtyard_offset`].
    CourtyardOffset,
    /// [`FixParams::pad_function`].
    PadFunction,
    /// [`FixParams::specific_component`].
    SpecificComponent,
}

/// The result of `auto_fix()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixOutcome {
    /// The fix was applied (one undo group).
    Applied,
    /// The message has no automatic fix (or the fix is not possible).
    NotFixable,
    /// The application has to do something (dialog, tool).
    Request(FixRequest),
}

/// Fixes of the messages common to all elements.
fn fix_base<E: EditableElement>(
    editor: &mut LibraryElementEditor<E>,
    msg: &LibraryCheckMessage,
    params: &FixParams,
) -> Result<Option<FixOutcome>> {
    Ok(Some(match msg {
        LibraryCheckMessage::BaseElement(LibraryBaseElementCheckMessage::NameNotTitleCase {
            name,
        }) => {
            editor.execute(EditElementMetadata {
                name: Some(title_case_fixed_name(name)),
                ..EditElementMetadata::default()
            })?;
            FixOutcome::Applied
        }
        LibraryCheckMessage::BaseElement(LibraryBaseElementCheckMessage::MissingAuthor) => {
            match params.author.as_ref().filter(|a| !a.is_empty()) {
                Some(author) => {
                    editor.execute(EditElementMetadata {
                        author: Some(author.clone()),
                        ..EditElementMetadata::default()
                    })?;
                    FixOutcome::Applied
                }
                None => FixOutcome::Request(FixRequest::Parameter(FixParam::Author)),
            }
        }
        LibraryCheckMessage::Element(LibraryElementCheckMessage::MissingCategories) => {
            FixOutcome::Request(FixRequest::ChooseCategories)
        }
        _ => return Ok(None),
    }))
}

fn is_base_fixable(msg: &LibraryCheckMessage) -> bool {
    matches!(
        msg,
        LibraryCheckMessage::BaseElement(_)
            | LibraryCheckMessage::Element(LibraryElementCheckMessage::MissingCategories)
    )
}

impl LibraryElementEditor<Symbol> {
    /// Whether a message has an automatic fix (upstream
    /// `SymbolTab::autoFixImpl(checkOnly = true)`).
    pub fn can_auto_fix(msg: &LibraryCheckMessage) -> bool {
        is_base_fixable(msg)
            || matches!(
                msg,
                LibraryCheckMessage::Symbol(
                    SymbolCheckMessage::MissingSymbolName
                        | SymbolCheckMessage::MissingSymbolValue
                        | SymbolCheckMessage::WrongTextLayer { .. }
                        | SymbolCheckMessage::PinNotOnGrid { .. }
                        | SymbolCheckMessage::NonFunctionalPinInversionSign { .. }
                        | SymbolCheckMessage::OriginNotInCenter { .. }
                )
            )
    }

    /// Fixes a message (upstream `SymbolTab::autoFix()`).
    pub fn auto_fix(
        &mut self,
        msg: &LibraryCheckMessage,
        params: &FixParams,
    ) -> Result<FixOutcome> {
        if let Some(outcome) = fix_base(self, msg, params)? {
            return Ok(outcome);
        }
        let LibraryCheckMessage::Symbol(msg) = msg else {
            return Ok(FixOutcome::NotFixable);
        };
        match msg {
            SymbolCheckMessage::MissingSymbolName => {
                Ok(FixOutcome::Request(FixRequest::StartTool {
                    footprint: None,
                    tool: LibraryTool::AddNames,
                }))
            }
            SymbolCheckMessage::MissingSymbolValue => {
                Ok(FixOutcome::Request(FixRequest::StartTool {
                    footprint: None,
                    tool: LibraryTool::AddValues,
                }))
            }
            SymbolCheckMessage::WrongTextLayer {
                text,
                expected_layer,
            } => {
                let mut text = self
                    .element()
                    .texts()
                    .by_uuid(&text.uuid())
                    .ok_or_else(|| Error::not_found("text", text.uuid()))?
                    .clone();
                text.set_layer(*expected_layer);
                self.execute(UpdateSymbolObject(SymbolObject::Text(text)))?;
                Ok(FixOutcome::Applied)
            }
            SymbolCheckMessage::PinNotOnGrid { pin, grid_interval } => {
                let current = self
                    .element()
                    .pins()
                    .by_uuid(&pin.uuid())
                    .ok_or_else(|| Error::not_found("pin", pin.uuid()))?;
                self.execute(EditSymbolPin {
                    position: Some(current.position().mapped_to_grid(*grid_interval)),
                    ..EditSymbolPin::new(pin.uuid())
                })?;
                Ok(FixOutcome::Applied)
            }
            SymbolCheckMessage::NonFunctionalPinInversionSign { pin } => {
                let current = self
                    .element()
                    .pins()
                    .by_uuid(&pin.uuid())
                    .ok_or_else(|| Error::not_found("pin", pin.uuid()))?;
                let name = format!(
                    "!{}",
                    current.name().as_str().chars().skip(1).collect::<String>()
                );
                self.execute(EditSymbolPin {
                    name: Some(CircuitIdentifier::new(name)?),
                    ..EditSymbolPin::new(pin.uuid())
                })?;
                Ok(FixOutcome::Applied)
            }
            SymbolCheckMessage::OriginNotInCenter { center } => {
                // Upstream: select all, move by -center (snapped to grid).
                let grid = self.element().grid_interval();
                let items = all_symbol_items(self.element());
                self.execute(TransformSymbolItems {
                    items,
                    grid,
                    ops: vec![TransformOp::Translate(-center.mapped_to_grid(grid))],
                })?;
                Ok(FixOutcome::Applied)
            }
            _ => Ok(FixOutcome::NotFixable),
        }
    }
}

impl LibraryElementEditor<Package> {
    /// Whether a message has an automatic fix (upstream
    /// `PackageTab::autoFixImpl(checkOnly = true)`).
    pub fn can_auto_fix(msg: &LibraryCheckMessage) -> bool {
        use PackageCheckMessage as M;
        is_base_fixable(msg)
            || matches!(
                msg,
                LibraryCheckMessage::Package(
                    M::DeprecatedAssemblyType
                        | M::SuspiciousAssemblyType
                        | M::MissingPackageOutline(_)
                        | M::MissingCourtyard(_)
                        | M::MinimumWidthViolation { .. }
                        | M::MissingFootprint
                        | M::MissingFootprintModel(_)
                        | M::MissingFootprintName(_)
                        | M::MissingFootprintValue(_)
                        | M::FootprintOriginNotInCenter { .. }
                        | M::WrongFootprintTextLayer { .. }
                        | M::UnusedCustomPadOutline(_)
                        | M::InvalidCustomPadOutline(_)
                        | M::PadStopMaskOff(_)
                        | M::SmtPadWithSolderPaste(_)
                        | M::ThtPadWithSolderPaste(_)
                        | M::PadWithCopperClearance(_)
                        | M::FiducialClearanceLessThanStopMask(_)
                        | M::HoleWithoutStopMask { .. }
                        | M::UnspecifiedPadFunction(_)
                        | M::SuspiciousPadFunction(_)
                )
            )
    }

    /// Replaces a pad of a footprint after modifying it with `f`.
    fn fix_pad(
        &mut self,
        footprint: Uuid,
        pad: Uuid,
        f: impl FnOnce(&mut librepcb_core::library::pkg::FootprintPad),
    ) -> Result<FixOutcome> {
        let mut obj = self
            .element()
            .footprints()
            .by_uuid(&footprint)
            .and_then(|f| f.pads().by_uuid(&pad))
            .ok_or_else(|| Error::not_found("pad", pad))?
            .clone();
        f(&mut obj);
        self.execute(UpdateFootprintObject {
            footprint,
            object: FootprintObject::Pad(obj),
        })?;
        Ok(FixOutcome::Applied)
    }

    /// Fixes a message (upstream `PackageTab::autoFix()`).
    pub fn auto_fix(
        &mut self,
        msg: &LibraryCheckMessage,
        params: &FixParams,
    ) -> Result<FixOutcome> {
        use PackageCheckMessage as M;
        if let Some(outcome) = fix_base(self, msg, params)? {
            return Ok(outcome);
        }
        let LibraryCheckMessage::Package(msg) = msg else {
            return Ok(FixOutcome::NotFixable);
        };
        match msg {
            M::DeprecatedAssemblyType | M::SuspiciousAssemblyType => {
                let guessed = self.element().guess_assembly_type();
                self.execute(EditPackage {
                    assembly_type: Some(guessed),
                    ..EditPackage::default()
                })?;
                Ok(FixOutcome::Applied)
            }
            M::MissingPackageOutline(f) => Ok(
                if self.execute(GeneratePackageOutline { footprint: f.uuid })? {
                    FixOutcome::Applied
                } else {
                    FixOutcome::NotFixable
                },
            ),
            M::MissingCourtyard(f) => match params.courtyard_offset {
                Some(offset) => Ok(
                    if self.execute(GenerateCourtyard {
                        footprint: f.uuid,
                        offset,
                    })? {
                        FixOutcome::Applied
                    } else {
                        FixOutcome::NotFixable
                    },
                ),
                None => Ok(FixOutcome::Request(FixRequest::Parameter(
                    FixParam::CourtyardOffset,
                ))),
            },
            M::MinimumWidthViolation {
                footprint, object, ..
            } => {
                let Some(width) = params.line_width else {
                    return Ok(FixOutcome::Request(FixRequest::Parameter(
                        FixParam::LineWidth,
                    )));
                };
                let fpt = self
                    .element()
                    .footprints()
                    .by_uuid(&footprint.uuid)
                    .ok_or_else(|| Error::not_found("footprint", footprint.uuid))?;
                let object = match object {
                    WidthObject::Polygon(u) => fpt.polygons().by_uuid(u).map(|p| {
                        let mut p = p.clone();
                        p.set_line_width(width);
                        FootprintObject::Polygon(p)
                    }),
                    WidthObject::Circle(u) => fpt.circles().by_uuid(u).map(|c| {
                        let mut c = c.clone();
                        c.set_line_width(width);
                        FootprintObject::Circle(c)
                    }),
                    WidthObject::StrokeText(u) => fpt.stroke_texts().by_uuid(u).map(|t| {
                        let mut t = t.clone();
                        t.set_stroke_width(width);
                        FootprintObject::StrokeText(t)
                    }),
                };
                let object = object.ok_or_else(|| Error::not_found("object", footprint.uuid))?;
                self.execute(UpdateFootprintObject {
                    footprint: footprint.uuid,
                    object,
                })?;
                Ok(FixOutcome::Applied)
            }
            M::MissingFootprint => {
                self.execute(AddFootprint {
                    name: ElementName::new("default")?,
                })?;
                Ok(FixOutcome::Applied)
            }
            M::MissingFootprintModel(f) => Ok(FixOutcome::Request(FixRequest::ShowModels {
                footprint: f.uuid,
            })),
            M::MissingFootprintName(f) => Ok(FixOutcome::Request(FixRequest::StartTool {
                footprint: Some(f.uuid),
                tool: LibraryTool::AddNames,
            })),
            M::MissingFootprintValue(f) => Ok(FixOutcome::Request(FixRequest::StartTool {
                footprint: Some(f.uuid),
                tool: LibraryTool::AddValues,
            })),
            M::FootprintOriginNotInCenter { footprint, center } => {
                // Upstream: select all, move by -center.
                let fpt = self
                    .element()
                    .footprints()
                    .by_uuid(&footprint.uuid)
                    .ok_or_else(|| Error::not_found("footprint", footprint.uuid))?;
                let items = all_footprint_items(fpt);
                let grid = self.element().grid_interval();
                self.execute(TransformFootprintItems {
                    footprint: footprint.uuid,
                    items,
                    grid,
                    ops: vec![TransformOp::Translate(-*center)],
                })?;
                Ok(FixOutcome::Applied)
            }
            M::WrongFootprintTextLayer {
                footprint,
                text,
                expected_layer,
                ..
            } => {
                let mut t = self
                    .element()
                    .footprints()
                    .by_uuid(&footprint.uuid)
                    .and_then(|f| f.stroke_texts().by_uuid(text))
                    .ok_or_else(|| Error::not_found("stroke text", text))?
                    .clone();
                t.set_layer(*expected_layer);
                self.execute(UpdateFootprintObject {
                    footprint: footprint.uuid,
                    object: FootprintObject::StrokeText(t),
                })?;
                Ok(FixOutcome::Applied)
            }
            M::UnusedCustomPadOutline(p) => self.fix_pad(p.footprint.uuid, p.pad, |pad| {
                pad.pad_mut()
                    .set_custom_shape_outline(Path::new(Vec::new()));
            }),
            M::InvalidCustomPadOutline(p) => self.fix_pad(p.footprint.uuid, p.pad, |pad| {
                pad.pad_mut().set_shape(PadShape::RoundedRect);
            }),
            M::PadStopMaskOff(p) => self.fix_pad(p.footprint.uuid, p.pad, |pad| {
                pad.pad_mut().set_stop_mask_config(MaskConfig::Automatic);
            }),
            M::SmtPadWithSolderPaste(p) | M::ThtPadWithSolderPaste(p) => {
                self.fix_pad(p.footprint.uuid, p.pad, |pad| {
                    pad.pad_mut().set_solder_paste_config(MaskConfig::Off);
                })
            }
            M::PadWithCopperClearance(p) => self.fix_pad(p.footprint.uuid, p.pad, |pad| {
                pad.pad_mut()
                    .set_copper_clearance(UnsignedLength::default());
            }),
            M::FiducialClearanceLessThanStopMask(p) => {
                let offset = self
                    .element()
                    .footprints()
                    .by_uuid(&p.footprint.uuid)
                    .and_then(|f| f.pads().by_uuid(&p.pad))
                    .and_then(|pad| pad.pad().stop_mask_config().offset())
                    .and_then(|o| UnsignedLength::new(o).ok())
                    .filter(|o| **o > librepcb_core::types::Length::ZERO);
                match offset {
                    Some(offset) => self.fix_pad(p.footprint.uuid, p.pad, |pad| {
                        pad.pad_mut().set_copper_clearance(offset);
                    }),
                    None => Ok(FixOutcome::NotFixable),
                }
            }
            M::HoleWithoutStopMask {
                footprint, hole, ..
            } => {
                let mut h = self
                    .element()
                    .footprints()
                    .by_uuid(&footprint.uuid)
                    .and_then(|f| f.holes().by_uuid(hole))
                    .ok_or_else(|| Error::not_found("hole", hole))?
                    .clone();
                h.set_stop_mask_config(MaskConfig::Automatic);
                self.execute(UpdateFootprintObject {
                    footprint: footprint.uuid,
                    object: FootprintObject::Hole(h),
                })?;
                Ok(FixOutcome::Applied)
            }
            M::UnspecifiedPadFunction(p) | M::SuspiciousPadFunction(p) => {
                let Some((function, all)) = params.pad_function else {
                    return Ok(FixOutcome::Request(FixRequest::Parameter(
                        FixParam::PadFunction,
                    )));
                };
                if all {
                    let text = tr!(
                        "librepcb::editor::PackageTab",
                        "Fix Unspecified Pad Functions"
                    );
                    self.begin_group(text)?;
                    let result = self.modify(|pkg| {
                        for fpt in pkg.footprints_mut().iter_mut() {
                            for pad in fpt.pads_mut().iter_mut() {
                                if pad.pad().function() == PadFunction::Unspecified {
                                    pad.pad_mut().set_function(function);
                                }
                            }
                        }
                    });
                    if let Err(e) = result {
                        self.abort_group()?;
                        return Err(e);
                    }
                    self.commit_group()?;
                    Ok(FixOutcome::Applied)
                } else {
                    self.fix_pad(p.footprint.uuid, p.pad, |pad| {
                        pad.pad_mut().set_function(function);
                    })
                }
            }
            _ => Ok(FixOutcome::NotFixable),
        }
    }
}

impl LibraryElementEditor<Component> {
    /// Whether a message has an automatic fix (upstream
    /// `ComponentTab::autoFixImpl(checkOnly = true)`).
    pub fn can_auto_fix(msg: &LibraryCheckMessage) -> bool {
        is_base_fixable(msg)
            || matches!(
                msg,
                LibraryCheckMessage::Component(
                    ComponentCheckMessage::MissingDefaultValue
                        | ComponentCheckMessage::MissingSymbolVariant
                        | ComponentCheckMessage::NonFunctionalSignalInversionSign { .. }
                )
            )
    }

    /// Fixes a message (upstream `ComponentTab::autoFix()`).
    pub fn auto_fix(
        &mut self,
        msg: &LibraryCheckMessage,
        params: &FixParams,
    ) -> Result<FixOutcome> {
        if let Some(outcome) = fix_base(self, msg, params)? {
            return Ok(outcome);
        }
        let LibraryCheckMessage::Component(msg) = msg else {
            return Ok(FixOutcome::NotFixable);
        };
        match msg {
            ComponentCheckMessage::MissingDefaultValue => match params.specific_component {
                Some(specific) => {
                    let value = if specific {
                        "{{MPN or DEVICE or COMPONENT}}"
                    } else {
                        "{{MPN or DEVICE}}"
                    };
                    self.execute(super::commands::EditComponent {
                        default_value: Some(value.to_owned()),
                        ..Default::default()
                    })?;
                    Ok(FixOutcome::Applied)
                }
                None => Ok(FixOutcome::Request(FixRequest::Parameter(
                    FixParam::SpecificComponent,
                ))),
            },
            ComponentCheckMessage::MissingSymbolVariant => {
                let variant = ComponentSymbolVariant::new(
                    Uuid::new_random(),
                    "",
                    ElementName::new("default")?,
                    "",
                );
                self.execute(InsertVariant(variant))?;
                Ok(FixOutcome::Applied)
            }
            ComponentCheckMessage::NonFunctionalSignalInversionSign { signal } => {
                let current = self
                    .element()
                    .signals()
                    .by_uuid(&signal.uuid())
                    .ok_or_else(|| Error::not_found("signal", signal.uuid()))?;
                let name = format!(
                    "!{}",
                    current.name().as_str().chars().skip(1).collect::<String>()
                );
                self.execute(super::commands::EditComponentSignal {
                    name: Some(CircuitIdentifier::new(name)?),
                    ..super::commands::EditComponentSignal::new(signal.uuid())
                })?;
                Ok(FixOutcome::Applied)
            }
            _ => Ok(FixOutcome::NotFixable),
        }
    }
}

/// Inserts a symbol variant (upstream `CmdComponentSymbolVariantInsert`).
struct InsertVariant(ComponentSymbolVariant);

impl super::ElementCommand<Component> for InsertVariant {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementInsert",
            "Add {0}",
            "variant"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        cmp.symbol_variants_mut().push(self.0);
        Ok(())
    }
}

impl LibraryElementEditor<Device> {
    /// Whether a message has an automatic fix (upstream
    /// `DeviceTab::autoFixImpl(checkOnly = true)`).
    pub fn can_auto_fix(msg: &LibraryCheckMessage) -> bool {
        is_base_fixable(msg)
    }

    /// Fixes a message (upstream `DeviceTab::autoFix()`).
    pub fn auto_fix(
        &mut self,
        msg: &LibraryCheckMessage,
        params: &FixParams,
    ) -> Result<FixOutcome> {
        Ok(fix_base(self, msg, params)?.unwrap_or(FixOutcome::NotFixable))
    }
}

/// Returns the approvals of `messages` which are approved in the element
/// (for the check message list: which messages are shown as approved).
pub fn approved_messages<E: EditableElement>(
    editor: &LibraryElementEditor<E>,
    messages: &[LibraryCheckMessage],
) -> BTreeSet<usize> {
    let approvals = editor.element().metadata().message_approvals();
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| approvals.contains(m.to_message().approval()))
        .map(|(i, _)| i)
        .collect()
}
