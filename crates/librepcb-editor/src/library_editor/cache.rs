//! Port of libs/librepcb/editor/library/libraryelementcache.{h,cpp}: loads
//! library elements by UUID from a [`LibraryElementSource`] (upstream: the
//! workspace library database) and caches them.
//!
//! Differences to upstream: categories and organizations are not cached
//! (not needed by the element editors yet); there is no automatic reset on
//! library rescans, the application calls [`LibraryElementCache::reset()`].

use std::collections::HashMap;
use std::sync::Arc;

use librepcb_core::fileio::{TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::project::LibraryElementKind;
use librepcb_core::types::Uuid;
use librepcb_i18n::tr;
use parking_lot::Mutex;

use crate::error::{Error, Result};
use crate::library_source::LibraryElementSource;

/// Cached elements of one kind.
type Cache<T> = Mutex<HashMap<Uuid, Arc<T>>>;

/// Loads and caches library elements (read-only).
pub struct LibraryElementCache {
    source: Arc<dyn LibraryElementSource>,
    symbols: Cache<Symbol>,
    packages: Cache<Package>,
    components: Cache<Component>,
    devices: Cache<Device>,
}

static_assertions::assert_impl_all!(LibraryElementCache: Send, Sync);

impl std::fmt::Debug for LibraryElementCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryElementCache")
            .finish_non_exhaustive()
    }
}

impl LibraryElementCache {
    /// Creates a cache over `source`.
    pub fn new(source: Arc<dyn LibraryElementSource>) -> Self {
        Self {
            source,
            symbols: Mutex::default(),
            packages: Mutex::default(),
            components: Mutex::default(),
            devices: Mutex::default(),
        }
    }

    /// Discards all cached elements (upstream: on library rescans).
    pub fn reset(&self) {
        let count = self.symbols.lock().drain().count()
            + self.packages.lock().drain().count()
            + self.components.lock().drain().count()
            + self.devices.lock().drain().count();
        log::debug!("Discarded {count} cached library elements.");
    }

    /// Returns the latest version of a symbol (upstream `getSymbol()`).
    pub fn symbol(&self, uuid: &Uuid) -> Result<Arc<Symbol>> {
        self.element(&self.symbols, LibraryElementKind::Symbol, uuid)
    }

    /// Returns the latest version of a package.
    pub fn package(&self, uuid: &Uuid) -> Result<Arc<Package>> {
        self.element(&self.packages, LibraryElementKind::Package, uuid)
    }

    /// Returns the latest version of a component.
    pub fn component(&self, uuid: &Uuid) -> Result<Arc<Component>> {
        self.element(&self.components, LibraryElementKind::Component, uuid)
    }

    /// Returns the latest version of a device.
    pub fn device(&self, uuid: &Uuid) -> Result<Arc<Device>> {
        self.element(&self.devices, LibraryElementKind::Device, uuid)
    }

    fn element<T: LibraryBaseElement>(
        &self,
        cache: &Cache<T>,
        kind: LibraryElementKind,
        uuid: &Uuid,
    ) -> Result<Arc<T>> {
        if let Some(element) = cache.lock().get(uuid) {
            return Ok(element.clone());
        }
        let Some(dir) = self.source.element_directory(kind, uuid) else {
            return Err(Error::InvalidArgument(format!(
                "{} {}",
                tr!(
                    "librepcb::editor::LibraryElementCache",
                    "Library element '{0}' with UUID '{1}' not found in workspace library.",
                    T::LONG_ELEMENT_NAME,
                    uuid
                ),
                tr!(
                    "librepcb::editor::LibraryElementCache",
                    "Please make sure that all dependent libraries are installed."
                )
            )));
        };
        let fs = TransactionalFileSystem::open_ro(&dir)?;
        let element = Arc::new(T::open(TransactionalDirectory::new(Arc::new(fs), ""))?);
        cache.lock().insert(*uuid, element.clone());
        Ok(element)
    }
}
