//! Port of libs/librepcb/core/serialization/sexpression.{h,cpp}.
//!
//! [`SExpression`] is the in-memory tree of the LibrePCB file format. Parsing
//! and formatting (escaping, indentation, line breaks) are byte-compatible
//! with upstream.
//!
//! Differences to upstream:
//! - The node type is an enum; list-only operations live on [`List`], so
//!   "not a list" logic errors of upstream are unrepresentable.
//! - Nodes do not store a file path (upstream never sets it on parsed
//!   nodes anyway); the path passed to [`SExpression::parse()`] is only used
//!   for parse error messages.
//! - Removing a child by identity is replaced by index based access through
//!   [`List::children_mut()`].

use std::cmp::Ordering;
use std::path::Path;

use super::error::{Error, ParseError, Result};
use super::{FromSExpression, ToSExpression};

/// Parsing/formatting mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// LibrePCB syntax (very strict).
    #[default]
    LibrePcb,
    /// Compatibility with other tools (very permissive).
    Permissive,
}

/// A node of an S-expression tree.
///
/// Serde: externally tagged, e.g. `{"List": {"name": "approved",
/// "children": [{"Token": "foo"}]}}` (used for check message approvals in
/// the project model).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SExpression {
    /// A list with a name (tag) and an arbitrary number of children,
    /// e.g. `(position 1.0 2.0)`.
    List(List),
    /// A value without quotes, e.g. `-12.34`.
    Token(String),
    /// A value with double quotes, e.g. `"Foo!"`.
    String(String),
    /// A manual line break inside a list.
    LineBreak,
}

static_assertions::assert_impl_all!(SExpression: Send, Sync);

/// A list node: a name followed by child nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct List {
    name: String,
    children: Vec<SExpression>,
}

impl List {
    /// Creates an empty list.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            children: Vec::new(),
        }
    }

    /// Returns the name (tag) of the list.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sets the name (tag) of the list.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Returns all children, including line breaks.
    pub fn children(&self) -> &[SExpression] {
        &self.children
    }

    /// Returns all children for modification (e.g. by file format migrations).
    pub fn children_mut(&mut self) -> &mut Vec<SExpression> {
        &mut self.children
    }

    /// Appends a line break, unless the last child already is one.
    pub fn ensure_line_break(&mut self) {
        if !self.children.last().is_some_and(SExpression::is_line_break) {
            self.children.push(SExpression::LineBreak);
        }
    }

    /// Appends a child node and returns a reference to it.
    pub fn push(&mut self, child: SExpression) -> &mut SExpression {
        self.children.push(child);
        self.children.last_mut().expect("child was just pushed")
    }

    /// Appends a new empty list child and returns it.
    pub fn append_list(&mut self, name: impl Into<String>) -> &mut List {
        match self.push(SExpression::List(List::new(name))) {
            SExpression::List(list) => list,
            _ => unreachable!("a list was just pushed"),
        }
    }

    /// Appends the serialized `value` as child (upstream `appendChild(obj)`).
    pub fn append_value<T: ToSExpression + ?Sized>(&mut self, value: &T) -> &mut SExpression {
        self.push(value.to_sexpression())
    }

    /// Appends a child list `(name value)` and returns it (upstream
    /// `appendChild(name, obj)`).
    pub fn append_child<T: ToSExpression + ?Sized>(
        &mut self,
        name: impl Into<String>,
        value: &T,
    ) -> &mut List {
        let list = self.append_list(name);
        list.append_value(value);
        list
    }
}

impl SExpression {
    /// Creates an empty list node.
    pub fn list(name: impl Into<String>) -> Self {
        Self::List(List::new(name))
    }

    /// Creates a token node.
    pub fn token(token: impl Into<String>) -> Self {
        Self::Token(token.into())
    }

    /// Creates a string node.
    pub fn string(string: impl Into<String>) -> Self {
        Self::String(string.into())
    }

    /// Returns whether this is a list node.
    pub fn is_list(&self) -> bool {
        matches!(self, Self::List(_))
    }

    /// Returns whether this is a token node.
    pub fn is_token(&self) -> bool {
        matches!(self, Self::Token(_))
    }

    /// Returns whether this is a string node.
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }

    /// Returns whether this is a line break node.
    pub fn is_line_break(&self) -> bool {
        matches!(self, Self::LineBreak)
    }

    /// Returns the list, if this is a list node.
    pub fn as_list(&self) -> Option<&List> {
        match self {
            Self::List(list) => Some(list),
            _ => None,
        }
    }

    /// Returns the list mutably, if this is a list node.
    pub fn as_list_mut(&mut self) -> Option<&mut List> {
        match self {
            Self::List(list) => Some(list),
            _ => None,
        }
    }

    /// Returns the list name, or an error if this is not a list.
    pub fn name(&self) -> Result<&str> {
        match self {
            Self::List(list) => Ok(list.name()),
            _ => Err(Error::parse(ParseError::NotAList, "")),
        }
    }

    /// Returns the value of a token or string node, or an error otherwise.
    pub fn value(&self) -> Result<&str> {
        match self {
            Self::Token(value) | Self::String(value) => Ok(value),
            Self::List(list) => Err(Error::parse(ParseError::NotAValue, list.name())),
            Self::LineBreak => Err(Error::parse(ParseError::NotAValue, "")),
        }
    }

    /// Replaces the value of a token or string node, or returns an error for
    /// other node types.
    pub fn set_value(&mut self, value: impl Into<String>) -> Result<()> {
        match self {
            Self::Token(v) | Self::String(v) => {
                *v = value.into();
                Ok(())
            }
            other => other.value().map(|_| ()),
        }
    }

    /// Returns all children (empty for non-list nodes), including line
    /// breaks.
    pub fn children(&self) -> &[SExpression] {
        match self {
            Self::List(list) => list.children(),
            _ => &[],
        }
    }

    /// Returns the number of children, including line breaks.
    pub fn child_count(&self) -> usize {
        self.children().len()
    }

    /// Returns all children which are lists with the given name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a SExpression> {
        self.children()
            .iter()
            .filter(move |c| c.as_list().is_some_and(|l| l.name() == name))
    }

    /// Returns whether any direct child is equal to `child`.
    pub fn contains_child(&self, child: &SExpression) -> bool {
        self.children().contains(child)
    }

    /// Returns a (nested) child by path, or `None` if it does not exist
    /// (upstream `tryGetChild()`).
    ///
    /// The path consists of segments separated by `/`. A segment is either
    /// the name of a child list (the first match is taken) or `@` followed
    /// by an index. Indices skip line breaks, i.e. `@3` is the fourth child
    /// which is not a line break. For example, `via/position/@1` returns the
    /// Y coordinate of the first via.
    pub fn child(&self, path: &str) -> Option<&SExpression> {
        path.split('/').try_fold(self, |node, segment| {
            node.children().get(Self::child_index(node, segment)?)
        })
    }

    /// Same as [`child()`](Self::child), but returns an error if the child
    /// does not exist (upstream `getChild()`). Use this in deserialization,
    /// where a missing child means an invalid file.
    pub fn required_child(&self, path: &str) -> Result<&SExpression> {
        self.child(path)
            .ok_or_else(|| Error::parse(ParseError::ChildNotFound(path.to_owned()), ""))
    }

    /// Same as [`child()`](Self::child), but returns a mutable reference.
    pub fn child_mut(&mut self, path: &str) -> Option<&mut SExpression> {
        let mut node = self;
        for segment in path.split('/') {
            let index = Self::child_index(node, segment)?;
            node = node.as_list_mut()?.children.get_mut(index)?;
        }
        Some(node)
    }

    /// Resolves one path segment to the index of the child in `node`.
    fn child_index(node: &SExpression, segment: &str) -> Option<usize> {
        let children = node.children();
        if let Some(index) = segment.strip_prefix('@') {
            let index: usize = index.parse().ok()?;
            children
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.is_line_break())
                .nth(index)
                .map(|(i, _)| i)
        } else {
            children
                .iter()
                .position(|c| c.as_list().is_some_and(|l| l.name() == segment))
        }
    }

    /// Deserializes the child at `path` (see [`child()`](Self::child)); a
    /// missing child is an error.
    ///
    /// Shorthand for `T::from_sexpression(node.required_child(path)?)`. For
    /// optional children, use
    /// `node.child(path).map(T::from_sexpression).transpose()?`.
    pub fn child_value<T: FromSExpression>(&self, path: &str) -> Result<T> {
        T::from_sexpression(self.required_child(path)?)
    }

    /// Removes (recursively) all children which directly contain a child
    /// equal to `search`.
    pub fn remove_children_with_node_recursive(&mut self, search: &SExpression) {
        if let Self::List(list) = self {
            list.children.retain_mut(|child| {
                if child.contains_child(search) {
                    false
                } else {
                    child.remove_children_with_node_recursive(search);
                    true
                }
            });
        }
    }

    /// Replaces (recursively) all children equal to `search` by `replace`.
    pub fn replace_recursive(&mut self, search: &SExpression, replace: &SExpression) {
        if let Self::List(list) = self {
            for child in &mut list.children {
                if child == search {
                    *child = replace.clone();
                } else {
                    child.replace_recursive(search, replace);
                }
            }
        }
    }

    /// Serializes the tree to a string (with trailing newline).
    ///
    /// Fails if a list name or token is not valid in the given mode.
    pub fn to_string_with_mode(&self, mode: Mode) -> Result<String> {
        let mut out = String::new();
        self.write(&mut out, 0, mode)?;
        if !out.ends_with('\n') {
            out.push('\n'); // Newline at end of file.
        }
        Ok(out)
    }

    /// Serializes the tree to UTF-8 bytes (with trailing newline), i.e. the
    /// file content.
    pub fn to_byte_array(&self, mode: Mode) -> Result<Vec<u8>> {
        self.to_string_with_mode(mode).map(String::into_bytes)
    }

    fn write(&self, out: &mut String, indent: usize, mode: Mode) -> Result<()> {
        match self {
            Self::List(list) => {
                if !is_valid_token(&list.name, mode) {
                    return Err(Error::InvalidListName(list.name.clone()));
                }
                out.push('(');
                out.push_str(&list.name);
                let children = &list.children;
                let mut last_char_is_space = false;
                for (i, child) in children.iter().enumerate() {
                    let is_last = i + 1 == children.len();
                    if !last_char_is_space && !child.is_line_break() {
                        out.push(' ');
                    }
                    let next_child_is_line_break =
                        children.get(i + 1).is_some_and(SExpression::is_line_break);
                    let mut current_indent = if child.is_line_break() && next_child_is_line_break {
                        0
                    } else {
                        indent + 1
                    };
                    last_char_is_space = child.is_line_break() && (current_indent > 0);
                    if last_char_is_space && is_last {
                        current_indent -= 1;
                    }
                    child.write(out, current_indent, mode)?;
                }
                out.push(')');
            }
            Self::Token(token) => {
                if !is_valid_token(token, mode) {
                    return Err(Error::InvalidToken(token.clone()));
                }
                out.push_str(token);
            }
            Self::String(string) => {
                out.push('"');
                escape_string_into(string, out);
                out.push('"');
            }
            Self::LineBreak => {
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', indent));
            }
        }
        Ok(())
    }

    /// Parses S-expression file content.
    ///
    /// `file_path` is only used for error messages. Invalid UTF-8 sequences
    /// are replaced by U+FFFD (like `QString::fromUtf8()`).
    pub fn parse(content: &[u8], file_path: Option<&Path>, mode: Mode) -> Result<SExpression> {
        let content = String::from_utf8_lossy(content);
        let mut parser = Parser {
            content: &content,
            pos: 0,
            mode,
        };
        let result = parser.parse_root();
        result.map_err(|err| match err {
            Error::FileParse { content, kind, .. } => Error::FileParse {
                file: file_path.map(Path::to_path_buf),
                content,
                kind,
            },
            other => other,
        })
    }

    /// Type rank for ordering, identical to the upstream enum values.
    fn type_rank(&self) -> u32 {
        match self {
            Self::List(_) => 1 << 0,
            Self::Token(_) => 1 << 1,
            Self::String(_) => 1 << 2,
            Self::LineBreak => 1 << 3,
        }
    }

    fn raw_value(&self) -> &str {
        match self {
            Self::List(list) => &list.name,
            Self::Token(v) | Self::String(v) => v,
            Self::LineBreak => "",
        }
    }
}

impl Ord for SExpression {
    /// Orders by node type (list < token < string < line break), then by
    /// name/value (`str` ordering; upstream compares UTF-16 code units, see
    /// COMPAT.md), then by children.
    fn cmp(&self, other: &Self) -> Ordering {
        self.type_rank()
            .cmp(&other.type_rank())
            .then_with(|| self.raw_value().cmp(other.raw_value()))
            .then_with(|| self.children().cmp(other.children()))
    }
}

impl PartialOrd for SExpression {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<List> for SExpression {
    fn from(list: List) -> Self {
        Self::List(list)
    }
}

fn escape_string_into(string: &str, out: &mut String) {
    for c in string.chars() {
        match c {
            '"' => out.push_str("\\\""),    // Double quote *must* be escaped.
            '\\' => out.push_str("\\\\"),   // Backslash *must* be escaped.
            '\u{8}' => out.push_str("\\b"), // Escape backspace for readability.
            '\u{c}' => out.push_str("\\f"), // Escape form feed for readability.
            '\n' => out.push_str("\\n"),    // Escape line feed for readability.
            '\r' => out.push_str("\\r"),    // Escape carriage return for readability.
            '\t' => out.push_str("\\t"),    // Escape horizontal tab for readability.
            '\u{b}' => out.push_str("\\v"), // Escape vertical tab for readability.
            c => out.push(c),
        }
    }
}

fn is_valid_token(token: &str, mode: Mode) -> bool {
    !token.is_empty() && token.chars().all(|c| is_valid_token_char(c, mode))
}

fn is_valid_token_char(c: char, mode: Mode) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '\\' | '.' | ':' | '_' | '-')
        || ((mode == Mode::Permissive) && (c != '(') && (c != ')') && !c.is_whitespace())
}

/// Recursive descent parser over the decoded file content.
struct Parser<'a> {
    content: &'a str,
    /// Current byte position in `content`.
    pos: usize,
    mode: Mode,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.content[self.pos..].chars().next()
    }

    fn bump(&mut self, c: char) {
        self.pos += c.len_utf8();
    }

    fn parse_root(&mut self) -> Result<SExpression> {
        self.skip_whitespace_and_comments(true);
        if self.peek().is_none() {
            return Err(Error::parse(ParseError::NoNode, ""));
        }
        let root = self.parse_node()?;
        self.skip_whitespace_and_comments(true);
        if self.peek().is_some() {
            return Err(Error::parse(ParseError::MultipleRootNodes, ""));
        }
        Ok(root)
    }

    /// Parses the node at the current (non-end) position.
    fn parse_node(&mut self) -> Result<SExpression> {
        match self.peek() {
            Some('\n') => {
                self.bump('\n');
                self.skip_whitespace_and_comments(false);
                Ok(SExpression::LineBreak)
            }
            Some('(') => self.parse_list().map(SExpression::List),
            Some('"') => self.parse_string().map(SExpression::String),
            _ => self.parse_token().map(SExpression::Token),
        }
    }

    fn parse_list(&mut self) -> Result<List> {
        self.bump('(');
        let mut list = List::new(self.parse_token()?);
        loop {
            match self.peek() {
                None => return Err(Error::parse(ParseError::UnclosedList, "")),
                Some(')') => {
                    self.bump(')');
                    self.skip_whitespace_and_comments(false);
                    return Ok(list);
                }
                Some(_) => list.children.push(self.parse_node()?),
            }
        }
    }

    fn parse_token(&mut self) -> Result<String> {
        let start = self.pos;
        while let Some(c) = self.peek().filter(|&c| is_valid_token_char(c, self.mode)) {
            self.bump(c);
        }
        if self.pos == start {
            let c = self.peek().unwrap_or('\0');
            return Err(Error::parse(ParseError::InvalidTokenChar(c), ""));
        }
        let token = self.content[start..self.pos].to_owned();
        self.skip_whitespace_and_comments(false);
        Ok(token)
    }

    fn parse_string(&mut self) -> Result<String> {
        self.bump('"');
        let mut string = String::new();
        let mut escaped = false;
        loop {
            let Some(c) = self.peek() else {
                return Err(Error::parse(ParseError::UnterminatedString, ""));
            };
            if escaped {
                // Note: Until LibrePCB 0.1.5 the sexpresso library was used,
                // which escaped more characters than we do now. To still
                // support reading file format 0.1, all of them are accepted.
                let unescaped = match c {
                    '\'' => '\'',
                    '"' => '"',
                    '?' => '?',
                    '\\' => '\\',
                    'a' => '\u{7}',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'v' => '\u{b}',
                    c => return Err(Error::parse(ParseError::IllegalEscape(c), "")),
                };
                string.push(unescaped);
                self.bump(c);
                escaped = false;
            } else if c == '"' {
                self.bump(c);
                self.skip_whitespace_and_comments(false);
                return Ok(string);
            } else if c == '\\' {
                escaped = true;
                self.bump(c);
            } else {
                string.push(c);
                self.bump(c);
            }
        }
    }

    fn skip_whitespace_and_comments(&mut self, skip_newline: bool) {
        let mut is_comment = false;
        while let Some(c) = self.peek() {
            if c == ';' {
                is_comment = true; // Line comment of the Lisp language.
            } else if c == '\n' {
                is_comment = false;
            }
            let is_space = matches!(c, ' ' | '\u{c}' | '\r' | '\t' | '\u{b}');
            if is_comment || (skip_newline && c == '\n') || is_space {
                self.bump(c);
            } else {
                break;
            }
        }
    }
}
