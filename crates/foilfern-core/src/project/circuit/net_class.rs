//! Port of libs/librepcb/core/project/circuit/netclass.{h,cpp}.
//!
//! Differences to upstream: no circuit back-pointer, no `isAddedToCircuit`
//! state and no registration of net signals (see
//! [`RefIndex`](crate::project::ref_index::RefIndex)); the
//! `designRulesModified` signal is not ported.

use crate::geometry::{impl_uuid, property};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject,
};
use crate::types::{ElementName, PositiveLength, UnsignedLength, Uuid};

/// A net class: design rules shared by a group of nets.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetClass {
    uuid: Uuid,
    name: ElementName,
    /// `None` = inherit from the board design rules.
    default_trace_width: Option<PositiveLength>,
    /// `None` = inherit from the board design rules.
    default_via_drill: Option<PositiveLength>,
    min_copper_copper_clearance: UnsignedLength,
    min_copper_width: UnsignedLength,
    min_via_drill_diameter: UnsignedLength,
}

impl NetClass {
    /// Creates a net class inheriting all design rules from the board.
    pub fn new(uuid: Uuid, name: ElementName) -> Self {
        Self {
            uuid,
            name,
            default_trace_width: None,
            default_via_drill: None,
            min_copper_copper_clearance: UnsignedLength::ZERO,
            min_copper_width: UnsignedLength::ZERO,
            min_via_drill_diameter: UnsignedLength::ZERO,
        }
    }

    property!(
        /// Returns the name.
        ref name: ElementName, set_name
    );
    property!(
        /// Returns the default trace width (`None` = inherit).
        copy default_trace_width: Option<PositiveLength>, set_default_trace_width
    );
    property!(
        /// Returns the default via drill diameter (`None` = inherit).
        copy default_via_drill: Option<PositiveLength>, set_default_via_drill
    );
    property!(
        /// Returns the minimum copper to copper clearance.
        copy min_copper_copper_clearance: UnsignedLength, set_min_copper_copper_clearance
    );
    property!(
        /// Returns the minimum copper width.
        copy min_copper_width: UnsignedLength, set_min_copper_width
    );
    property!(
        /// Returns the minimum via drill diameter.
        copy min_via_drill_diameter: UnsignedLength, set_min_via_drill_diameter
    );
}

impl_uuid!(NetClass);

fn serialize_design_rule_value(value: Option<PositiveLength>) -> SExpression {
    match value {
        Some(v) => SExpression::token(v.to_string()),
        None => SExpression::token("inherit"),
    }
}

fn deserialize_design_rule_value(
    node: &SExpression,
) -> serialization::Result<Option<PositiveLength>> {
    if node.value()? == "inherit" {
        Ok(None)
    } else {
        Ok(Some(PositiveLength::from_sexpression(node)?))
    }
}

impl SerializeObject for NetClass {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("name", &self.name);
        root.ensure_line_break();
        root.append_child(
            "default_trace_width",
            &serialize_design_rule_value(self.default_trace_width),
        );
        root.ensure_line_break();
        root.append_child(
            "default_via_drill_diameter",
            &serialize_design_rule_value(self.default_via_drill),
        );
        root.ensure_line_break();
        root.append_child(
            "min_copper_copper_clearance",
            &self.min_copper_copper_clearance,
        );
        root.ensure_line_break();
        root.append_child("min_copper_width", &self.min_copper_width);
        root.ensure_line_break();
        root.append_child("min_via_drill_diameter", &self.min_via_drill_diameter);
        root.ensure_line_break();
    }
}

impl DeserializeObject for NetClass {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
            default_trace_width: deserialize_design_rule_value(
                node.required_child("default_trace_width/@0")?,
            )?,
            default_via_drill: deserialize_design_rule_value(
                node.required_child("default_via_drill_diameter/@0")?,
            )?,
            min_copper_copper_clearance: node.child_value("min_copper_copper_clearance/@0")?,
            min_copper_width: node.child_value("min_copper_width/@0")?,
            min_via_drill_diameter: node.child_value("min_via_drill_diameter/@0")?,
        })
    }
}
