//! Readable unit tests of the public API. The expected values were produced
//! by the original C++ library.

use clipper::{
    ClipType, Clipper, ClipperOffset, EndType, Error, IntPoint, JoinType, Path, Paths,
    PolyFillType, PolyType,
};

fn path(coords: &[(i64, i64)]) -> Path {
    coords.iter().map(|&(x, y)| IntPoint::new(x, y)).collect()
}

fn square(x: i64, y: i64, size: i64) -> Path {
    path(&[(x, y), (x + size, y), (x + size, y + size), (x, y + size)])
}

fn clip(ct: ClipType, subject: &[Path], clip: &[Path]) -> Paths {
    let mut c = Clipper::new();
    c.add_paths(subject, PolyType::Subject, true).unwrap();
    c.add_paths(clip, PolyType::Clip, true).unwrap();
    c.execute(ct, PolyFillType::EvenOdd, PolyFillType::EvenOdd)
        .unwrap()
}

#[test]
fn boolean_operations() {
    let a = [square(0, 0, 10)];
    let b = [square(5, 5, 10)];
    assert_eq!(
        clip(ClipType::Union, &a, &b),
        [path(&[
            (10, 5),
            (15, 5),
            (15, 15),
            (5, 15),
            (5, 10),
            (0, 10),
            (0, 0),
            (10, 0)
        ])]
    );
    assert_eq!(
        clip(ClipType::Intersection, &a, &b),
        [path(&[(10, 10), (5, 10), (5, 5), (10, 5)])]
    );
    assert_eq!(
        clip(ClipType::Difference, &a, &b),
        [path(&[(10, 5), (5, 5), (5, 10), (0, 10), (0, 0), (10, 0)])]
    );
    assert_eq!(
        clip(ClipType::Xor, &a, &b),
        [
            path(&[(15, 15), (5, 15), (5, 10), (10, 10), (10, 5), (15, 5)]),
            path(&[(10, 5), (5, 5), (5, 10), (0, 10), (0, 0), (10, 0)]),
        ]
    );
}

#[test]
fn union_with_fill_types() {
    let paths = [square(0, 0, 10), square(5, 5, 10)];
    let mut c = Clipper::new();
    c.add_paths(&paths, PolyType::Subject, true).unwrap();
    // Even-odd: the overlapping area is not filled.
    assert_eq!(
        c.execute(
            ClipType::Union,
            PolyFillType::EvenOdd,
            PolyFillType::EvenOdd
        )
        .unwrap()
        .len(),
        2
    );
    // Non-zero: the overlapping area is filled. The clipper can be reused.
    assert_eq!(
        c.execute(
            ClipType::Union,
            PolyFillType::NonZero,
            PolyFillType::NonZero
        )
        .unwrap(),
        [path(&[
            (10, 5),
            (15, 5),
            (15, 15),
            (5, 15),
            (5, 10),
            (0, 10),
            (0, 0),
            (10, 0)
        ])]
    );
}

#[test]
fn poly_tree_with_hole() {
    let mut c = Clipper::new();
    c.add_path(&square(0, 0, 30), PolyType::Subject, true)
        .unwrap();
    c.add_path(&square(10, 10, 10), PolyType::Clip, true)
        .unwrap();
    let tree = c
        .execute_tree(
            ClipType::Difference,
            PolyFillType::EvenOdd,
            PolyFillType::EvenOdd,
        )
        .unwrap();
    assert_eq!(tree.total(), 2);
    assert!(tree.root().is_hole()); // Like upstream.
    let outline = tree.first().unwrap();
    assert!(!outline.is_hole());
    assert_eq!(
        outline.contour(),
        &path(&[(30, 30), (0, 30), (0, 0), (30, 0)])
    );
    let hole = outline.children().next().unwrap();
    assert!(hole.is_hole());
    assert_eq!(hole.parent(), Some(outline));
    assert_eq!(
        hole.contour(),
        &path(&[(10, 10), (10, 20), (20, 20), (20, 10)])
    );
    assert_eq!(outline.next(), Some(hole));
    assert_eq!(hole.next(), None);
    assert_eq!(
        clipper::poly_tree_to_paths(&tree),
        [outline.contour().clone(), hole.contour().clone()]
    );
}

#[test]
fn open_path_clipping() {
    let mut c = Clipper::new();
    c.add_path(&path(&[(0, 5), (20, 5), (40, 5)]), PolyType::Subject, false)
        .unwrap();
    c.add_path(
        &path(&[(10, 0), (30, 0), (30, 10), (10, 10)]),
        PolyType::Clip,
        true,
    )
    .unwrap();
    assert_eq!(
        c.execute(
            ClipType::Intersection,
            PolyFillType::EvenOdd,
            PolyFillType::EvenOdd
        ),
        Err(Error::PolyTreeNeededForOpenPaths)
    );
    let tree = c
        .execute_tree(
            ClipType::Intersection,
            PolyFillType::EvenOdd,
            PolyFillType::EvenOdd,
        )
        .unwrap();
    assert_eq!(
        clipper::open_paths_from_poly_tree(&tree),
        [path(&[(30, 5), (20, 5), (10, 5)])]
    );
    assert!(clipper::closed_paths_from_poly_tree(&tree).is_empty());
}

#[test]
fn errors() {
    let mut c = Clipper::new();
    assert_eq!(
        c.add_path(&square(0, 0, 10), PolyType::Clip, false),
        Err(Error::OpenPathMustBeSubject)
    );
    assert_eq!(
        c.add_path(
            &square(0x3FFF_FFFF_FFFF_FFFF, 0, 10),
            PolyType::Subject,
            true
        ),
        Err(Error::CoordinateOutOfRange)
    );
    // Large coordinates within the range are fine.
    assert_eq!(
        c.add_path(
            &square(0x3FFF_FFFF_0000_0000, 0, 10),
            PolyType::Subject,
            true
        ),
        Ok(true)
    );
    // Degenerate paths are ignored.
    assert_eq!(
        c.add_path(&path(&[(0, 0), (1, 1)]), PolyType::Subject, true),
        Ok(false)
    );
    // Nothing to do.
    assert_eq!(
        Clipper::new().execute(
            ClipType::Union,
            PolyFillType::EvenOdd,
            PolyFillType::EvenOdd
        ),
        Ok(vec![])
    );
}

#[test]
fn offset() {
    let mut o = ClipperOffset::new(2.0, 0.25);
    o.add_path(&square(0, 0, 10), JoinType::Round, EndType::ClosedPolygon);
    assert_eq!(
        o.execute(2.0).unwrap(),
        [path(&[
            (12, -1),
            (12, 10),
            (11, 12),
            (0, 12),
            (-2, 11),
            (-2, 0),
            (-1, -2),
            (10, -2)
        ])]
    );

    let mut o = ClipperOffset::default();
    o.add_path(&square(0, 0, 10), JoinType::Square, EndType::ClosedPolygon);
    assert_eq!(
        o.execute(-2.0).unwrap(),
        [path(&[(8, 8), (2, 8), (2, 2), (8, 2)])]
    );

    let mut o = ClipperOffset::default();
    o.add_path(
        &path(&[(0, 0), (10, 0)]),
        JoinType::Square,
        EndType::OpenButt,
    );
    assert_eq!(
        o.execute(1.0).unwrap(),
        [path(&[(10, 1), (0, 1), (0, -1), (10, -1)])]
    );
}

#[test]
fn point_in_polygon() {
    let sq = square(0, 0, 10);
    assert_eq!(clipper::point_in_polygon(IntPoint::new(5, 5), &sq), 1);
    assert_eq!(clipper::point_in_polygon(IntPoint::new(15, 5), &sq), 0);
    assert_eq!(clipper::point_in_polygon(IntPoint::new(10, 5), &sq), -1);
    assert_eq!(clipper::point_in_polygon(IntPoint::new(0, 0), &sq), -1);
}

#[test]
fn orientation_and_area() {
    let mut sq = square(0, 0, 10);
    assert_eq!(clipper::area(&sq), 100.0);
    assert!(clipper::orientation(&sq));
    clipper::reverse_path(&mut sq);
    assert_eq!(clipper::area(&sq), -100.0);
    assert!(!clipper::orientation(&sq));
    assert_eq!(clipper::area(&path(&[(0, 0), (1, 1)])), 0.0);
}

#[test]
fn std_sort_order_of_ties() {
    // Order produced by libstdc++'s std::sort() for (key, index) pairs
    // compared by key only.
    let keys = [3, 1, 3, 2, 1, 3, 2, 2, 1, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 2];
    let mut v: Vec<(i32, usize)> = keys.iter().copied().zip(0..).collect();
    clipper::std_sort::sort_by(&mut v, |a, b| a.0 < b.0);
    let order: Vec<usize> = v.iter().map(|x| x.1).collect();
    assert_eq!(
        order,
        [
            1, 16, 13, 10, 8, 4, 6, 7, 3, 11, 14, 17, 19, 5, 9, 12, 2, 15, 0, 18
        ]
    );
}
