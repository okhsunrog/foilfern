//! Port of libs/librepcb/core/utils/clipperhelpers.{h,cpp}.
//!
//! Helpers around the [`clipper`] crate (a faithful port of Clipper 1, the
//! polygon clipping library used by upstream, see the crate documentation
//! for why no other clipping crate is used): boolean operations, offsetting,
//! conversion from/to [`Path`], and flattening of polygon trees into paths
//! without holes (holes are connected to their outline by cut-ins).
//!
//! Hand-written because the cut-in algorithm and the exact conversion
//! semantics define the geometry of planes and exports.

use clipper::{
    ClipType, Clipper, ClipperOffset, IntPoint, JoinType, PolyFillType, PolyNode, PolyTree,
    PolyType,
};

use crate::geometry::{Path, Vertex};
use crate::types::{Length, Point, PositiveLength};

/// A Clipper path (upstream `ClipperLib::Path`).
pub type ClipperPath = clipper::Path;
/// Clipper paths (upstream `ClipperLib::Paths`).
pub type ClipperPaths = clipper::Paths;

/// Errors of the [`clipper_helpers`](self) module (upstream `LogicError`s).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A Clipper operation failed.
    #[error("Failed to {operation}: {message}")]
    Clipper {
        /// The failed operation, e.g. `"unite paths"`.
        operation: &'static str,
        /// The Clipper error message.
        message: String,
    },
    /// [`intersect_all_to_tree()`] was called with less than two areas.
    #[error("Less than two areas specified.")]
    TooFewAreas,
    /// A polygon tree has holes and outlines at unexpected levels.
    #[error("Unexpected hole nesting in polygon tree.")]
    InvalidTree,
    /// No connection point for a hole cut-in was found.
    #[error("Failed to calculate the connection point of a cut-in to an outline!")]
    CutInConnection,
}

/// Result type of the [`clipper_helpers`](self) module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

fn clipper_error(operation: &'static str) -> impl FnOnce(clipper::Error) -> Error {
    move |e| Error::Clipper {
        operation,
        message: e.to_string(),
    }
}

/// Converts the result of `execute()`: like upstream (which ignores the
/// `false` return value of `Execute()`), a failed execution gives an empty
/// solution, while thrown errors (e.g. range errors) are errors.
fn solution<T: Default>(operation: &'static str, result: clipper::Result<T>) -> Result<T> {
    match result {
        Err(clipper::Error::ExecutionFailed) => Ok(T::default()),
        other => other.map_err(clipper_error(operation)),
    }
}

/// Returns whether all `points` are inside of or on `path`.
pub fn all_points_inside(points: &[IntPoint], path: &[IntPoint]) -> bool {
    points
        .iter()
        .all(|p| clipper::point_in_polygon(*p, path) != 0)
}

/// Returns whether any of `points` is strictly inside `path`.
pub fn any_points_inside(points: &[IntPoint], path: &[IntPoint]) -> bool {
    points
        .iter()
        .any(|p| clipper::point_in_polygon(*p, path) > 0)
}

/// Returns whether any point of `points` is strictly inside `path`.
pub fn any_points_of_paths_inside(points: &[ClipperPath], path: &[IntPoint]) -> bool {
    points.iter().any(|p| any_points_inside(p, path))
}

/// Unites `paths` in place.
pub fn unite(paths: &mut ClipperPaths, fill_type: PolyFillType) -> Result<()> {
    let op = "unite paths";
    let mut c = Clipper::new();
    c.add_paths(paths, PolyType::Subject, true)
        .map_err(clipper_error(op))?;
    *paths = solution(
        op,
        c.execute(ClipType::Union, fill_type, PolyFillType::EvenOdd),
    )?;
    Ok(())
}

/// Unites `subject` with `clip` in place.
pub fn unite_with(
    subject: &mut ClipperPaths,
    clip: &[ClipperPath],
    subject_fill_type: PolyFillType,
    clip_fill_type: PolyFillType,
) -> Result<()> {
    *subject = execute(
        "unite paths",
        subject,
        true,
        clip,
        ClipType::Union,
        subject_fill_type,
        clip_fill_type,
    )?;
    Ok(())
}

/// Unites `paths` and returns the result as tree.
pub fn unite_to_tree(paths: &[ClipperPath], fill_type: PolyFillType) -> Result<PolyTree> {
    execute_tree(
        "unite paths",
        paths,
        true,
        &[],
        ClipType::Union,
        fill_type,
        PolyFillType::EvenOdd,
    )
}

/// Unites `paths` with `clip` and returns the result as tree.
pub fn unite_with_to_tree(
    paths: &[ClipperPath],
    clip: &[ClipperPath],
    subject_fill_type: PolyFillType,
    clip_fill_type: PolyFillType,
) -> Result<PolyTree> {
    execute_tree(
        "unite paths",
        paths,
        true,
        clip,
        ClipType::Union,
        subject_fill_type,
        clip_fill_type,
    )
}

/// Intersects `subject` with `clip` in place.
pub fn intersect(
    subject: &mut ClipperPaths,
    clip: &[ClipperPath],
    subject_fill_type: PolyFillType,
    clip_fill_type: PolyFillType,
) -> Result<()> {
    *subject = execute(
        "intersect paths",
        subject,
        true,
        clip,
        ClipType::Intersection,
        subject_fill_type,
        clip_fill_type,
    )?;
    Ok(())
}

/// Intersects `subject` (closed or open paths) with `clip` and returns the
/// result as tree.
pub fn intersect_to_tree(
    subject: &[ClipperPath],
    clip: &[ClipperPath],
    subject_fill_type: PolyFillType,
    clip_fill_type: PolyFillType,
    closed: bool,
) -> Result<PolyTree> {
    execute_tree(
        "intersect paths",
        subject,
        closed,
        clip,
        ClipType::Intersection,
        subject_fill_type,
        clip_fill_type,
    )
}

/// Intersects all given areas (at least two) with each other (even-odd fill)
/// and returns the result as tree.
pub fn intersect_all_to_tree(areas: &[ClipperPaths]) -> Result<PolyTree> {
    // Intersection makes no sense with less than two areas.
    let [first, rest @ ..] = areas else {
        return Err(Error::TooFewAreas);
    };
    if rest.is_empty() {
        return Err(Error::TooFewAreas);
    }
    let op = "intersect paths";
    let mut result = PolyTree::new();
    let mut c = Clipper::new();
    for (i, clip) in rest.iter().enumerate() {
        c.clear();
        if i == 0 {
            c.add_paths(first, PolyType::Subject, true)
                .map_err(clipper_error(op))?;
        } else {
            let intermediate = clipper::poly_tree_to_paths(&result);
            c.add_paths(&intermediate, PolyType::Subject, true)
                .map_err(clipper_error(op))?;
        }
        c.add_paths(clip, PolyType::Clip, true)
            .map_err(clipper_error(op))?;
        result = solution(
            op,
            c.execute_tree(
                ClipType::Intersection,
                PolyFillType::EvenOdd,
                PolyFillType::EvenOdd,
            ),
        )?;
    }
    Ok(result)
}

/// Subtracts `clip` from `subject` in place.
pub fn subtract(
    subject: &mut ClipperPaths,
    clip: &[ClipperPath],
    subject_fill_type: PolyFillType,
    clip_fill_type: PolyFillType,
) -> Result<()> {
    *subject = execute(
        "subtract paths",
        subject,
        true,
        clip,
        ClipType::Difference,
        subject_fill_type,
        clip_fill_type,
    )?;
    Ok(())
}

/// Subtracts `clip` from `subject` (closed or open paths) and returns the
/// result as tree.
pub fn subtract_to_tree(
    subject: &[ClipperPath],
    clip: &[ClipperPath],
    subject_fill_type: PolyFillType,
    clip_fill_type: PolyFillType,
    closed: bool,
) -> Result<PolyTree> {
    execute_tree(
        "subtract paths",
        subject,
        closed,
        clip,
        ClipType::Difference,
        subject_fill_type,
        clip_fill_type,
    )
}

/// Offsets closed `paths` in place (miter limit 2, arc tolerance
/// `max_arc_tolerance`).
pub fn offset(
    paths: &mut ClipperPaths,
    offset: Length,
    max_arc_tolerance: PositiveLength,
    join_type: JoinType,
) -> Result<()> {
    let op = "offset a path";
    let mut o = ClipperOffset::new(2.0, max_arc_tolerance.to_nm() as f64);
    o.add_paths(paths, join_type, clipper::EndType::ClosedPolygon);
    *paths = solution(op, o.execute(offset.to_nm() as f64))?;
    Ok(())
}

/// Offsets closed `paths` (round joins) and returns the result as tree.
pub fn offset_to_tree(
    paths: &[ClipperPath],
    offset: Length,
    max_arc_tolerance: PositiveLength,
) -> Result<PolyTree> {
    let op = "offset paths";
    let mut o = ClipperOffset::new(2.0, max_arc_tolerance.to_nm() as f64);
    o.add_paths(paths, JoinType::Round, clipper::EndType::ClosedPolygon);
    solution(op, o.execute_tree(offset.to_nm() as f64))
}

/// Returns all contours of a tree.
pub fn tree_to_paths(tree: &PolyTree) -> ClipperPaths {
    clipper::poly_tree_to_paths(tree)
}

/// Converts the children of `node` (outlines with nested holes) into paths
/// without holes by connecting each hole to its outline with a cut-in.
pub fn flatten_tree(node: PolyNode<'_>) -> Result<ClipperPaths> {
    let mut paths = ClipperPaths::new();
    for outline_child in node.children() {
        if outline_child.is_hole() {
            return Err(Error::InvalidTree);
        }
        let mut holes = ClipperPaths::new();
        for hole_child in outline_child.children() {
            if !hole_child.is_hole() {
                return Err(Error::InvalidTree);
            }
            holes.push(hole_child.contour().clone());
            paths.extend(flatten_tree(hole_child)?);
        }
        paths.push(convert_holes_to_cut_ins(outline_child.contour(), &holes)?);
    }
    Ok(paths)
}

/// Converts Clipper paths to closed [`Path`]s.
pub fn paths_from_clipper(paths: &[ClipperPath]) -> Vec<Path> {
    paths.iter().map(|p| path_from_clipper(p)).collect()
}

/// Converts a Clipper path to a closed [`Path`].
pub fn path_from_clipper(path: &[IntPoint]) -> Path {
    let mut p: Path = path
        .iter()
        .map(|pt| Vertex::at(point_from_clipper(*pt)))
        .collect();
    p.close();
    p
}

/// Converts a Clipper point to a [`Point`].
pub fn point_from_clipper(point: IntPoint) -> Point {
    Point::from_nm(point.x, point.y)
}

/// Converts [`Path`]s to Clipper paths, see [`path_to_clipper()`].
pub fn paths_to_clipper(paths: &[Path], max_arc_tolerance: PositiveLength) -> ClipperPaths {
    paths
        .iter()
        .map(|p| path_to_clipper(p, max_arc_tolerance))
        .collect()
}

/// Converts a [`Path`] to a Clipper path with flattened arcs, always
/// oriented counter-clockwise (positive orientation).
pub fn path_to_clipper(path: &Path, max_arc_tolerance: PositiveLength) -> ClipperPath {
    let mut p: ClipperPath = path
        .flattened_arcs(max_arc_tolerance)
        .vertices()
        .iter()
        .map(|v| point_to_clipper(v.pos))
        .collect();
    // Make sure all paths have the same orientation, otherwise we get strange
    // results.
    if !clipper::orientation(&p) {
        clipper::reverse_path(&mut p);
    }
    p
}

/// Converts a [`Point`] to a Clipper point.
pub fn point_to_clipper(point: Point) -> IntPoint {
    IntPoint::new(point.x.to_nm(), point.y.to_nm())
}

fn execute(
    operation: &'static str,
    subject: &[ClipperPath],
    subject_closed: bool,
    clip: &[ClipperPath],
    clip_type: ClipType,
    subject_fill_type: PolyFillType,
    clip_fill_type: PolyFillType,
) -> Result<ClipperPaths> {
    let mut c = Clipper::new();
    c.add_paths(subject, PolyType::Subject, subject_closed)
        .map_err(clipper_error(operation))?;
    c.add_paths(clip, PolyType::Clip, true)
        .map_err(clipper_error(operation))?;
    solution(
        operation,
        c.execute(clip_type, subject_fill_type, clip_fill_type),
    )
}

fn execute_tree(
    operation: &'static str,
    subject: &[ClipperPath],
    subject_closed: bool,
    clip: &[ClipperPath],
    clip_type: ClipType,
    subject_fill_type: PolyFillType,
    clip_fill_type: PolyFillType,
) -> Result<PolyTree> {
    let mut c = Clipper::new();
    c.add_paths(subject, PolyType::Subject, subject_closed)
        .map_err(clipper_error(operation))?;
    c.add_paths(clip, PolyType::Clip, true)
        .map_err(clipper_error(operation))?;
    solution(
        operation,
        c.execute_tree(clip_type, subject_fill_type, clip_fill_type),
    )
}

fn convert_holes_to_cut_ins(outline: &[IntPoint], holes: &[ClipperPath]) -> Result<ClipperPath> {
    let mut path = outline.to_vec();
    for hole in prepare_holes(holes) {
        add_cut_in_to_path(&mut path, &hole)?;
    }
    // Remove duplicates which might have been created by cut-ins.
    for i in (1..path.len()).rev() {
        if path[i] == path[i - 1] {
            path.remove(i);
        }
    }
    Ok(path)
}

fn prepare_holes(holes: &[ClipperPath]) -> ClipperPaths {
    // Holes with less than 3 points are invalid and ignored (upstream prints
    // a warning).
    let mut prepared: ClipperPaths = holes
        .iter()
        .filter(|hole| hole.len() > 2)
        .map(|hole| rotate_cut_in_hole(hole))
        .collect();
    // Important: sort holes by the y coordinate of their connection point (to
    // make sure no cut-ins are overlapping in the resulting plane). Upstream
    // uses the unstable std::sort(), so ties must be ordered the same way.
    clipper::std_sort::sort_by(&mut prepared, |p1, p2| p1[0].y < p2[0].y);
    prepared
}

fn rotate_cut_in_hole(hole: &[IntPoint]) -> ClipperPath {
    let mut p = hole.to_vec();
    if p.last() == p.first() {
        p.pop();
    }
    // Like std::min_element(): the first of all minimal elements.
    let min_index = p
        .iter()
        .enumerate()
        .fold(None, |min: Option<(usize, IntPoint)>, (i, pt)| match min {
            Some((_, m)) if !((pt.y < m.y) || ((pt.y == m.y) && (pt.x < m.x))) => min,
            _ => Some((i, *pt)),
        })
        .map_or(0, |(i, _)| i);
    p.rotate_left(min_index);
    p
}

fn add_cut_in_to_path(outline: &mut ClipperPath, hole: &[IntPoint]) -> Result<()> {
    let index = insert_connection_point_to_path(outline, hole[0])?;
    outline.splice(index..index, hole.iter().copied());
    Ok(())
}

fn insert_connection_point_to_path(path: &mut ClipperPath, p: IntPoint) -> Result<usize> {
    let mut nearest: Option<(usize, IntPoint)> = None;
    for i in 0..path.len() {
        if let Some(y) = calc_intersection_pos(path[i], path[(i + 1) % path.len()], p.x)
            && (y <= p.y)
            && nearest.is_none_or(|(_, n)| p.y - y < p.y - n.y)
        {
            nearest = Some((i, IntPoint::new(p.x, y)));
        }
    }
    let (index, point) = nearest.ok_or(Error::CutInConnection)?;
    path.splice(index + 1..index + 1, [point, p, point]);
    Ok(index + 2)
}

fn calc_intersection_pos(p1: IntPoint, p2: IntPoint, x: i64) -> Option<i64> {
    if ((p1.x <= x) && (p2.x > x)) || ((p1.x >= x) && (p2.x < x)) {
        let y_calc =
            p1.y as f64 + ((x - p1.x) as f64 * (p2.y - p1.y) as f64 / (p2.x - p1.x) as f64);
        // Truncating conversion like the upstream cast to `cInt`.
        Some((y_calc as i64).clamp(p1.y.min(p2.y), p1.y.max(p2.y)))
    } else {
        None
    }
}
