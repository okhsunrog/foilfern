//! Attribute substitution of schematic and board texts with the core
//! [`ProjectAttributeLookup`], choosing the lookup objects like upstream
//! `SI_Text::updateText()` and `BI_StrokeText::updateText()` (no assembly
//! variant, first part of the device resp. component).

use librepcb_core::project::board::{Board, BoardDevice};
use librepcb_core::project::circuit::ComponentInstance;
use librepcb_core::project::schematic::{Schematic, SchematicSymbol};
use librepcb_core::project::{Project, ProjectAttributeLookup};

/// The device of a component on the first board which has one (upstream
/// `ComponentInstance::getPrimaryDevice()`).
pub(crate) fn primary_device<'a>(p: &'a Project, c: &ComponentInstance) -> Option<&'a BoardDevice> {
    p.boards().iter().find_map(|b| b.device(c.id()))
}

/// Substitutes the variables of a text of a schematic symbol.
pub(crate) fn substitute_symbol_text(
    p: &Project,
    schematic: &Schematic,
    symbol: &SchematicSymbol,
    text: &str,
) -> String {
    let component = p.circuit().component_instance(symbol.component());
    let device = component.and_then(|c| primary_device(p, c));
    let part = component.and_then(|c| match device {
        Some(device) => device.parts(c, None).into_iter().next(),
        None => c.parts(None).into_iter().next(),
    });
    ProjectAttributeLookup::for_symbol(p, schematic, symbol, device, part.as_ref(), None)
        .substitute(text)
}

/// Substitutes the variables of a text of a schematic page.
pub(crate) fn substitute_schematic_text(p: &Project, schematic: &Schematic, text: &str) -> String {
    ProjectAttributeLookup::for_schematic(p, schematic, None).substitute(text)
}

/// Substitutes the variables of a stroke text of a board.
pub(crate) fn substitute_board_text(p: &Project, board: &Board, text: &str) -> String {
    ProjectAttributeLookup::for_board(p, board, None).substitute(text)
}

/// Substitutes the variables of a stroke text of a device.
pub(crate) fn substitute_device_text(
    p: &Project,
    board: &Board,
    device: &BoardDevice,
    text: &str,
) -> String {
    let part = p
        .circuit()
        .component_instance(device.component())
        .and_then(|c| device.parts(c, None).into_iter().next());
    ProjectAttributeLookup::for_device(p, board, device, part.as_ref()).substitute(text)
}
