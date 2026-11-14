//! Errors of the [`serialization`](super) module.
//!
//! Replaces the `FileParseError`, `RuntimeError` and `LogicError` exceptions
//! thrown by libs/librepcb/core/serialization/*. Messages are the upstream
//! strings; only those translated upstream go through [`tr!`].

use std::path::PathBuf;

use foilfern_i18n::tr;

/// Result type of the serialization module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Reason of a [`Error::FileParse`] error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// The input contains no node at all.
    #[error("No S-Expression node found.")]
    NoNode,
    /// The input contains more than one root node.
    #[error("File contains more than one root node.")]
    MultipleRootNodes,
    /// A list is not terminated by `)`.
    #[error("S-Expression node ended without closing ')'.")]
    UnclosedList,
    /// A token (or list name) is empty or starts with an invalid character.
    /// `'\0'` denotes the end of the input.
    #[error("Invalid token character detected: '{0}'")]
    InvalidTokenChar(char),
    /// A string is not terminated by `"`.
    #[error("String ended without quote.")]
    UnterminatedString,
    /// Unsupported escape sequence in a string.
    #[error("Illegal escape sequence: '\\{0}'")]
    IllegalEscape(char),
    /// A list was expected but the node is something else.
    #[error("Node is not a list.")]
    NotAList,
    /// A token or string was expected but the node is something else.
    #[error("Node is not a token or string.")]
    NotAValue,
    /// A child path could not be resolved.
    #[error("Child not found: {0}")]
    ChildNotFound(String),
}

/// Error returned by S-expression parsing, serialization and deserialization.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Malformed file content (upstream `FileParseError`).
    #[error(
        "File parse error: {kind}\nFile: {}\nInvalid Content: '{content}'",
        .file.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
    )]
    FileParse {
        /// Path of the parsed file, if known.
        file: Option<PathBuf>,
        /// The offending content (may be empty).
        content: String,
        /// What went wrong.
        kind: ParseError,
    },
    /// A list name is not a valid token for the requested output mode.
    #[error("Invalid S-Expression list name: {0}")]
    InvalidListName(String),
    /// A token is not valid for the requested output mode.
    #[error("Invalid S-Expression token: {0}")]
    InvalidToken(String),
    /// Invalid unsigned integer value.
    #[error("Invalid unsigned integer: '{0}'")]
    InvalidUnsignedInteger(String),
    /// Invalid (32 bit) integer value.
    #[error("Invalid integer: '{0}'")]
    InvalidInteger(String),
    /// Invalid (64 bit) integer value.
    #[error("Invalid longlong: '{0}'")]
    InvalidLongLong(String),
    /// Invalid single precision floating point value.
    #[error("Invalid float: '{0}'")]
    InvalidFloat(String),
    /// Invalid double precision floating point value.
    #[error("Invalid double: '{0}'")]
    InvalidDouble(String),
    /// Invalid boolean value.
    #[error("Invalid boolean: '{0}'")]
    InvalidBoolean(String),
    /// Invalid date/time value.
    #[error("Invalid datetime: '{0}'")]
    InvalidDateTime(String),
    /// No list element with the given UUID.
    #[error(
        "{}",
        tr!(
            "SerializableObjectList",
            "There is no element of type \"{0}\" with the UUID \"{1}\" in the list.",
            .tag,
            .uuid
        )
    )]
    UuidNotFound {
        /// Tag name of the list elements.
        tag: &'static str,
        /// The requested UUID.
        uuid: String,
    },
    /// No list element with the given name.
    #[error(
        "{}",
        tr!(
            "SerializableObjectList",
            "There is no element of type \"{0}\" with the name \"{1}\" in the list.",
            .tag,
            .name
        )
    )]
    NameNotFound {
        /// Tag name of the list elements.
        tag: &'static str,
        /// The requested name.
        name: String,
    },
    /// A key of a key-value map is defined multiple times.
    #[error("{}", tr!("SerializableKeyValueMap", "Key \"{0}\" defined multiple times.", .0))]
    DuplicateKey(String),
    /// A key-value map has no default (empty key) entry.
    #[error("{}", tr!("SerializableKeyValueMap", "No default {0} defined.", .0))]
    NoDefaultValue(&'static str),
    /// A value failed validation.
    #[error(transparent)]
    Value(#[from] crate::types::Error),
    /// Invalid attribute type, unit or value.
    #[error(transparent)]
    Attribute(#[from] crate::attribute::Error),
    /// A geometry object failed validation.
    #[error(transparent)]
    Geometry(#[from] crate::geometry::Error),
    /// A package attribute failed validation.
    #[error(transparent)]
    Package(#[from] crate::library::pkg::Error),
}

impl Error {
    /// Creates a [`Error::FileParse`] without file path.
    pub(crate) fn parse(kind: ParseError, content: impl Into<String>) -> Self {
        Self::FileParse {
            file: None,
            content: content.into(),
            kind,
        }
    }
}
