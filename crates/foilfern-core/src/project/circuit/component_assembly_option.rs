//! Port of libs/librepcb/core/project/circuit/componentassemblyoption.{h,cpp}.
//!
//! Differences to upstream: the `onEdited` signal and the part list slot
//! are not ported.

use std::collections::BTreeSet;

use crate::attribute::AttributeList;
use crate::geometry::{object_list, property};
use crate::library::dev::PartList;
use crate::project::id::AssemblyVariantId;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::Uuid;

/// One option to assemble a component: a device with its parts (MPNs),
/// enabled in a set of assembly variants.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ComponentAssemblyOption {
    device: Uuid,
    attributes: AttributeList,
    assembly_variants: BTreeSet<AssemblyVariantId>,
    parts: PartList,
}

impl ComponentAssemblyOption {
    /// Creates an assembly option.
    pub fn new(
        device: Uuid,
        attributes: AttributeList,
        assembly_variants: BTreeSet<AssemblyVariantId>,
        parts: PartList,
    ) -> Self {
        Self {
            device,
            attributes,
            assembly_variants,
            parts,
        }
    }

    property!(
        /// Returns the UUID of the library device.
        copy device: Uuid, set_device
    );
    property!(
        /// Returns the attributes.
        ref attributes: AttributeList, set_attributes
    );
    property!(
        /// Returns the assembly variants in which the option is assembled.
        ref assembly_variants: BTreeSet<AssemblyVariantId>, set_assembly_variants
    );

    /// Returns the parts.
    pub fn parts(&self) -> &PartList {
        &self.parts
    }

    /// Returns the parts for modification.
    pub fn parts_mut(&mut self) -> &mut PartList {
        &mut self.parts
    }
}

impl SerializeObject for ComponentAssemblyOption {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.device);
        root.ensure_line_break();
        self.attributes.serialize(root);
        root.ensure_line_break();
        for variant in &self.assembly_variants {
            root.append_child("variant", variant);
            root.ensure_line_break();
        }
        self.parts.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ComponentAssemblyOption {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            device: node.child_value("@0")?,
            attributes: AttributeList::deserialize(node)?,
            assembly_variants: node
                .children_named("variant")
                .map(|n| n.child_value("@0"))
                .collect::<serialization::Result<_>>()?,
            parts: PartList::deserialize(node)?,
        })
    }
}

object_list!(
    /// The assembly options of a component instance, in user defined order.
    ComponentAssemblyOptionList,
    ComponentAssemblyOptionListTag,
    ComponentAssemblyOption,
    "device"
);
