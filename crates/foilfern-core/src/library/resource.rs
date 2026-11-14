//! Port of libs/librepcb/core/library/resource.{h,cpp}.
//!
//! Differences to upstream: the URL is stored verbatim as string instead of
//! a `QUrl` (see COMPAT.md).

use crate::geometry::{object_list, property};
use crate::serialization::{self, DeserializeObject, HasName, List, SExpression, SerializeObject};
use crate::types::ElementName;

/// A resource of a library element, e.g. a link to a datasheet.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Resource {
    name: ElementName,
    media_type: String,
    url: String,
}

impl Resource {
    /// Creates a resource.
    pub fn new(name: ElementName, media_type: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name,
            media_type: media_type.into(),
            url: url.into(),
        }
    }

    property!(
        /// Returns the name.
        ref name: ElementName, set_name
    );
    property!(
        /// Returns the media type (MIME type), e.g. `application/pdf`.
        ref media_type: String, set_media_type
    );
    property!(
        /// Returns the URL (may be empty).
        ref url: String, set_url
    );
}

impl HasName for Resource {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

object_list!(
    /// List of [`Resource`]s.
    ResourceList,
    ResourceListTag,
    Resource,
    "resource"
);

impl SerializeObject for Resource {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.name);
        root.append_child("mediatype", &self.media_type);
        root.ensure_line_break();
        root.append_child("url", &self.url);
        root.ensure_line_break();
    }
}

impl DeserializeObject for Resource {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            name: node.child_value("@0")?,
            media_type: node.child_value("mediatype/@0")?,
            url: node.child_value("url/@0")?,
        })
    }
}
