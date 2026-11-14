//! Port of libs/librepcb/core/font/strokefontpool.{h,cpp}.
//!
//! Upstream reads all `*.bene` files of a directory. To stay independent of
//! the file I/O layer, the pool is filled with file names and contents
//! ([`StrokeFontPool::from_files()`]); reading the directory is up to the
//! caller.

use std::collections::HashMap;

use super::{Error, StrokeFont};

/// A set of stroke fonts, identified by their file name (e.g.
/// `"newstroke.bene"`).
#[derive(Debug, Default)]
pub struct StrokeFontPool {
    fonts: HashMap<String, StrokeFont>,
}

// Shared by all threads rendering stroke texts (fonts are loaded lazily).
static_assertions::assert_impl_all!(StrokeFontPool: Send, Sync);

impl StrokeFontPool {
    /// Creates an empty pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a pool from `(file name, content)` pairs of a directory; files
    /// without `.bene` suffix are ignored.
    pub fn from_files<N, C>(files: impl IntoIterator<Item = (N, C)>) -> Self
    where
        N: Into<String>,
        C: Into<Vec<u8>>,
    {
        let mut pool = Self::new();
        for (name, content) in files {
            let name = name.into();
            if name
                .rsplit_once('.')
                .is_some_and(|(_, suffix)| suffix == "bene")
            {
                pool.insert(name, content);
            }
        }
        pool
    }

    /// Adds (or replaces) a font.
    pub fn insert(&mut self, filename: impl Into<String>, content: impl Into<Vec<u8>>) {
        self.fonts.insert(filename.into(), StrokeFont::new(content));
    }

    /// Returns whether a font with the given file name exists.
    pub fn exists(&self, filename: &str) -> bool {
        self.fonts.contains_key(filename)
    }

    /// Returns the font with the given file name.
    pub fn font(&self, filename: &str) -> Result<&StrokeFont, Error> {
        self.fonts
            .get(filename)
            .ok_or_else(|| Error::FontNotFound(filename.to_owned()))
    }
}
