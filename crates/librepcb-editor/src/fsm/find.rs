//! The "find" feature of the schematic and board editors: port of
//! libs/librepcb/editor/utils/searchcontext.{h,cpp} and of the
//! `goToObjects()` zoom rectangle of `SchematicTab`/`Board2dTab`.
//!
//! A [`SearchContext`] holds the candidates (components and named nets of
//! the page or board), the search term and the position of "find next" /
//! "find previous". The editors' `find_*()` methods select the found
//! objects and return a [`FindResult`] with the rectangle to zoom to.
//!
//! Wildcard terms (containing `*`) are matched by a small hand-written
//! matcher (upstream `QRegularExpression::fromWildcard()`, anchored and
//! case-insensitive: `*` matches any sequence, `?` one character); other
//! terms match case-insensitive substrings. No crate reproduces exactly
//! these two rules together, and the matcher is a few lines.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use librepcb_core::project::{ComponentInstanceId, NetSignalId, Project};
use librepcb_core::types::{Length, Point};
use librepcb_core::utils::toolbox;

/// The kind of a search candidate (upstream `SearchContext::ObjectType`;
/// components sort before nets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FindKind {
    /// A component (by name, e.g. "R1").
    Component,
    /// A net (by name).
    Net,
}

/// A search candidate (upstream `SearchContext::Candidate`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FindCandidate {
    /// The kind.
    pub kind: FindKind,
    /// The name.
    pub name: String,
}

impl FindCandidate {
    /// A component candidate.
    pub fn component(name: impl Into<String>) -> Self {
        Self {
            kind: FindKind::Component,
            name: name.into(),
        }
    }

    /// A net candidate.
    pub fn net(name: impl Into<String>) -> Self {
        Self {
            kind: FindKind::Net,
            name: name.into(),
        }
    }
}

/// The result of a search step: what was found and selected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FindResult {
    /// The found candidates.
    pub objects: Vec<FindCandidate>,
    /// The found components (to cross-probe).
    pub components: Vec<ComponentInstanceId>,
    /// The found nets (to cross-probe).
    pub nets: Vec<NetSignalId>,
    /// The rectangle (two corners) to zoom to, with upstream's margin
    /// (1.5 times the larger dimension, at most 10 mm); `None` if nothing
    /// was selected.
    pub zoom_rect: Option<(Point, Point)>,
}

/// The state of the "find" tool bar field (upstream `SearchContext`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchContext {
    term: String,
    wildcard: bool,
    forward: bool,
    index: i64,
    candidates: Vec<FindCandidate>,
    filtered: Vec<FindCandidate>,
}

impl SearchContext {
    /// An empty context.
    pub fn new() -> Self {
        Self {
            forward: true,
            ..Self::default()
        }
    }

    /// The (trimmed) search term.
    pub fn term(&self) -> &str {
        &self.term
    }

    /// Sets the search term (upstream `setTerm()`); restarts at the first
    /// match.
    pub fn set_term(&mut self, term: &str) {
        let term = term.trim();
        if term != self.term {
            self.term = term.to_owned();
            self.wildcard = self.term.contains('*');
            self.index = 0;
            self.forward = true;
            self.apply_filter();
        }
    }

    /// Sets the candidates (upstream `setCandidates()`): sorted by kind,
    /// then naturally by name.
    pub fn set_candidates(&mut self, mut candidates: Vec<FindCandidate>) {
        candidates.sort_by(|a, b| match a.kind.cmp(&b.kind) {
            Ordering::Equal => toolbox::compare_numeric(&a.name, &b.name),
            other => other,
        });
        self.candidates = candidates;
        self.apply_filter();
    }

    /// The candidates matching the term, best matches first (the
    /// suggestions list).
    pub fn suggestions(&self) -> &[FindCandidate] {
        &self.filtered
    }

    /// The objects to go to for "find next" (upstream `findNext()`): all
    /// matches for wildcard terms (or if nothing matches), else the next
    /// match (cycling).
    pub fn find_next(&mut self) -> Vec<FindCandidate> {
        if !self.forward {
            self.forward = true;
            self.index += 2;
        }
        self.step(1)
    }

    /// The objects to go to for "find previous" (upstream
    /// `findPrevious()`).
    pub fn find_previous(&mut self) -> Vec<FindCandidate> {
        if self.forward {
            self.forward = false;
            self.index -= 2;
        }
        self.step(-1)
    }

    fn step(&mut self, delta: i64) -> Vec<FindCandidate> {
        if self.filtered.is_empty() || self.wildcard {
            self.filtered.clone()
        } else if !self.term.is_empty() {
            let count = self.filtered.len() as i64;
            // Upstream: `mIndex %= count` (C++ remainder, negative after
            // stepping back from the first match, which then finds
            // nothing); we wrap around instead.
            self.index = self.index.rem_euclid(count);
            let index = usize::try_from(self.index).unwrap_or(0);
            let result = self.filtered.get(index).cloned().into_iter().collect();
            self.index += delta;
            result
        } else {
            Vec::new()
        }
    }

    fn matches(&self, name: &str) -> bool {
        if self.wildcard {
            wildcard_match(&self.term.to_lowercase(), &name.to_lowercase())
        } else {
            name.to_lowercase().contains(&self.term.to_lowercase())
        }
    }

    /// Upstream `applyFilter()`.
    fn apply_filter(&mut self) {
        if self.term.is_empty() {
            self.filtered = self.candidates.clone();
            return;
        }
        let mut filtered: Vec<FindCandidate> = self
            .candidates
            .iter()
            .filter(|c| self.matches(&c.name))
            .cloned()
            .collect();
        // Move best matches to the top (exact, then prefix matches).
        let term = self.term.to_lowercase();
        filtered.sort_by_key(|c| {
            let name = c.name.to_lowercase();
            if name == term {
                0
            } else if name.starts_with(&term) {
                1
            } else {
                2
            }
        });
        self.filtered = filtered;
    }
}

/// Anchored wildcard match: `*` any sequence, `?` any character.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// The candidates of all nets which are not anonymous (see
/// [`named_nets()`]), sorted later by [`SearchContext::set_candidates()`].
pub(crate) fn net_candidates(p: &Project) -> Vec<FindCandidate> {
    named_nets(p)
        .into_iter()
        .filter_map(|id| p.circuit().net_signal(id))
        .map(|n| FindCandidate::net(n.name().to_string()))
        .collect()
}

/// The nets which are not anonymous (upstream `!NetSignal::isAnonymous()`:
/// the name is forced by a pin or the net has net labels).
pub(crate) fn named_nets(p: &Project) -> BTreeSet<NetSignalId> {
    let mut named = forced_nets(p);
    for s in p.schematics() {
        for seg in s.net_segments().values() {
            if !seg.labels().is_empty() {
                named.insert(seg.net());
            }
        }
    }
    named
}

/// The nets whose name is forced by a connected component signal (upstream
/// `NetSignal::isNameForced()`).
pub(crate) fn forced_nets(p: &Project) -> BTreeSet<NetSignalId> {
    let mut named: BTreeSet<NetSignalId> = BTreeSet::new();
    for c in p.circuit().component_instances().values() {
        let Some(lib) = p.library().component(&c.lib_component()) else {
            continue;
        };
        for (uuid, signal) in c.signals() {
            if let Some(net) = signal.net()
                && lib
                    .signals()
                    .by_uuid(uuid)
                    .is_some_and(|s| s.is_net_signal_name_forced())
            {
                named.insert(net);
            }
        }
    }
    named
}

/// Resolves the found candidates to components and nets.
pub(crate) fn resolve_candidates(
    p: &Project,
    objects: &[FindCandidate],
) -> (BTreeSet<ComponentInstanceId>, BTreeSet<NetSignalId>) {
    let mut components = BTreeSet::new();
    let mut nets = BTreeSet::new();
    for o in objects {
        match o.kind {
            FindKind::Component => {
                if let Some((id, _)) = p.circuit().component_instance_by_name(&o.name) {
                    components.insert(id);
                }
            }
            FindKind::Net => {
                if let Some((id, _)) = p.circuit().net_signal_by_name(&o.name) {
                    nets.insert(id);
                }
            }
        }
    }
    (components, nets)
}

/// The zoom rectangle around `points` with upstream's margin (see
/// [`FindResult::zoom_rect`]).
pub(crate) fn zoom_rect(points: impl IntoIterator<Item = Point>) -> Option<(Point, Point)> {
    let mut iter = points.into_iter();
    let first = iter.next()?;
    let (mut min, mut max) = (first, first);
    for p in iter {
        min = Point::new(min.x.min(p.x), min.y.min(p.y));
        max = Point::new(max.x.max(p.x), max.y.max(p.y));
    }
    let size = (max.x - min.x).max(max.y - min.y);
    let margin = Length::new((size.to_nm() as f64 * 1.5) as i64).min(Length::new(10_000_000));
    let m = Point::new(margin, margin);
    Some((min - m, max + m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("r*", "r12"));
        assert!(wildcard_match("*xd", "mcu_rxd"));
        assert!(wildcard_match("r?", "r1"));
        assert!(!wildcard_match("r?", "r12"));
        assert!(!wildcard_match("x*", "rx"));
        assert!(wildcard_match("*", ""));
    }

    #[test]
    fn filter_and_cycle() {
        let mut s = SearchContext::new();
        s.set_candidates(vec![
            FindCandidate::net("MCU_RXD"),
            FindCandidate::net("RXD"),
            FindCandidate::component("R10"),
            FindCandidate::component("R2"),
        ]);
        // Sorted by kind, then naturally.
        s.set_term("");
        assert_eq!(s.suggestions()[0], FindCandidate::component("R2"));
        s.set_term("rxd");
        // The exact match comes first.
        assert_eq!(
            s.suggestions(),
            &[FindCandidate::net("RXD"), FindCandidate::net("MCU_RXD")]
        );
        assert_eq!(s.find_next(), vec![FindCandidate::net("RXD")]);
        assert_eq!(s.find_next(), vec![FindCandidate::net("MCU_RXD")]);
        assert_eq!(s.find_next(), vec![FindCandidate::net("RXD")]);
        assert_eq!(s.find_previous(), vec![FindCandidate::net("MCU_RXD")]);
        // Wildcards go to all matches.
        s.set_term("R*");
        assert_eq!(s.find_next().len(), 3);
        s.set_term("nothing");
        assert!(s.find_next().is_empty());
    }
}
