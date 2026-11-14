//! Port of libs/librepcb/core/library/sym/symbol.{h,cpp}.

use std::collections::BTreeSet;

use super::symbol_check::run_symbol_checks;
use super::symbol_pin::SymbolPinList;
use crate::fileio::{FileSystem, TransactionalDirectory};
use crate::geometry::{CircleList, ImageList, PolygonList, TextList, property};
use crate::library::{
    BaseMetadata, ElementMetadata, LibraryBaseElement, LibraryCheckMessage, Result,
    element_accessors, impl_library_element,
};
use crate::serialization::{DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Length, PositiveLength, Uuid};

/// The part of a component which is added to schematics.
///
/// The UUID and the pins (their UUIDs) are the interface of a symbol and
/// must never be changed.
#[derive(Debug)]
pub struct Symbol {
    directory: TransactionalDirectory,
    metadata: ElementMetadata,
    grid_interval: PositiveLength,
    pins: SymbolPinList,
    polygons: PolygonList,
    circles: CircleList,
    texts: TextList,
    images: ImageList,
}

static_assertions::assert_impl_all!(Symbol: Send, Sync);

impl Symbol {
    /// Creates a new, empty symbol in a temporary directory.
    pub fn new(metadata: BaseMetadata) -> Result<Self> {
        Ok(Self {
            directory: TransactionalDirectory::new_temporary()?,
            metadata: ElementMetadata::new(metadata),
            grid_interval: PositiveLength::new(Length::new(2_540_000))
                .expect("constant is positive"),
            pins: SymbolPinList::new(),
            polygons: PolygonList::new(),
            circles: CircleList::new(),
            texts: TextList::new(),
            images: ImageList::new(),
        })
    }

    property!(
        /// Returns the grid interval used in the editor.
        copy grid_interval: PositiveLength, set_grid_interval
    );

    /// Returns whether the symbol contains no pins, polygons, circles,
    /// texts and images.
    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
            && self.polygons.is_empty()
            && self.circles.is_empty()
            && self.texts.is_empty()
            && self.images.is_empty()
    }

    /// Returns the pins.
    pub fn pins(&self) -> &SymbolPinList {
        &self.pins
    }

    /// Returns the pins for modification.
    pub fn pins_mut(&mut self) -> &mut SymbolPinList {
        // upstream: emits onEdited(PinsEdited) on modifications
        &mut self.pins
    }

    /// Returns the polygons.
    pub fn polygons(&self) -> &PolygonList {
        &self.polygons
    }

    /// Returns the polygons for modification.
    pub fn polygons_mut(&mut self) -> &mut PolygonList {
        // upstream: emits onEdited(PolygonsEdited) on modifications
        &mut self.polygons
    }

    /// Returns the circles.
    pub fn circles(&self) -> &CircleList {
        &self.circles
    }

    /// Returns the circles for modification.
    pub fn circles_mut(&mut self) -> &mut CircleList {
        // upstream: emits onEdited(CirclesEdited) on modifications
        &mut self.circles
    }

    /// Returns the texts.
    pub fn texts(&self) -> &TextList {
        &self.texts
    }

    /// Returns the texts for modification.
    pub fn texts_mut(&mut self) -> &mut TextList {
        // upstream: emits onEdited(TextsEdited) on modifications
        &mut self.texts
    }

    /// Returns the images.
    pub fn images(&self) -> &ImageList {
        &self.images
    }

    /// Returns the images for modification.
    pub fn images_mut(&mut self) -> &mut ImageList {
        // upstream: emits onEdited(ImagesEdited) on modifications
        &mut self.images
    }

    /// Makes this symbol a copy of `other` with new UUIDs of all contained
    /// objects (but keeps the symbol UUID), removing all files of this
    /// symbol and copying the image files (upstream `duplicateFrom()`).
    pub fn duplicate_from(&mut self, other: &Symbol) -> Result<()> {
        self.directory.remove_dir_recursively("")?;
        self.metadata.duplicate_from(&other.metadata);
        self.grid_interval = other.grid_interval;
        self.pins = other
            .pins
            .iter()
            .map(|o| o.with_uuid(Uuid::new_random()))
            .collect();
        self.polygons = other
            .polygons
            .iter()
            .map(|o| o.with_uuid(Uuid::new_random()))
            .collect();
        self.circles = other
            .circles
            .iter()
            .map(|o| o.with_uuid(Uuid::new_random()))
            .collect();
        self.texts = other
            .texts
            .iter()
            .map(|o| o.with_uuid(Uuid::new_random()))
            .collect();
        self.images = other
            .images
            .iter()
            .map(|o| o.with_uuid(Uuid::new_random()))
            .collect();
        // Copy all referenced files.
        let files: BTreeSet<&str> = other
            .images
            .iter()
            .map(|i| i.file_name().as_str())
            .collect();
        for file in files {
            if other.directory.file_exists(file) {
                self.directory.write(file, &other.directory.read(file)?)?;
            }
        }
        Ok(())
    }
}

impl LibraryBaseElement for Symbol {
    const SHORT_ELEMENT_NAME: &'static str = "sym";
    const LONG_ELEMENT_NAME: &'static str = "symbol";

    element_accessors!();

    fn load(directory: TransactionalDirectory, root: &SExpression) -> Result<Self> {
        Ok(Self {
            directory,
            metadata: ElementMetadata::deserialize(root)?,
            grid_interval: root.child_value("grid_interval/@0")?,
            pins: SymbolPinList::deserialize(root)?,
            polygons: PolygonList::deserialize(root)?,
            circles: CircleList::deserialize(root)?,
            texts: TextList::deserialize(root)?,
            images: ImageList::deserialize(root)?,
        })
    }

    fn run_checks(&self) -> Result<Vec<LibraryCheckMessage>> {
        let mut msgs = Vec::new();
        run_symbol_checks(self, &mut msgs);
        Ok(msgs)
    }
}

impl_library_element!(Symbol);

impl SerializeObject for Symbol {
    fn serialize(&self, root: &mut List) {
        self.metadata.serialize(root);
        root.ensure_line_break();
        root.append_child("grid_interval", &self.grid_interval);
        root.ensure_line_break();
        self.pins.serialize(root);
        root.ensure_line_break();
        self.polygons.serialize(root);
        root.ensure_line_break();
        self.circles.serialize(root);
        root.ensure_line_break();
        self.texts.serialize(root);
        root.ensure_line_break();
        self.images.serialize(root);
        root.ensure_line_break();
        self.metadata.base().serialize_message_approvals(root);
        root.ensure_line_break();
    }
}
