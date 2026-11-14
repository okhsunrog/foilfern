//! Port of libs/librepcb/core/library/sym/symbolcheck.{h,cpp}.
//!
//! The image file validation is a port of `Image::tryLoad()`
//! (libs/librepcb/core/geometry/image.cpp) without keeping the decoded
//! image: PNG/JPEG files are decoded with the `image` crate, SVG files are
//! parsed with `usvg` (see COMPAT.md for the differences to Qt).

use std::collections::{BTreeMap, BTreeSet};

use super::symbol::Symbol;
use super::symbol_check_messages::{ImageFileError, SymbolCheckMessage as Msg};
use super::symbol_pin::SymbolPin;
use crate::fileio::{Error as FileIoError, FileSystem};
use crate::geometry::Image;
use crate::library::{LibraryBaseElement, LibraryCheckMessage, LibraryElement, run_element_checks};
use crate::types::{Layer, Length, Point, PositiveLength};

type MsgList = Vec<LibraryCheckMessage>;

/// Runs the symbol check, including the checks common to all elements
/// (upstream `SymbolCheck::runChecks()`).
pub fn run_symbol_checks(symbol: &Symbol, msgs: &mut MsgList) {
    run_element_checks(symbol.element_metadata(), msgs);
    check_invalid_image_files(symbol, msgs);
    check_duplicate_pin_names(symbol, msgs);
    check_pin_names_inversion_sign(symbol, msgs);
    check_off_the_grid_pins(symbol, msgs);
    check_overlapping_pins(symbol, msgs);
    check_missing_texts(symbol, msgs);
    check_wrong_text_layers(symbol, msgs);
    check_origin_in_center(symbol, msgs);
}

fn check_invalid_image_files(symbol: &Symbol, msgs: &mut MsgList) {
    // Emit the message only once per file name.
    let mut errors = BTreeMap::new();
    for image in symbol.images() {
        errors
            .entry(image.file_name().to_string())
            .or_insert_with(|| image_file_error(symbol, image));
    }
    for (file_name, error) in errors {
        if let Some((error, details)) = error {
            msgs.push(
                Msg::InvalidImageFile {
                    file_name,
                    error,
                    details,
                }
                .into(),
            );
        }
    }
}

fn image_file_error(symbol: &Symbol, image: &Image) -> Option<(ImageFileError, String)> {
    let dir = symbol.directory();
    if !dir.file_exists(image.file_name()) {
        return Some((ImageFileError::FileMissing, String::new()));
    }
    let content = match dir.read(image.file_name()) {
        // Upstream cannot read empty files (`QFile::readAll()` returns a null
        // byte array, which `TransactionalFileSystem::read()` treats as
        // non-existent file), so they are reported as read error.
        Ok(content) if content.is_empty() => {
            let error = dir
                .abs_path(image.file_name())
                .map(|path| FileIoError::TransactionalFileNotFound(path).to_string())
                .unwrap_or_default();
            return Some((ImageFileError::FileReadError, error));
        }
        Ok(content) => content,
        Err(e) => return Some((ImageFileError::FileReadError, e.to_string())),
    };
    let extension = image.file_extension();
    match try_load_image(&content, extension) {
        Ok(()) => None,
        Err(error) if !Image::SUPPORTED_EXTENSIONS.contains(&extension) => {
            Some((ImageFileError::UnsupportedFormat, error))
        }
        Err(error) => Some((ImageFileError::ImageLoadError, error)),
    }
}

/// Checks whether `data` is a valid image of the given format (file
/// extension), returning the error message if not (upstream
/// `Image::tryLoad()`).
fn try_load_image(data: &[u8], format: &str) -> Result<(), String> {
    Image::try_load(data, format).map(|_| ())
}

fn check_duplicate_pin_names(symbol: &Symbol, msgs: &mut MsgList) {
    let mut names = BTreeSet::new();
    for pin in symbol.pins() {
        if !names.insert(pin.name()) {
            msgs.push(
                Msg::DuplicatePinName {
                    name: pin.name().clone(),
                }
                .into(),
            );
        }
    }
}

/// Returns whether `name` starts with an inversion sign which has no
/// function in LibrePCB (`/` or `n` followed by an uppercase character).
pub(crate) fn has_non_functional_inversion_sign(name: &str) -> bool {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some('/'), _) => true,
        (Some('n'), Some(c)) => c.is_uppercase(),
        _ => false,
    }
}

fn check_pin_names_inversion_sign(symbol: &Symbol, msgs: &mut MsgList) {
    for pin in symbol.pins() {
        if has_non_functional_inversion_sign(pin.name()) {
            msgs.push(Msg::NonFunctionalPinInversionSign { pin: pin.clone() }.into());
        }
    }
}

fn check_off_the_grid_pins(symbol: &Symbol, msgs: &mut MsgList) {
    let grid = PositiveLength::new(Length::new(2_540_000)).expect("constant is positive");
    for pin in symbol.pins() {
        if pin.position() % *grid != Point::ORIGIN {
            msgs.push(
                Msg::PinNotOnGrid {
                    pin: pin.clone(),
                    grid_interval: grid,
                }
                .into(),
            );
        }
    }
}

fn check_overlapping_pins(symbol: &Symbol, msgs: &mut MsgList) {
    // Grouped by position, in order of first occurrence (upstream iterates a
    // QHash, i.e. in unspecified order).
    let mut groups: Vec<(Point, Vec<SymbolPin>)> = Vec::new();
    for pin in symbol.pins() {
        match groups.iter_mut().find(|(pos, _)| *pos == pin.position()) {
            Some((_, pins)) => pins.push(pin.clone()),
            None => groups.push((pin.position(), vec![pin.clone()])),
        }
    }
    for (_, pins) in groups {
        if pins.len() > 1 {
            msgs.push(Msg::OverlappingPins { pins }.into());
        }
    }
}

fn check_missing_texts(symbol: &Symbol, msgs: &mut MsgList) {
    let has_text = |text: &str| symbol.texts().iter().any(|t| t.text() == text);
    if !has_text("{{NAME}}") {
        msgs.push(Msg::MissingSymbolName.into());
    }
    if !has_text("{{VALUE}}") {
        msgs.push(Msg::MissingSymbolValue.into());
    }
}

fn check_wrong_text_layers(symbol: &Symbol, msgs: &mut MsgList) {
    for text in symbol.texts() {
        let expected_layer = match text.text().as_str() {
            "{{NAME}}" => Layer::SYMBOL_NAMES,
            "{{VALUE}}" => Layer::SYMBOL_VALUES,
            _ => continue,
        };
        if text.layer() != expected_layer {
            msgs.push(
                Msg::WrongTextLayer {
                    text: text.clone(),
                    expected_layer,
                }
                .into(),
            );
        }
    }
}

fn check_origin_in_center(symbol: &Symbol, msgs: &mut MsgList) {
    // Suppress this warning for symbols which have no pins and no grab areas.
    // This avoids false-positives on very special symbols like schematic
    // frames.
    let is_normal_symbol = !symbol.pins().is_empty()
        || symbol
            .circles()
            .iter()
            .any(|c| c.layer() == Layer::SYMBOL_OUTLINES && c.is_grab_area())
        || symbol
            .polygons()
            .iter()
            .any(|p| p.layer() == Layer::SYMBOL_OUTLINES && p.is_grab_area());
    if !is_normal_symbol {
        return;
    }

    // Determine bounding area of grab area polygons, as they are the best
    // indicator for the symbol body.
    let mut x = BTreeSet::new();
    let mut y = BTreeSet::new();
    for p in symbol.polygons() {
        if p.layer() == Layer::SYMBOL_OUTLINES
            && !p.is_filled()
            && p.is_grab_area()
            && p.path().is_closed()
            && !p.path().is_curved()
        {
            for v in p.path().vertices() {
                x.insert(v.pos.x);
                y.insert(v.pos.y);
            }
        }
    }

    // Only if we didn't find a symbol body, take more objects into account.
    if x.is_empty() || y.is_empty() {
        for pin in symbol.pins() {
            x.insert(pin.position().x);
            y.insert(pin.position().y);
        }
        for circle in symbol.circles() {
            if circle.layer() == Layer::SYMBOL_OUTLINES {
                let r = *circle.diameter() / 2;
                x.insert(circle.center().x - r);
                x.insert(circle.center().x + r);
                y.insert(circle.center().y - r);
                y.insert(circle.center().y + r);
            }
        }
        for p in symbol.polygons() {
            if p.layer() == Layer::SYMBOL_OUTLINES {
                for v in p.path().vertices() {
                    x.insert(v.pos.x);
                    y.insert(v.pos.y);
                }
            }
        }
    }

    // If there is no boundary, abort.
    let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)) =
        (x.first(), x.last(), y.first(), y.last())
    else {
        return;
    };

    // Calculate and check center.
    let center = Point::new((*min_x + *max_x) / 2, (*min_y + *max_y) / 2);
    let tolerance = Length::new(2_540_000 - 1); // Not ideal, but good enough?
    if center.x.abs().max(center.y.abs()) > tolerance {
        msgs.push(Msg::OriginNotInCenter { center }.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inversion_sign() {
        assert!(has_non_functional_inversion_sign("/RESET"));
        assert!(has_non_functional_inversion_sign("nRESET"));
        assert!(!has_non_functional_inversion_sign("n"));
        assert!(!has_non_functional_inversion_sign("nc"));
        assert!(!has_non_functional_inversion_sign("!RESET"));
        assert!(!has_non_functional_inversion_sign("RESET"));
    }

    #[test]
    fn image_validation() {
        assert!(try_load_image(b"foo", "bmp").is_err());
        assert!(try_load_image(b"", "png").is_err());
        assert!(try_load_image(b"foo", "png").is_err());
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20"/>"#;
        assert_eq!(try_load_image(svg, "svg"), Ok(()));
        assert!(try_load_image(b"<svg", "svg").is_err());
    }
}
