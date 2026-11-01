//! Port of libs/librepcb/core/project/bomgenerator.{h,cpp}.
//!
//! Project-specific assembly selection feeds the existing [`Bom`] grouping
//! and sorting implementation. Attribute substitution and token removal use
//! the existing attribute model and `regex` crate, respectively.

use super::board::Board;
use super::{AssemblyVariantId, Error, Project, ProjectAttributeLookup, Result};
use crate::export::Bom;
use crate::library::pkg::AssemblyType;
use regex::Regex;

struct PartItem {
    value: String,
    mpn: String,
    manufacturer: String,
    attributes: Vec<String>,
}

struct ComponentItem {
    designator: String,
    parts: Vec<PartItem>,
    package: String,
    attributes: Vec<String>,
    mount: bool,
}

/// Generates a generic or board-specific bill of materials for one assembly
/// variant. Custom keys ending in `[]` are repeated for every alternative
/// part; other custom keys become common columns.
#[derive(Debug, Clone)]
pub struct BomGenerator<'a> {
    project: &'a Project,
    additional_attributes: Vec<String>,
}

static_assertions::assert_impl_all!(BomGenerator<'_>: Send, Sync);

impl<'a> BomGenerator<'a> {
    /// Creates a generator without custom columns.
    pub fn new(project: &'a Project) -> Self {
        Self {
            project,
            additional_attributes: Vec::new(),
        }
    }

    /// Sets custom attribute keys, preserving their order within the common
    /// and part-specific column groups (like upstream).
    pub fn set_additional_attributes(&mut self, attributes: Vec<String>) {
        self.additional_attributes = attributes;
    }

    /// Generates a BOM. Missing selected parts fall back to all assembly
    /// variants for display, while the row remains marked "do not mount".
    pub fn generate(&self, board: Option<&Board>, variant: AssemblyVariantId) -> Result<Bom> {
        let mut common_keys = Vec::new();
        let mut part_keys = Vec::new();
        for key in &self.additional_attributes {
            if let Some(key) = key.strip_suffix("[]") {
                part_keys.push(key);
            } else {
                common_keys.push(key.as_str());
            }
        }
        let mut items = Vec::new();
        let mut max_parts = 1;
        for component in self.project.circuit().component_instances().values() {
            let lib_component = self
                .project
                .library()
                .component(&component.lib_component())
                .ok_or(Error::MissingLibraryComponent(component.lib_component()))?;
            let device = board.and_then(|board| board.device(component.id()));
            let lookup = match (board, device) {
                (Some(board), Some(device)) => {
                    ProjectAttributeLookup::device(self.project, board, device, None)
                }
                _ => ProjectAttributeLookup::component(self.project, component, None, None),
            };
            let mut assembly_expected = !lib_component.schematic_only();
            let (parts, mount) = if let Some(device) = device {
                let lib_device = self
                    .project
                    .library()
                    .device(&device.lib_device())
                    .ok_or(Error::MissingLibraryDevice(device.lib_device()))?;
                let package = self
                    .project
                    .library()
                    .package(&lib_device.package_uuid())
                    .ok_or(Error::MissingLibraryPackage(lib_device.package_uuid()))?;
                assembly_expected = package.assembly_type() != AssemblyType::None;
                let parts = device.parts(self.project.circuit(), Some(variant))?;
                if parts.is_empty() {
                    (device.parts(self.project.circuit(), None)?, false)
                } else {
                    (parts, true)
                }
            } else if board.is_some() {
                (component.parts(None), false)
            } else {
                let parts = component.parts(Some(variant));
                if parts.is_empty() {
                    (component.parts(None), false)
                } else {
                    (parts, true)
                }
            };
            if !mount && !assembly_expected {
                continue;
            }
            let package = if board.is_some() {
                lookup.value("PACKAGE")?.unwrap_or_default()
            } else {
                "N/A".to_owned()
            };
            let mut part_items = Vec::new();
            for part in &parts {
                let lookup = match (board, device) {
                    (Some(board), Some(device)) => {
                        ProjectAttributeLookup::device(self.project, board, device, Some(part))
                    }
                    _ => {
                        ProjectAttributeLookup::component(self.project, component, None, Some(part))
                    }
                };
                let mut value = expanded(&lookup, "VALUE")?;
                if !part.mpn().is_empty() {
                    for redundant in [
                        part.mpn().to_string(),
                        lookup.value("DEVICE")?.unwrap_or_default(),
                        lookup.value("COMPONENT")?.unwrap_or_default(),
                    ] {
                        value = remove_substring(&value, &redundant)?;
                    }
                }
                if !part.manufacturer().is_empty() {
                    value = remove_substring(&value, part.manufacturer().as_str())?;
                }
                part_items.push(PartItem {
                    value: value.split_whitespace().collect::<Vec<_>>().join(" "),
                    mpn: part.mpn().to_string(),
                    manufacturer: part.manufacturer().to_string(),
                    attributes: part_keys
                        .iter()
                        .map(|key| expanded(&lookup, key))
                        .collect::<Result<_>>()?,
                });
            }
            let attributes = common_keys
                .iter()
                .map(|key| expanded(&lookup, key))
                .collect::<Result<_>>()?;
            if mount {
                max_parts = max_parts.max(part_items.len());
            }
            items.push(ComponentItem {
                designator: component.name().to_string(),
                parts: part_items,
                package,
                attributes,
                mount,
            });
        }

        // Headers are deliberately not translated, like upstream.
        let mut columns = vec!["Package".to_owned()];
        columns.extend(common_keys.iter().map(|key| (*key).to_owned()));
        let mut mpn_manufacturer_columns = Vec::new();
        for i in 0..max_parts {
            let suffix = if i == 0 {
                String::new()
            } else {
                format!("[{}]", i + 1)
            };
            columns.extend([
                format!("Value{suffix}"),
                format!("MPN{suffix}"),
                format!("Manufacturer{suffix}"),
            ]);
            mpn_manufacturer_columns.push((columns.len() - 2, columns.len() - 1));
            columns.extend(part_keys.iter().map(|key| format!("{key}{suffix}")));
        }
        let mut bom = Bom::new(columns, mpn_manufacturer_columns);
        for item in items {
            let mut attributes = vec![item.package];
            attributes.extend(item.attributes);
            for i in 0..max_parts {
                if let Some(part) = item.parts.get(i) {
                    attributes.extend([
                        part.value.clone(),
                        part.mpn.clone(),
                        part.manufacturer.clone(),
                    ]);
                    attributes.extend(part.attributes.iter().cloned());
                } else {
                    attributes.extend(std::iter::repeat_n(String::new(), 3 + part_keys.len()));
                }
            }
            bom.add_item(&item.designator, attributes, item.mount);
        }
        Ok(bom)
    }
}

fn expanded(lookup: &ProjectAttributeLookup<'_>, key: &str) -> Result<String> {
    lookup.substitute(&lookup.value(key)?.unwrap_or_default())
}

fn remove_substring(value: &str, redundant: &str) -> Result<String> {
    // Escaping guarantees valid syntax, but user data can still exceed
    // the regex engine's compiled pattern size limit.
    let regex = Regex::new(&format!(r"(^|\s){}($|\s)", regex::escape(redundant)))?;
    Ok(regex.replace_all(value, " ").into_owned())
}
