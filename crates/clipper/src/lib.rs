//! Pure Rust port of Clipper 1, i.e. polyclipping 6.4.2 by Angus Johnson
//! (libs/polyclipping/clipper.{hpp,cpp} in the LibrePCB repository, Boost
//! Software License 1.0, see `License.txt`).
//!
//! Clipper performs boolean operations (intersection, union, difference,
//! xor) on polygons with integer coordinates, and offsets polygons and
//! polylines.
//!
//! # Why a port
//!
//! LibrePCB computes board planes, copper/stop mask/solder paste areas and
//! the geometry of exported production data (Gerber, ...) with Clipper 1.
//! These outputs must be identical to upstream LibrePCB, i.e. the same
//! vertices in the same order, the same polygon order and the same polygon
//! tree. No existing crate reproduces Clipper 1 (there is no pure Rust
//! Clipper 1 port on crates.io). Pure Rust alternatives were evaluated
//! against the C++ library on 1012 cases built from the geometry in
//! LibrePCB's test data (union/intersection/difference with Paths and
//! PolyTree output, round offsets with arc tolerance 5000 nm):
//!
//! | crate                        | exact | same rings¹ | same area | area ±0.1% |
//! |------------------------------|------:|------------:|----------:|-----------:|
//! | `clipper2-rust` 1.2.0        | 44.8% |       52.6% |     62.3% |      99.8% |
//! | `i_overlay` 9.0.0 (i64)      |  5.2% |       51.9% |     59.1% |      85.8% |
//! | `geo` 0.33.1 (via i_overlay) |  5.1% |       47.9% |     49.0% |      73.8% |
//! | `knipsa` 0.3.0               |  4.6% |       44.2% |     49.0% |      80.5% |
//!
//! ¹ Ignoring ring order, start vertex and orientation.
//!
//! Typical divergences: extra collinear vertices kept (Clipper2, knipsa),
//! polygons touching in a vertex merged instead of split (or vice versa),
//! different start vertex/ring order/tree nesting (i_overlay, geo),
//! different number of arc vertices in round offsets (all; no candidate
//! matched a single negative offset), and overlapping inputs of negative
//! offsets merged differently. Hence this crate is a faithful, line-by-line
//! port:
//!
//! - Same algorithm, same floating point expressions (evaluated in the same
//!   order), same rounding (including the behavior of the C++ double to
//!   integer conversion on x86-64 for out-of-range values), same 128 bit
//!   integer math and coordinate range checks. Transcendental functions
//!   (`sin`, `cos`, `tan`, `acos`, `atan2`) come from the [`libm`] crate,
//!   so results do not depend on the platform's C library; they differ from
//!   glibc in the last bit for some inputs, which did not change any golden
//!   vector (captured with glibc) after rounding to integer coordinates.
//! - The linked data structures (edges, output points, output records, joins,
//!   poly nodes) live in arenas and reference each other by index instead of
//!   raw pointers. No `unsafe` code.
//! - `std::sort()` is replaced by an exact replica of the libstdc++
//!   implementation (see [`std_sort`]), because the (unstable) order of equal
//!   elements affects the output.
//! - Exceptions are replaced by [`Error`]; `Execute()` returning `false` is
//!   mapped to [`Error::ExecutionFailed`] (except for the "nothing to do"
//!   case, which returns an empty solution).
//!
//! The port was verified with a differential test against the original C++
//! library (random and real-world inputs, all operations and fill types);
//! `tests/clipper/golden.rs` replays a subset of these cases.
//!
//! Configuration of the original: 64 bit coordinates (no `use_int32`), no Z
//! coordinate (no `use_xyz`), line clipping enabled (`use_lines`).

mod engine;
mod misc;
mod offset;
mod poly_tree;
pub mod std_sort;

pub use engine::Clipper;
pub use misc::{
    area, clean_polygon, clean_polygons, closed_paths_from_poly_tree, minkowski_diff,
    minkowski_sum, minkowski_sum_paths, open_paths_from_poly_tree, orientation, point_in_polygon,
    poly_tree_to_paths, reverse_path, reverse_paths, simplify_polygon, simplify_polygons,
};
pub use offset::ClipperOffset;
pub use poly_tree::{PolyNode, PolyTree};

/// Integer coordinate type (upstream `cInt`).
pub type CInt = i64;

/// A point with integer coordinates (upstream `IntPoint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct IntPoint {
    /// X coordinate.
    pub x: CInt,
    /// Y coordinate.
    pub y: CInt,
}

impl IntPoint {
    /// Creates a point.
    pub const fn new(x: CInt, y: CInt) -> Self {
        Self { x, y }
    }
}

/// A polygon or polyline (upstream `Path`).
pub type Path = Vec<IntPoint>;

/// A list of polygons or polylines (upstream `Paths`).
pub type Paths = Vec<Path>;

/// An axis aligned rectangle (upstream `IntRect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct IntRect {
    /// Minimum X.
    pub left: CInt,
    /// Minimum Y.
    pub top: CInt,
    /// Maximum X.
    pub right: CInt,
    /// Maximum Y.
    pub bottom: CInt,
}

/// Boolean operation (upstream `ClipType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipType {
    /// Subject AND clip.
    Intersection,
    /// Subject OR clip.
    Union,
    /// Subject AND NOT clip.
    Difference,
    /// Subject XOR clip.
    Xor,
}

/// Role of a path in a boolean operation (upstream `PolyType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolyType {
    /// Subject path (may be open).
    Subject,
    /// Clip path (must be closed).
    Clip,
}

/// Filling rule (upstream `PolyFillType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolyFillType {
    /// Odd winding numbers are filled.
    EvenOdd,
    /// Non-zero winding numbers are filled.
    NonZero,
    /// Positive winding numbers are filled.
    Positive,
    /// Negative winding numbers are filled.
    Negative,
}

/// Join type of offset corners (upstream `JoinType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JoinType {
    /// Squared corners.
    Square,
    /// Rounded corners (approximated by the arc tolerance).
    Round,
    /// Mitered corners (limited by the miter limit).
    Miter,
}

/// End type of offset paths (upstream `EndType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndType {
    /// Closed polygon (offset only on one side, depending on orientation).
    ClosedPolygon,
    /// Closed polyline (offset on both sides).
    ClosedLine,
    /// Open path with butt ends.
    OpenButt,
    /// Open path with square ends.
    OpenSquare,
    /// Open path with round ends.
    OpenRound,
}

/// Errors (upstream `clipperException`, plus failed executions).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A coordinate exceeds the supported range of ±0x3FFFFFFFFFFFFFFF.
    #[error("Coordinate outside allowed range")]
    CoordinateOutOfRange,
    /// An open path was added as clip path.
    #[error("AddPath: Open paths must be subject.")]
    OpenPathMustBeSubject,
    /// Open paths were added, but the result was requested as [`Paths`].
    #[error("Error: PolyTree struct is needed for open path clipping.")]
    PolyTreeNeededForOpenPaths,
    /// The clipping algorithm failed (upstream `Execute()` returns `false`,
    /// which happens only for pathological input).
    #[error("Clipper execution failed")]
    ExecutionFailed,
}

/// Result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Upstream `Round()`: rounds half away from zero and converts to an
/// integer like the C++ `static_cast<cInt>()` on x86-64 (`cvttsd2si`):
/// NaN and out of range values yield `i64::MIN`.
pub(crate) fn round(val: f64) -> CInt {
    if val < 0.0 {
        cast_f64(val - 0.5)
    } else {
        cast_f64(val + 0.5)
    }
}

/// C++ `static_cast<long long>(double)` as compiled for x86-64.
pub(crate) fn cast_f64(val: f64) -> CInt {
    // 2^63 is exactly representable; values in [-2^63, 2^63) are converted
    // by truncation, everything else (incl. NaN) yields the "integer
    // indefinite" value.
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&val) {
        val as CInt
    } else {
        CInt::MIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding() {
        assert_eq!(round(0.5), 1);
        assert_eq!(round(-0.5), -1);
        assert_eq!(round(1.49), 1);
        assert_eq!(round(-2.5), -3);
        assert_eq!(round(f64::NAN), i64::MIN);
        assert_eq!(round(1e30), i64::MIN);
        assert_eq!(round(-1e30), i64::MIN);
    }
}
