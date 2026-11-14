//! Port of libs/librepcb/core/export/gerberattributewriter.{h,cpp}.

use super::{GerberAttribute, GerberAttributeType};

/// Keeps track of the currently set aperture and object attributes of a
/// Gerber file and generates the commands to change them.
#[derive(Debug, Clone, Default)]
pub struct GerberAttributeWriter {
    /// All currently set attributes, except file attributes.
    dictionary: Vec<GerberAttribute>,
}

impl GerberAttributeWriter {
    /// Creates a writer with no attributes set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the given attributes (and deletes all others), returning the
    /// Gerber commands needed for it (0..n lines).
    ///
    /// Attributes which are already set with the same value are not written
    /// again. If all attributes are removed, a single delete-all command is
    /// written.
    pub fn set_attributes(&mut self, attributes: &[GerberAttribute]) -> String {
        // Check which attributes need to be deleted or set.
        let mut to_delete = Vec::new();
        let mut to_set = attributes.to_vec();
        for current in &self.dictionary {
            let mut is_still_set = false;
            for new in attributes.iter().filter(|a| a.key() == current.key()) {
                is_still_set = true;
                if new == current
                    && let Some(index) = to_set.iter().position(|a| a == new)
                {
                    to_set.remove(index);
                }
            }
            if !is_still_set {
                to_delete.push(GerberAttribute::unset(current.key()));
            }
        }

        // If all attributes were removed, use the delete-all command.
        if !to_delete.is_empty() && (to_delete.len() == self.dictionary.len()) {
            to_delete = vec![GerberAttribute::unset("")];
        }

        // Update dictionary, but don't add file-scoped attributes since they
        // are handled quite special.
        self.dictionary = attributes
            .iter()
            .filter(|a| {
                matches!(
                    a.attribute_type(),
                    GerberAttributeType::Aperture | GerberAttributeType::Object
                )
            })
            .cloned()
            .collect();

        to_delete
            .iter()
            .chain(&to_set)
            .map(GerberAttribute::to_gerber_string)
            .collect()
    }
}
