//! Port of parseagle/common/domelement.{h,cpp} (libs/parseagle).
//!
//! An owned XML element tree built with [`roxmltree`] (upstream uses
//! `QXmlStreamReader`), with the typed attribute accessors of upstream.
//!
//! Differences to upstream:
//! - Numbers are parsed with Rust's `str::parse` after trimming whitespace
//!   (upstream `QString::toInt()`/`toDouble()` also ignore surrounding
//!   whitespace), see COMPAT.md.

use std::collections::HashMap;

use super::super::error::{Error, Result};

/// An XML element with its attributes, direct text content and children.
#[derive(Debug, Clone, PartialEq)]
pub struct DomElement {
    name: String,
    attributes: HashMap<String, String>,
    text: String,
    children: Vec<DomElement>,
}

impl DomElement {
    /// Parses an XML document (or fragment with a single root element).
    pub fn parse(data: &str) -> Result<Self> {
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        };
        let doc = roxmltree::Document::parse_with_options(data, options)
            .map_err(|e| Error::Xml(e.to_string()))?;
        Ok(Self::from_node(doc.root_element()))
    }

    /// Parses the content of an EAGLE file, with a sanity check that it is
    /// not a binary file of EAGLE v5 or older (upstream `parseDocument()`).
    pub fn parse_document(data: &[u8]) -> Result<Self> {
        // Sanity check that no Eagle v5 (binary) project is imported. To
        // avoid false-positives, we test for several patterns.
        let contains = |pattern: &[u8]| data.windows(pattern.len()).any(|w| w == pattern);
        if !contains(b"<?xml") && !contains(b"<!DOCTYPE eagle") && !contains(b"<eagle version") {
            return Err(Error::NotXml);
        }
        // Workaround for garbage in some Eagle XML files, see
        // https://gitlab.com/kicad/code/kicad/-/work_items/11008
        let data: Vec<u8> = data
            .iter()
            .copied()
            .filter(|&b| (b != 0x0c) && (b != 0x06))
            .collect();
        Self::parse(&String::from_utf8_lossy(&data))
    }

    fn from_node(node: roxmltree::Node<'_, '_>) -> Self {
        let mut text = String::new();
        let mut children = Vec::new();
        for child in node.children() {
            if child.is_element() {
                children.push(Self::from_node(child));
            } else if child.is_text() {
                text.push_str(child.text().unwrap_or_default());
            }
        }
        Self {
            name: node.tag_name().name().to_owned(),
            attributes: node
                .attributes()
                .map(|a| (a.name().to_owned(), a.value().to_owned()))
                .collect(),
            text,
            children,
        }
    }

    /// Returns the tag name.
    pub fn tag_name(&self) -> &str {
        &self.name
    }

    /// Returns the text content (without the text of child elements).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns all child elements.
    pub fn children(&self) -> &[DomElement] {
        &self.children
    }

    /// Returns whether the attribute `name` exists.
    pub fn has_attribute(&self, name: &str) -> bool {
        self.attributes.contains_key(name)
    }

    /// Returns the attribute `name` as string.
    pub fn attr_str(&self, name: &str) -> Result<&str> {
        self.attributes
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| Error::MissingAttribute {
                attribute: name.to_owned(),
                element: self.name.clone(),
            })
    }

    /// Returns the attribute `name` as string, if it exists.
    pub fn opt_str(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    /// Returns the attribute `name` as bool (`yes`/`no`).
    pub fn attr_bool(&self, name: &str) -> Result<bool> {
        match self.attr_str(name)? {
            "yes" => Ok(true),
            "no" => Ok(false),
            _ => Err(Error::InvalidBool(name.to_owned())),
        }
    }

    /// Returns the attribute `name` as integer.
    pub fn attr_int(&self, name: &str) -> Result<i32> {
        self.attr_str(name)?
            .trim()
            .parse()
            .map_err(|_| Error::InvalidInt(name.to_owned()))
    }

    /// Returns the attribute `name` as floating point number.
    pub fn attr_double(&self, name: &str) -> Result<f64> {
        parse_double(self.attr_str(name)?).ok_or_else(|| Error::InvalidDouble(name.to_owned()))
    }

    /// Returns the optional attribute `name` as bool.
    pub fn opt_bool(&self, name: &str) -> Result<Option<bool>> {
        self.opt(name, Self::attr_bool)
    }

    /// Returns the optional attribute `name` as integer.
    pub fn opt_int(&self, name: &str) -> Result<Option<i32>> {
        self.opt(name, Self::attr_int)
    }

    /// Returns the optional attribute `name` as floating point number.
    pub fn opt_double(&self, name: &str) -> Result<Option<f64>> {
        self.opt(name, Self::attr_double)
    }

    fn opt<T>(&self, name: &str, f: fn(&Self, &str) -> Result<T>) -> Result<Option<T>> {
        if self.has_attribute(name) {
            f(self, name).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Returns whether a child element with the tag name `tag` exists.
    pub fn has_child(&self, tag: &str) -> bool {
        self.children.iter().any(|c| c.name == tag)
    }

    /// Returns the first child element with the tag name `tag`.
    pub fn first_child(&self, tag: &str) -> Result<&DomElement> {
        self.child(tag)
            .ok_or_else(|| Error::MissingChild(tag.to_owned()))
    }

    /// Returns the first child element with the tag name `tag`, if any.
    pub fn child(&self, tag: &str) -> Option<&DomElement> {
        self.children.iter().find(|c| c.name == tag)
    }

    /// Returns the children of the first child element `tag` (e.g. all
    /// `<symbol>` of `<symbols>`), or nothing if it doesn't exist.
    pub fn children_of(&self, tag: &str) -> &[DomElement] {
        self.child(tag).map(|c| c.children()).unwrap_or_default()
    }
}

/// Parses a floating point number like `QString::toDouble()` (surrounding
/// whitespace ignored).
pub(crate) fn parse_double(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        let e = DomElement::parse(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE eagle SYSTEM \"eagle.dtd\">\n\
             <a x=\" 1.5 \" b=\"yes\">t<c/>&gt;u</a>",
        )
        .unwrap();
        assert_eq!(e.tag_name(), "a");
        assert_eq!(e.text(), "t>u");
        assert_eq!(e.attr_double("x").unwrap(), 1.5);
        assert!(e.attr_bool("b").unwrap());
        assert!(e.attr_int("x").is_err());
        assert!(e.has_child("c"));
        assert!(e.first_child("d").is_err());
    }

    #[test]
    fn test_parse_document_rejects_binary() {
        assert!(matches!(
            DomElement::parse_document(b"\x10\x80binary"),
            Err(Error::NotXml)
        ));
    }
}
