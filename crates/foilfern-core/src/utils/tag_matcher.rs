//! Port of libs/librepcb/core/utils/tagmatcher.{h,cpp}.

use std::collections::HashSet;

use crate::types::Tag;

/// Finds the best option (e.g. footprint) for a list of preferred tags.
#[derive(Debug, Clone, Default)]
pub struct TagMatcher {
    options: Vec<HashSet<Tag>>,
}

impl TagMatcher {
    /// Creates a matcher without options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an option with its tags and returns its index.
    pub fn add_option(&mut self, tags: impl IntoIterator<Item = Tag>) -> usize {
        self.options.push(tags.into_iter().collect());
        self.options.len() - 1
    }

    /// Returns the index of the best matching option, or `None` if there
    /// are no options.
    ///
    /// The preferred tags are applied in order, each one narrowing down the
    /// candidates to those having the tag (unless no candidate has it). As
    /// soon as one candidate is left, or all tags were applied, the first
    /// remaining candidate is returned.
    pub fn find_first_match(&self, preferred_tags: &[Tag]) -> Option<usize> {
        let mut candidates: Vec<usize> = (0..self.options.len()).collect();
        for tag in preferred_tags {
            if candidates.len() == 1 {
                break;
            }
            let remaining: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| self.options[i].contains(tag))
                .collect();
            if !remaining.is_empty() {
                candidates = remaining;
            }
        }
        candidates.first().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(tags: &[&str]) -> Vec<Tag> {
        tags.iter().map(|t| Tag::new(*t).unwrap()).collect()
    }

    #[test]
    fn empty() {
        assert_eq!(TagMatcher::new().find_first_match(&tags(&["a"])), None);
    }

    #[test]
    fn matching() {
        let mut matcher = TagMatcher::new();
        assert_eq!(matcher.add_option(tags(&["density-a", "reflow"])), 0);
        assert_eq!(matcher.add_option(tags(&["density-b", "reflow"])), 1);
        assert_eq!(matcher.add_option(tags(&["density-b", "wave"])), 2);
        assert_eq!(matcher.find_first_match(&[]), Some(0));
        assert_eq!(matcher.find_first_match(&tags(&["unknown"])), Some(0));
        assert_eq!(matcher.find_first_match(&tags(&["density-b"])), Some(1));
        assert_eq!(
            matcher.find_first_match(&tags(&["density-b", "wave"])),
            Some(2)
        );
        assert_eq!(
            matcher.find_first_match(&tags(&["wave", "reflow"])),
            Some(2)
        );
        assert_eq!(
            matcher.find_first_match(&tags(&["reflow", "x", "density-b"])),
            Some(1)
        );
    }
}
