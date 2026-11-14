//! Port of libs/librepcb/core/serialization/fileformatmigrationunstable.{h,cpp}.
//!
//! Partially upgrades files of a previous unstable (development) release of
//! the current file format. Like upstream, it reuses the V1 migration with
//! the steps overridden which do not apply to such files; its content
//! changes whenever the unstable file format changes.

use super::v1::{self, ProjectContext, V1ProjectSteps};
use super::{FileFormatMigration, MigrationMessage, MigrationResult, V1Migration};
use crate::application;
use crate::fileio::TransactionalDirectory;
use crate::serialization::SExpression;
use crate::types::Version;

/// Migration of files of a previous unstable release of the current file
/// format (upstream `FileFormatMigrationUnstable`); both versions are the
/// current file format version.
#[derive(Debug, Clone)]
pub struct UnstableMigration {
    base: V1Migration,
    version: Version,
}

impl UnstableMigration {
    /// Creates the migration.
    pub fn new() -> Self {
        Self {
            base: V1Migration::new(),
            // Clearly distinguish from the base migration.
            version: application::file_format_version(),
        }
    }
}

impl Default for UnstableMigration {
    fn default() -> Self {
        Self::new()
    }
}

impl V1ProjectSteps for UnstableMigration {
    fn upgrade_output_jobs(
        &self,
        _root: &mut SExpression,
        _context: &mut ProjectContext,
    ) -> MigrationResult<()> {
        Ok(())
    }

    fn upgrade_circuit(
        &self,
        _root: &mut SExpression,
        _messages: &mut Vec<MigrationMessage>,
    ) -> MigrationResult<()> {
        Ok(())
    }

    fn upgrade_board(&self, _root: &mut SExpression) -> MigrationResult<()> {
        Ok(())
    }
}

impl FileFormatMigration for UnstableMigration {
    fn from_version(&self) -> &Version {
        &self.version
    }

    fn to_version(&self) -> &Version {
        &self.version
    }

    fn upgrade_component_category(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        Ok(())
    }

    fn upgrade_package_category(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        Ok(())
    }

    fn upgrade_symbol(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        Ok(())
    }

    fn upgrade_package(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        Ok(())
    }

    fn upgrade_component(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        Ok(())
    }

    fn upgrade_device(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        v1::upgrade_device_pads(dir)
    }

    fn upgrade_organization(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        self.base.upgrade_organization(dir)
    }

    fn upgrade_library(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        Ok(())
    }

    fn upgrade_project(
        &self,
        dir: &mut TransactionalDirectory,
        messages: &mut Vec<MigrationMessage>,
    ) -> MigrationResult<()> {
        v1::upgrade_project(self, dir, messages)
    }

    fn upgrade_workspace_data(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        Ok(())
    }
}
