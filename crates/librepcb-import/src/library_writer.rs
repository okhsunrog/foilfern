//! Writing imported elements into the destination library (shared parts
//! of upstream `EagleLibraryImport::run()` and `KiCadLibraryImport::import()`).

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::Utc;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::library::cat::{CategoryKind, LibraryCategory};
use librepcb_core::library::{BaseMetadata, LibraryBaseElement};
use librepcb_core::types::{ElementName, Uuid};
use librepcb_core::utils::message_logger::MessageLogger;

/// Saves `element` into `<library>/<short element name>/<uuid>/`.
pub fn save_element<E: LibraryBaseElement>(
    library_dir: &FilePath,
    element: &mut E,
) -> Result<(), librepcb_core::library::Error> {
    let fp = library_dir
        .path_to(E::SHORT_ELEMENT_NAME)
        .path_to(&element.metadata().uuid().to_string());
    let fs = Arc::new(TransactionalFileSystem::open_rw(&fp)?);
    let mut dir = TransactionalDirectory::new(Arc::clone(&fs), "");
    element.save_to(&mut dir)?;
    fs.save()?;
    Ok(())
}

/// Creates the import category `uuid` in the library if it is contained in
/// `used_categories` and doesn't exist yet (upstream
/// `tryCreateCategoryIfRequired()`). Errors are logged.
pub fn try_create_category_if_required<K: CategoryKind>(
    library_dir: &FilePath,
    uuid: Uuid,
    used_categories: &BTreeSet<Uuid>,
    names: (&str, &str, &str),
    log: &MessageLogger<'_>,
) where
    LibraryCategory<K>: LibraryBaseElement,
{
    let (author, name, keywords) = names;
    if !used_categories.contains(&uuid) {
        return; // Category is not needed.
    }
    let fp = library_dir
        .path_to(K::SHORT_ELEMENT_NAME)
        .path_to(&uuid.to_string());
    if fp
        .path_to(&format!(".librepcb-{}", K::SHORT_ELEMENT_NAME))
        .is_existing_file()
    {
        return; // Category exists already.
    }
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut cat = LibraryCategory::<K>::new(BaseMetadata::new(
            uuid,
            "0.1".parse().expect("valid version"),
            author,
            Utc::now(),
            ElementName::new(name)?,
            "Imported library elements.",
            keywords,
        ))?;
        Ok(save_element(library_dir, &mut cat)?)
    })();
    if let Err(e) = result {
        log.critical(&format!("Failed to create category: {e}"));
    }
}
