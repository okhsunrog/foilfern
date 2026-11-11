//! Port of libs/librepcb/core/project/projectattributelookup.{h,cpp}.
//!
//! This is domain-specific precedence over the existing attribute model,
//! not a general expression parser. Borrowed read views replace Qt weak
//! pointers and parent pointers. Missing library references return the
//! project error instead of relying on an already-resolved object graph.

use super::board::{Board, BoardDevice};
use super::circuit::{AssemblyVariant, ComponentInstance};
use super::schematic::{Schematic, SchematicSymbol};
use super::{Error, Project, Result};
use crate::attribute::{AttributeList, substitute};
use crate::library::LibraryBaseElement;
use crate::library::dev::Part;
use chrono::Local;

#[derive(Debug, Clone, Copy)]
enum Scope<'a> {
    Project,
    Component(&'a ComponentInstance),
    Schematic(&'a Schematic),
    Board(&'a Board),
    Symbol(&'a Schematic, &'a SchematicSymbol),
    Device(&'a Board, &'a BoardDevice),
}

/// Resolves built-in and custom attributes in upstream's context-dependent
/// order. Values can contain placeholders; use [`Self::substitute`] to
/// expand them. A present empty value stops lookup in outer scopes.
#[derive(Debug, Clone, Copy)]
pub struct ProjectAttributeLookup<'a> {
    project: &'a Project,
    scope: Scope<'a>,
    variant: Option<&'a AssemblyVariant>,
    device: Option<&'a BoardDevice>,
    part: Option<&'a Part>,
}

static_assertions::assert_impl_all!(ProjectAttributeLookup<'_>: Send, Sync);

impl<'a> ProjectAttributeLookup<'a> {
    /// Queries assembly variant, then project attributes.
    pub fn for_project(project: &'a Project, variant: Option<&'a AssemblyVariant>) -> Self {
        Self {
            project,
            scope: Scope::Project,
            variant,
            device: None,
            part: None,
        }
    }

    /// Queries part, component, optional device, then project attributes.
    pub fn for_component(
        project: &'a Project,
        component: &'a ComponentInstance,
        device: Option<&'a BoardDevice>,
        part: Option<&'a Part>,
    ) -> Self {
        Self {
            scope: Scope::Component(component),
            device,
            part,
            ..Self::for_project(project, None)
        }
    }

    /// Queries schematic, assembly variant, then project attributes.
    pub fn for_schematic(
        project: &'a Project,
        schematic: &'a Schematic,
        variant: Option<&'a AssemblyVariant>,
    ) -> Self {
        Self {
            scope: Scope::Schematic(schematic),
            ..Self::for_project(project, variant)
        }
    }

    /// Queries board, assembly variant, then project attributes.
    pub fn for_board(
        project: &'a Project,
        board: &'a Board,
        variant: Option<&'a AssemblyVariant>,
    ) -> Self {
        Self {
            scope: Scope::Board(board),
            ..Self::for_project(project, variant)
        }
    }

    /// Queries part, symbol, component, optional device, schematic,
    /// assembly variant, then project attributes.
    pub fn for_symbol(
        project: &'a Project,
        schematic: &'a Schematic,
        symbol: &'a SchematicSymbol,
        device: Option<&'a BoardDevice>,
        part: Option<&'a Part>,
        variant: Option<&'a AssemblyVariant>,
    ) -> Self {
        Self {
            scope: Scope::Symbol(schematic, symbol),
            device,
            part,
            ..Self::for_project(project, variant)
        }
    }

    /// Queries part, device, component, board, then project attributes.
    pub fn for_device(
        project: &'a Project,
        board: &'a Board,
        device: &'a BoardDevice,
        part: Option<&'a Part>,
    ) -> Self {
        Self {
            scope: Scope::Device(board, device),
            part,
            ..Self::for_project(project, None)
        }
    }

    /// Returns the first matching value, including empty values. Keys are
    /// case sensitive, like upstream. Unknown keys return `None`.
    pub fn try_value(&self, key: &str) -> Result<Option<String>> {
        if let Some(part) = self.part {
            let value = custom(part.attributes(), key).or_else(|| match key {
                "MPN" => Some(part.mpn().to_string()),
                "MANUFACTURER" => Some(part.manufacturer().to_string()),
                _ => None,
            });
            if value.is_some() {
                return Ok(value);
            }
        }
        let value = match self.scope {
            Scope::Project => None,
            Scope::Component(component) => {
                if let Some(value) = self.component_value(component, key)? {
                    return Ok(Some(value));
                }
                self.optional_device_value(key)?
            }
            Scope::Schematic(schematic) => self.schematic_value(schematic, key),
            Scope::Board(board) => self.board_value(board, key),
            Scope::Symbol(schematic, symbol) => {
                if key == "NAME" {
                    return symbol.name(self.project.view()).map(Some);
                }
                let component = self.required_component(symbol.component())?;
                // Do not evaluate lower scopes once a value was found.
                if let Some(value) = self.component_value(component, key)? {
                    return Ok(Some(value));
                }
                if let Some(value) = self.optional_device_value(key)? {
                    return Ok(Some(value));
                }
                self.schematic_value(schematic, key)
            }
            Scope::Device(board, device) => {
                if let Some(value) = self.device_value(device, key)? {
                    return Ok(Some(value));
                }
                if let Some(value) =
                    self.component_value(self.required_component(device.component())?, key)?
                {
                    return Ok(Some(value));
                }
                self.board_value(board, key)
            }
        };
        if value.is_some() {
            return Ok(value);
        }
        if let Some(variant) = self.variant {
            match key {
                "VARIANT" => return Ok(Some(variant.name().to_string())),
                "VARIANT_INDEX" => {
                    return Ok(Some(
                        self.project
                            .circuit()
                            .assembly_variants()
                            .iter()
                            .position(|v| v.uuid() == variant.uuid())
                            .map_or(-1, |i| i as isize)
                            .to_string(),
                    ));
                }
                _ => {}
            }
        }
        Ok(self.project_value(key))
    }

    /// Expands placeholders with the shared attribute substitutor. Invalid
    /// model references are propagated instead of producing partial text.
    pub fn try_substitute(&self, text: &str) -> Result<String> {
        let mut error = None;
        let output = substitute(
            text,
            |key| match self.try_value(key) {
                Ok(value) => value,
                Err(err) => {
                    if error.is_none() {
                        error = Some(err);
                    }
                    None
                }
            },
            None,
        );
        match error {
            Some(error) => Err(error),
            None => Ok(output),
        }
    }

    /// Optional lookup for already validated models. Use try_value() to report invalid references.
    pub fn value(&self, key: &str) -> Option<String> {
        self.try_value(key).ok().flatten()
    }
    pub fn substitute(&self, text: &str) -> String {
        substitute(text, |key| self.value(key), None)
    }
    pub fn substitute_filtered(
        &self,
        text: &str,
        filter: &mut dyn FnMut(&str) -> String,
    ) -> String {
        substitute(text, |key| self.value(key), Some(filter))
    }

    fn required_component(&self, id: super::ComponentInstanceId) -> Result<&'a ComponentInstance> {
        self.project
            .circuit()
            .component_instance(id)
            .ok_or(Error::InexistentComponent(id.0))
    }

    fn component_value(&self, component: &ComponentInstance, key: &str) -> Result<Option<String>> {
        if let Some(value) = custom(component.attributes(), key) {
            return Ok(Some(value));
        }
        Ok(match key {
            "NAME" => Some(component.name().to_string()),
            "VALUE" => Some(component.value().clone()),
            "COMPONENT" => Some(
                self.project
                    .library()
                    .component(&component.lib_component())
                    .ok_or(Error::MissingLibraryComponent(component.lib_component()))?
                    .metadata()
                    .names()
                    .value(&self.project.settings().locale_order)
                    .to_string(),
            ),
            _ => None,
        })
    }

    fn optional_device_value(&self, key: &str) -> Result<Option<String>> {
        self.device
            .map(|device| self.device_value(device, key))
            .transpose()
            .map(Option::flatten)
    }

    fn device_value(&self, device: &BoardDevice, key: &str) -> Result<Option<String>> {
        if let Some(value) = custom(device.attributes(), key) {
            return Ok(Some(value));
        }
        if !matches!(key, "DEVICE" | "PACKAGE" | "FOOTPRINT") {
            return Ok(None);
        }
        let lib_device = self
            .project
            .library()
            .device(&device.lib_device())
            .ok_or(Error::MissingLibraryDevice(device.lib_device()))?;
        let locales = &self.project.settings().locale_order;
        if key == "DEVICE" {
            return Ok(Some(
                lib_device.metadata().names().value(locales).to_string(),
            ));
        }
        let package = self
            .project
            .library()
            .package(&lib_device.package_uuid())
            .ok_or(Error::MissingLibraryPackage(lib_device.package_uuid()))?;
        Ok(Some(if key == "PACKAGE" {
            package.metadata().names().value(locales).to_string()
        } else {
            package
                .footprints()
                .required_by_uuid(&device.lib_footprint())?
                .names()
                .value(locales)
                .to_string()
        }))
    }

    fn schematic_value(&self, schematic: &Schematic, key: &str) -> Option<String> {
        match key {
            "SHEET" => Some(schematic.name().to_string()),
            "PAGE" => Some(
                self.project
                    .schematic_index(schematic.id())
                    .map_or(0, |i| i + 1)
                    .to_string(),
            ),
            _ => None,
        }
    }

    fn board_value(&self, board: &Board, key: &str) -> Option<String> {
        match key {
            "BOARD" => Some(board.name().to_string()),
            "BOARD_DIRNAME" => Some(board.directory_name().to_owned()),
            "BOARD_INDEX" => Some(
                self.project
                    .board_index(board.id())
                    .map_or(-1, |i| i as isize)
                    .to_string(),
            ),
            _ => None,
        }
    }

    fn project_value(&self, key: &str) -> Option<String> {
        let metadata = self.project.metadata();
        if let Some(value) = custom(&metadata.attributes, key) {
            return Some(value);
        }
        Some(match key {
            "PROJECT" => metadata.name.to_string(),
            "PROJECT_DIRPATH" => self
                .project
                .attribute_directory_path()
                .map(|p| p.to_native())
                .unwrap_or_default(),
            "PROJECT_BASENAME" => self
                .project
                .attribute_file_path()
                .map(|p| p.basename().to_owned())
                .unwrap_or_default(),
            "PROJECT_FILENAME" => self.project.file_name().to_owned(),
            "PROJECT_FILEPATH" => self
                .project
                .attribute_file_path()
                .map(|p| p.to_native())
                .unwrap_or_default(),
            "CREATED_DATE" => metadata
                .created
                .with_timezone(&Local)
                .format("%Y-%m-%d")
                .to_string(),
            "CREATED_TIME" => metadata
                .created
                .with_timezone(&Local)
                .format("%H:%M:%S")
                .to_string(),
            "DATE" => self
                .project
                .date_time()
                .with_timezone(&Local)
                .format("%Y-%m-%d")
                .to_string(),
            "TIME" => self
                .project
                .date_time()
                .with_timezone(&Local)
                .format("%H:%M:%S")
                .to_string(),
            "AUTHOR" => metadata.author.clone(),
            "VERSION" => metadata.version.to_string(),
            "PAGES" => self.project.schematics().len().to_string(),
            "PAGE_X_OF_Y" => "Page {{PAGE}} of {{PAGES}}".to_owned(),
            _ => return None,
        })
    }
}

fn custom(attributes: &AttributeList, key: &str) -> Option<String> {
    attributes
        .by_name(key, true)
        .map(|attribute| attribute.value_tr(true))
}
