//! Port of libs/librepcb/core/project/bomgenerator.{h,cpp}.
//!
//! Collects the components of a project (or of one board) with their parts
//! and attributes into a [`Bom`], which
//! [`BomCsvWriter`](crate::export::BomCsvWriter) writes as CSV.
//!
//! Redundant information is removed from the value column with the same
//! regular expression as upstream (`(^|\s)<text>($|\s)`), using ASCII
//! whitespace like `QRegularExpression` without Unicode properties.

use regex::Regex;

use super::Project;
use super::attribute_lookup::ProjectAttributeLookup;
use super::board::{Board, BoardDevice};
use super::circuit::ComponentInstance;
use super::id::AssemblyVariantId;
use crate::export::Bom;
use crate::library::pkg::AssemblyType;

/// Generates the bill of materials of a project (upstream `BomGenerator`).
#[derive(Debug, Clone)]
pub struct BomGenerator<'a> {
    project: &'a Project,
    additional_attributes: Vec<String>,
}

/// A part of a component in the BOM.
#[derive(Debug, Clone, Default)]
struct PartItem {
    mpn: String,
    manufacturer: String,
    value: String,
    /// Values of the custom part attributes (`KEY[]`).
    attributes: Vec<String>,
}

/// A component in the BOM.
#[derive(Debug, Clone)]
struct ComponentItem {
    designator: String,
    parts: Vec<PartItem>,
    package_name: String,
    /// Values of the custom common attributes.
    attributes: Vec<String>,
    mount: bool,
}

impl<'a> BomGenerator<'a> {
    /// Creates a generator for `project` without additional attributes.
    pub fn new(project: &'a Project) -> Self {
        Self {
            project,
            additional_attributes: Vec::new(),
        }
    }

    /// Sets the attribute keys of additional columns. Keys ending with
    /// `[]` (e.g. `SUPPLIER[]`) are per part (one column per alternative
    /// part), the others per component.
    pub fn set_additional_attributes(&mut self, attributes: Vec<String>) {
        self.additional_attributes = attributes;
    }

    /// Generates the BOM of the whole project (`board` = `None`) or of the
    /// devices of one board, for an assembly variant.
    pub fn generate(&self, board: Option<&Board>, assembly_variant: AssemblyVariantId) -> Bom {
        // Parse custom attributes.
        let mut common_attributes: Vec<&str> = Vec::new();
        let mut part_attributes: Vec<&str> = Vec::new();
        for attribute in &self.additional_attributes {
            match attribute.strip_suffix("[]") {
                Some(key) => part_attributes.push(key),
                None => common_attributes.push(attribute),
            }
        }

        // Collect items.
        let mut items: Vec<ComponentItem> = Vec::new();
        let mut max_part_number = 1;
        for cmp in self.project.circuit.component_instances().values() {
            if let Some(item) = self.component_item(
                cmp,
                board,
                assembly_variant,
                &common_attributes,
                &part_attributes,
            ) {
                if item.mount {
                    max_part_number = max_part_number.max(item.parts.len());
                }
                items.push(item);
            }
        }

        // Build BOM header.
        let mut columns: Vec<String> = vec!["Package".to_owned()];
        let mut mpn_manufacturer_columns = Vec::new();
        columns.extend(common_attributes.iter().map(|a| (*a).to_owned()));
        for i in 0..max_part_number {
            let suffix = if i > 0 {
                format!("[{}]", i + 1)
            } else {
                String::new()
            };
            columns.push(format!("Value{suffix}"));
            columns.push(format!("MPN{suffix}"));
            columns.push(format!("Manufacturer{suffix}"));
            mpn_manufacturer_columns.push((columns.len() - 2, columns.len() - 1));
            columns.extend(part_attributes.iter().map(|a| format!("{a}{suffix}")));
        }

        // Generate BOM.
        let mut bom = Bom::new(columns, mpn_manufacturer_columns);
        for item in items {
            let mut attributes = vec![item.package_name];
            attributes.extend(item.attributes);
            for i in 0..max_part_number {
                let part = item.parts.get(i).cloned().unwrap_or_default();
                attributes.push(part.value);
                attributes.push(part.mpn);
                attributes.push(part.manufacturer);
                let count = part.attributes.len();
                attributes.extend(part.attributes);
                attributes.extend((count..part_attributes.len()).map(|_| String::new()));
            }
            bom.add_item(&item.designator, attributes, item.mount);
        }
        bom
    }

    /// Collects the BOM data of a component, `None` if the component does
    /// not appear in the BOM (not mounted and not expected to be assembled,
    /// like frames or supply symbols).
    fn component_item(
        &self,
        cmp: &ComponentInstance,
        board: Option<&Board>,
        assembly_variant: AssemblyVariantId,
        common_attributes: &[&str],
        part_attributes: &[&str],
    ) -> Option<ComponentItem> {
        let p = self.project;
        let mut mount = true;
        let mut device: Option<&BoardDevice> = None;
        let mut assembly_expected = !p
            .library
            .component(&cmp.lib_component())
            .is_some_and(|c| c.schematic_only());
        let parts = match board {
            Some(board) => match board.device(cmp.id()) {
                Some(dev) => {
                    device = Some(dev);
                    let mut parts = dev.parts(cmp, Some(assembly_variant));
                    if parts.is_empty() {
                        mount = false;
                        parts = dev.parts(cmp, None); // Fallback for convenience.
                    }
                    assembly_expected = p
                        .library
                        .device(&dev.lib_device())
                        .and_then(|d| p.library.package(&d.package_uuid()))
                        .is_some_and(|pkg| pkg.assembly_type() != AssemblyType::None);
                    parts
                }
                None => {
                    mount = false;
                    cmp.parts(None) // For convenience.
                }
            },
            None => {
                let mut parts = cmp.parts(Some(assembly_variant));
                if parts.is_empty() {
                    mount = false;
                    parts = cmp.parts(None); // Fallback for convenience.
                }
                parts
            }
        };

        if !mount && !assembly_expected {
            return None; // Skip components like frame sheets or supply symbols.
        }

        let substituted = |lookup: &ProjectAttributeLookup<'_>, key: &str| -> String {
            lookup.substitute(&lookup.value(key).unwrap_or_default())
        };

        let common_lookup = match (board, device) {
            (Some(board), Some(dev)) => ProjectAttributeLookup::for_device(p, board, dev, None),
            _ => ProjectAttributeLookup::for_component(p, cmp, None, None),
        };
        let package_name = if board.is_some() {
            common_lookup.value("PACKAGE").unwrap_or_default()
        } else {
            "N/A".to_owned()
        };
        let mut part_items = Vec::new();
        for part in &parts {
            let lookup = match (board, device) {
                (Some(board), Some(dev)) => {
                    ProjectAttributeLookup::for_device(p, board, dev, Some(part))
                }
                _ => ProjectAttributeLookup::for_component(p, cmp, None, Some(part)),
            };
            let mpn = part.mpn().to_string();
            let manufacturer = part.manufacturer().to_string();
            let mut value = substituted(&lookup, "VALUE");
            // Remove redundant information from the value since it could
            // lead to confusion.
            if !mpn.is_empty() {
                remove_sub_string(&mut value, &mpn);
                remove_sub_string(&mut value, &lookup.value("DEVICE").unwrap_or_default());
                remove_sub_string(&mut value, &lookup.value("COMPONENT").unwrap_or_default());
            }
            if !manufacturer.is_empty() {
                remove_sub_string(&mut value, &manufacturer);
            }
            // Simplify *after* the replacements!
            let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
            let attributes = part_attributes
                .iter()
                .map(|key| substituted(&lookup, key))
                .collect();
            part_items.push(PartItem {
                mpn,
                manufacturer,
                value,
                attributes,
            });
        }
        let attributes = common_attributes
            .iter()
            .map(|key| substituted(&common_lookup, key))
            .collect();
        Some(ComponentItem {
            designator: cmp.name().to_string(),
            parts: part_items,
            package_name,
            attributes,
            mount,
        })
    }
}

/// Replaces `substr` surrounded by whitespace or string boundaries by a
/// space (upstream `removeSubString()`).
fn remove_sub_string(s: &mut String, substr: &str) {
    let pattern = format!(r"(?-u:(^|\s)){}(?-u:($|\s))", regex::escape(substr));
    // The pattern is built from an escaped literal, so it is valid (unless
    // it exceeds the size limit, then nothing is removed).
    if let Ok(re) = Regex::new(&pattern) {
        *s = re.replace_all(s, " ").into_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_sub_string_like_upstream() {
        let mut s = "100nF MPN-1 X7R".to_owned();
        remove_sub_string(&mut s, "MPN-1");
        assert_eq!(s, "100nF X7R");
        let mut s = "MPN-1".to_owned();
        remove_sub_string(&mut s, "MPN-1");
        assert_eq!(s, " ");
        let mut s = "XMPN-1 MPN-1X".to_owned();
        remove_sub_string(&mut s, "MPN-1");
        assert_eq!(s, "XMPN-1 MPN-1X");
        let mut s = "a+b".to_owned();
        remove_sub_string(&mut s, "a+b");
        assert_eq!(s, " ");
    }
}
