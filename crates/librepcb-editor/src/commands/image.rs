//! Schematic images: port of
//! libs/librepcb/editor/project/cmd/cmdschematicimageadd.{h,cpp},
//! cmdschematicimageremove.{h,cpp} and of the file name helpers of
//! libs/librepcb/editor/utils/imagehelpers.{h,cpp}
//! (`findExistingFile()`, `getUnusedFileName()`).
//!
//! The image files live in the schematic directory; adding and removing
//! them is part of the undo group ([`Transaction::write_file()`]).
//!
//! Differences to upstream: the file name of a new image is derived from
//! the given base name without asking the user (upstream shows an input
//! dialog to edit it).

use librepcb_core::fileio::FileSystem;
use librepcb_core::geometry::Image;
use librepcb_core::project::{Mutation, Project, SchematicId, SchematicMutation};
use librepcb_core::types::{FileProofName, Uuid};
use librepcb_i18n::tr;

use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

/// Maximum length of a file-proof name (upstream
/// `FileProofNameConstraint::MAX_LEN`).
const FILE_PROOF_NAME_MAX_LEN: usize = 20;

/// The path of the schematic directory relative to the project directory.
pub(crate) fn schematic_dir(p: &Project, schematic: SchematicId) -> Result<String> {
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    Ok(format!("schematics/{}", s.directory_name()))
}

/// An image file of the schematic with the same content (upstream
/// `ImageHelpers::findExistingFile()`).
pub(crate) fn find_existing_image_file(
    p: &Project,
    schematic: SchematicId,
    data: &[u8],
) -> Result<Option<FileProofName>> {
    let dir = schematic_dir(p, schematic)?;
    let fs = p.directory();
    let mut files = fs.files(&dir);
    files.sort();
    for name in files {
        let ext = name.rsplit('.').next().unwrap_or_default();
        if !Image::SUPPORTED_EXTENSIONS.contains(&ext) {
            continue;
        }
        let Ok(proof) = FileProofName::new(&name) else {
            continue; // Invalid file name.
        };
        if fs.read(&format!("{dir}/{name}"))? == data {
            return Ok(Some(proof));
        }
    }
    Ok(None)
}

/// A file name which does not exist in the schematic directory yet,
/// derived from `basename` (upstream `ImageHelpers::getUnusedFileName()`):
/// `<basename>.<ext>`, `<basename>-2.<ext>`, ... (`image` if the cleaned
/// basename is empty).
pub(crate) fn unused_image_file_name(
    p: &Project,
    schematic: SchematicId,
    basename: &str,
    extension: &str,
) -> Result<FileProofName> {
    if !Image::SUPPORTED_EXTENSIONS.contains(&extension) {
        return Err(Error::InvalidArgument(format!(
            "Unsupported image file format '{extension}'."
        )));
    }
    let dir = schematic_dir(p, schematic)?;
    let mut name = FileProofName::clean(basename.trim());
    if name.is_empty() {
        name = "image".to_owned(); // Fallback / default for clipboard images.
    }
    let mut suffix = format!(".{extension}");
    let mut i = 2;
    loop {
        let max = FILE_PROOF_NAME_MAX_LEN.saturating_sub(suffix.chars().count());
        name = name.chars().take(max).collect();
        let file_name = format!("{name}{suffix}");
        if !p.directory().file_exists(&format!("{dir}/{file_name}")) {
            return FileProofName::new(&file_name)
                .map_err(|e| Error::InvalidArgument(e.to_string()));
        }
        suffix = format!("-{i}.{extension}");
        i += 1;
    }
}

/// Adds an image to a schematic page (upstream `CmdSchematicImageAdd`):
/// with `data`, its file is written to the schematic directory (it must
/// not exist yet), otherwise the file must exist already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddSchematicImage {
    /// The schematic.
    pub schematic: SchematicId,
    /// The image (its file name refers to the schematic directory).
    pub image: Image,
    /// The content of a new file.
    pub data: Option<Vec<u8>>,
}

impl Command for AddSchematicImage {
    /// UUID of the image.
    type Output = Uuid;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdSchematicImageAdd",
            "Add Image to Schematic"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Uuid> {
        let dir = schematic_dir(tx.project(), self.schematic)?;
        let path = format!("{dir}/{}", self.image.file_name());
        let exists = tx.project().directory().file_exists(&path);
        match (&self.data, exists) {
            (Some(_), true) => {
                return Err(Error::InvalidArgument(format!(
                    "File '{}' exists already.",
                    self.image.file_name()
                )));
            }
            (None, false) => {
                return Err(Error::InvalidArgument(format!(
                    "File '{}' does not exist yet.",
                    self.image.file_name()
                )));
            }
            _ => {}
        }
        if let Some(data) = self.data {
            tx.write_file(path, Some(data))?;
        }
        let uuid = self.image.uuid();
        tx.apply(Mutation::Schematic(SchematicMutation::AddImage {
            schematic: self.schematic,
            image: self.image,
        }))?;
        Ok(uuid)
    }
}

/// Removes an image from a schematic page with its file if no other image
/// refers to it (upstream `CmdSchematicImageRemove`).
pub(crate) fn remove_schematic_image(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    image: Uuid,
) -> Result<()> {
    let p = tx.project();
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    let img = s
        .images()
        .get(&image)
        .ok_or_else(|| Error::not_found("Image", image))?;
    let referenced_by_others = s
        .images()
        .values()
        .any(|i| i.uuid() != image && i.file_name() == img.file_name());
    let path = format!("{}/{}", schematic_dir(p, schematic)?, img.file_name());
    tx.apply(Mutation::Schematic(SchematicMutation::RemoveImage {
        schematic,
        image,
    }))?;
    if !referenced_by_others {
        tx.write_file(path, None)?;
    }
    Ok(())
}
