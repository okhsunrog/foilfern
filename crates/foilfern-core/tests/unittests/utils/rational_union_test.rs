//! Correctness tests of the exact copper union (not an upstream port).

use clipper::PolyFillType::{EvenOdd, Negative, NonZero, Positive};
use dashu_ratio::RBig;
use foilfern_core::utils::rational_union::{self, Error, Operand, Point, Region};

fn polygon(points: &[(i64, i64)]) -> Vec<Point> {
    points.iter().map(|&(x, y)| Point::from_nm(x, y)).collect()
}
fn rect(x: i64, y: i64, w: i64, h: i64) -> Vec<Point> {
    polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
}
fn operand(path: Vec<Point>) -> Operand {
    Operand {
        paths: vec![path],
        fill: NonZero,
    }
}
fn union(operands: &[Operand]) -> Region {
    rational_union::union(operands).unwrap().0
}
fn area(region: &Region) -> RBig {
    region
        .paths()
        .iter()
        .map(|p| {
            p.iter()
                .zip(p.iter().cycle().skip(1))
                .take(p.len())
                .map(|(a, b)| &a.x * &b.y - &a.y * &b.x)
                .fold(RBig::ZERO, |sum, n| sum + n)
        })
        .fold(RBig::ZERO, |sum, n| sum + n)
        / RBig::from(2)
}

#[test]
fn overlapping_and_touching_rectangles() {
    let a = operand(rect(0, 0, 10, 10));
    assert_eq!(
        area(&union(&[a.clone(), operand(rect(5, 0, 10, 10))])),
        RBig::from(150)
    );
    let shared = union(&[a.clone(), operand(rect(10, 0, 10, 10))]);
    assert_eq!(shared.paths(), &[rect(0, 0, 20, 10)]);
    let corner = union(&[a.clone(), operand(rect(10, 10, 10, 10))]);
    assert_eq!(corner.paths().len(), 2);
    assert_eq!(area(&corner), RBig::from(200));
    let partial = union(&[a, operand(rect(10, 3, 5, 4))]);
    assert_eq!(partial.paths().len(), 1);
    assert_eq!(area(&partial), RBig::from(120));
}

#[test]
fn holes_fill_rules_and_operand_isolation() {
    let outer = rect(0, 0, 20, 20);
    let hole = rect(5, 5, 10, 10);
    let donut = Operand {
        paths: vec![outer.clone(), hole.clone()],
        fill: EvenOdd,
    };
    let result = union(std::slice::from_ref(&donut));
    assert_eq!(result.paths().len(), 2);
    assert_eq!(area(&result), RBig::from(300));
    let island = union(&[donut.clone(), operand(rect(8, 8, 4, 4))]);
    assert_eq!(island.paths().len(), 3);
    assert_eq!(area(&island), RBig::from(316));
    let filled = union(&[donut, operand(hole.clone())]);
    assert_eq!(filled.paths(), std::slice::from_ref(&outer));
    assert_eq!(
        area(&union(&[Operand {
            paths: vec![outer.clone(), hole],
            fill: NonZero
        }])),
        RBig::from(400)
    );
    let mut reverse = outer.clone();
    reverse.reverse();
    assert_eq!(
        area(&union(&[operand(outer.clone()), operand(reverse.clone())])),
        RBig::from(400)
    );
    assert!(
        union(&[Operand {
            paths: vec![outer.clone(), reverse.clone()],
            fill: NonZero
        }])
        .paths()
        .is_empty()
    );
    assert!(
        union(&[Operand {
            paths: vec![outer.clone(), outer.clone()],
            fill: EvenOdd
        }])
        .paths()
        .is_empty()
    );
    assert!(
        union(&[Operand {
            paths: vec![reverse.clone()],
            fill: Positive
        }])
        .paths()
        .is_empty()
    );
    assert_eq!(
        area(&union(&[Operand {
            paths: vec![reverse],
            fill: Negative
        }])),
        RBig::from(400)
    );
}

#[test]
fn self_intersection_and_multiple_edges_at_one_vertex() {
    let bow = Operand {
        paths: vec![polygon(&[(0, 0), (10, 10), (0, 10), (10, 0)])],
        fill: EvenOdd,
    };
    let result = union(&[bow]);
    assert_eq!(result.paths().len(), 2);
    assert_eq!(area(&result), RBig::from(50));
    let pieces = [
        operand(rect(-10, -10, 10, 10)),
        operand(rect(0, 0, 10, 10)),
        operand(rect(-10, 0, 10, 10)),
        operand(rect(0, -10, 10, 10)),
    ];
    assert_eq!(union(&pieces).paths(), &[rect(-10, -10, 20, 20)]);
}

#[test]
fn fractional_vertices_survive_repeated_unions() {
    let a = operand(polygon(&[(0, 0), (9, 0), (0, 7)]));
    let b = operand(polygon(&[(1, -2), (5, 10), (8, -2)]));
    let c = operand(polygon(&[(-2, 4), (12, 3), (4, 12)]));
    let ab = union(&[a.clone(), b.clone()]);
    assert!(
        ab.paths()
            .iter()
            .flatten()
            .any(|p| !p.x.is_int() || !p.y.is_int())
    );
    let direct = union(&[a.clone(), b.clone(), c.clone()]);
    assert_eq!(direct, union(&[ab.operand(), c.clone()]));
    assert_eq!(
        direct,
        union(&[a.clone(), union(&[b.clone(), c.clone()]).operand()])
    );
    assert_eq!(direct, union(&[c, b, a]));
    assert_eq!(direct, union(&[direct.operand()]));
    let fractional = Operand {
        paths: vec![vec![
            Point::new(RBig::ONE / RBig::from(3), RBig::ZERO),
            Point::from_nm(2, 0),
            Point::from_nm(2, 2),
            Point::new(RBig::ONE / RBig::from(3), RBig::from(2)),
        ]],
        fill: NonZero,
    };
    assert_eq!(area(&union(&[fractional])), RBig::from(10) / RBig::from(3));
}

#[test]
fn full_coordinate_range_and_degenerate_inputs() {
    let m = 0x3fff_ffff_ffff_ffff;
    let a = operand(polygon(&[(-m, -m), (m, m), (-m, m)]));
    let b = operand(polygon(&[(-m, -m), (m, -m), (m, m)]));
    let result = union(&[a, b]);
    assert_eq!(area(&result), RBig::from(m) * RBig::from(m) * RBig::from(4));
    assert_eq!(result.rounded_paths().unwrap().len(), 1);
    let crossing = union(&[Operand {
        paths: vec![polygon(&[(-m, -m), (m, m), (-m, m), (m, -m)])],
        fill: EvenOdd,
    }]);
    assert_eq!(crossing.paths().len(), 2);
    assert!(
        crossing
            .paths()
            .iter()
            .flatten()
            .any(|p| p == &Point::from_nm(0, 0))
    );
    assert_eq!(
        area(&crossing),
        RBig::from(m) * RBig::from(m) * RBig::from(2)
    );
    assert!(matches!(
        rational_union::union(&[operand(rect(m, 0, 1, 1))]),
        Err(Error::CoordinateOutOfRange)
    ));
    assert!(
        union(&[
            operand(vec![]),
            operand(polygon(&[(0, 0), (1, 0), (2, 0), (0, 0)]))
        ])
        .paths()
        .is_empty()
    );
}

#[test]
fn explicit_rounding_uses_half_away_from_zero() {
    let half = RBig::ONE / RBig::from(2);
    let result = union(&[Operand {
        paths: vec![vec![
            Point::new(-&half, -&half),
            Point::new(half.clone(), -&half),
            Point::new(half.clone(), half.clone()),
            Point::new(-&half, half),
        ]],
        fill: NonZero,
    }]);
    let rounded = result.rounded_paths().unwrap();
    assert_eq!(
        rounded[0],
        vec![
            clipper::IntPoint::new(-1, -1),
            clipper::IntPoint::new(1, -1),
            clipper::IntPoint::new(1, 1),
            clipper::IntPoint::new(-1, 1)
        ]
    );
}

#[test]
fn constructed_half_grid_intersection_has_no_float_rounding_drift() {
    // The earlier scalar probe's floating-point line formula rounds y down
    // by 1 nm for this intersection. Both exact coordinates are half integers.
    let result = union(&[Operand {
        paths: vec![polygon(&[
            (620776482, 382053621),
            (455267721, 359395578),
            (451017960, 427449599),
            (625026243, 313999600),
        ])],
        fill: EvenOdd,
    }]);
    let half = RBig::ONE / RBig::from(2);
    let intersection = Point::new(RBig::from(538022101) + &half, RBig::from(370724599) + &half);
    assert!(result.paths().iter().flatten().any(|p| p == &intersection));
    assert!(
        result
            .rounded_paths()
            .unwrap()
            .iter()
            .flatten()
            .any(|p| p == &clipper::IntPoint::new(538022102, 370724600))
    );
}

#[test]
fn final_rounding_can_close_a_real_gap() {
    let left = operand(rect(0, 0, 1, 2));
    let right = operand(vec![
        Point::new(RBig::from(4) / RBig::from(3), RBig::ZERO),
        Point::from_nm(3, 0),
        Point::from_nm(3, 2),
        Point::new(RBig::from(4) / RBig::from(3), RBig::from(2)),
    ]);
    let exact = union(&[left, right]);
    assert_eq!(exact.paths().len(), 2);
    let rounded = union(&[Operand::from_integer(
        &exact.rounded_paths().unwrap(),
        NonZero,
    )]);
    assert_eq!(rounded.paths().len(), 1);
    assert_eq!(area(&exact), RBig::from(16) / RBig::from(3));
    assert_eq!(area(&rounded), RBig::from(6));
}

#[test]
fn rectangle_union_matches_independent_cell_coverage() {
    let mut state = 17_u64;
    let mut random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((state >> 32) % 8) as i64
    };
    for _ in 0..100 {
        let rectangles: Vec<_> = (0..5)
            .map(|_| (random(), random(), random() + 1, random() + 1))
            .collect();
        let operands: Vec<_> = rectangles
            .iter()
            .map(|&(x, y, w, h)| operand(rect(x, y, w, h)))
            .collect();
        let expected = (0..16)
            .flat_map(|x| (0..16).map(move |y| (x, y)))
            .filter(|&(x, y)| {
                rectangles
                    .iter()
                    .any(|&(a, b, w, h)| a <= x && x < a + w && b <= y && y < b + h)
            })
            .count();
        let result = union(&operands);
        assert_eq!(area(&result), RBig::from(expected));
        let mut reversed = operands;
        reversed.reverse();
        assert_eq!(result, union(&reversed));
    }
}

// Independent point-location oracle; the implementation classifies edges with
// a sweep, so this ray-crossing test does not reuse its winding bookkeeping.
fn winding(path: &[Point], p: &Point) -> Option<i64> {
    let mut count = 0;
    for (a, b) in path
        .iter()
        .zip(path.iter().cycle().skip(1))
        .take(path.len())
    {
        let determinant = (&b.x - &a.x) * (&p.y - &a.y) - (&b.y - &a.y) * (&p.x - &a.x);
        if determinant == RBig::ZERO
            && p.x >= a.x.clone().min(b.x.clone())
            && p.x <= a.x.clone().max(b.x.clone())
            && p.y >= a.y.clone().min(b.y.clone())
            && p.y <= a.y.clone().max(b.y.clone())
        {
            return None;
        }
        if a.y <= p.y && b.y > p.y && determinant > RBig::ZERO {
            count += 1;
        }
        if a.y > p.y && b.y <= p.y && determinant < RBig::ZERO {
            count -= 1;
        }
    }
    Some(count)
}

#[test]
fn exact_union_matches_independent_point_location() {
    let mut state = 91_u64;
    let mut random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((state >> 32) % 12) as i64
    };
    for case in 0..256 {
        let operands: Vec<_> = (0..3)
            .map(|i| Operand {
                paths: vec![(0..5).map(|_| Point::from_nm(random(), random())).collect()],
                fill: [EvenOdd, NonZero, Positive, Negative][(case + i) % 4],
            })
            .collect();
        let result = union(&operands);
        for x in 0..5 {
            for y in 0..5 {
                let p = Point::new(
                    RBig::from(x * 2) + RBig::ONE / RBig::from(7),
                    RBig::from(y * 2) + RBig::ONE / RBig::from(3),
                );
                let expected: Option<Vec<_>> = operands
                    .iter()
                    .map(|op| {
                        winding(&op.paths[0], &p).map(|w| match op.fill {
                            EvenOdd => w % 2 != 0,
                            NonZero => w != 0,
                            Positive => w > 0,
                            Negative => w < 0,
                        })
                    })
                    .collect();
                let actual: Option<Vec<_>> = result
                    .paths()
                    .iter()
                    .map(|path| winding(path, &p))
                    .collect();
                if let (Some(expected), Some(actual)) = (expected, actual) {
                    assert_eq!(
                        expected.into_iter().any(|v| v),
                        actual.into_iter().sum::<i64>() != 0,
                        "case {case}, point {p:?}"
                    );
                }
            }
        }
        assert_eq!(
            result,
            union(&[union(&operands[..2]).operand(), operands[2].clone()]),
            "case {case}"
        );
    }
}

#[test]
fn boolean_checks_preserve_sub_nanometer_gaps_and_overlap() {
    let left = operand(rect(0, 0, 1, 2));
    let right = operand(vec![
        Point::new(RBig::from(4) / RBig::from(3), RBig::ZERO),
        Point::from_nm(3, 0),
        Point::from_nm(3, 2),
        Point::new(RBig::from(4) / RBig::from(3), RBig::from(2)),
    ]);
    let copper = union(&[left, right]);
    let required = union(&[operand(rect(0, 0, 3, 2))]);
    let missing = required.difference(&copper).unwrap();
    assert_eq!(area(&missing), RBig::from(2) / RBig::from(3));
    // Annular-ring coverage must still fail after its display collapses to a line.
    assert!(!missing.is_empty());
    assert!(
        union(&[Operand::from_integer(
            &missing.rounded_paths().unwrap(),
            NonZero
        )])
        .is_empty()
    );
    let clip = union(&[operand(rect(1, 0, 1, 2))]);
    assert_eq!(
        area(&copper.intersection(&clip).unwrap()),
        RBig::from(4) / RBig::from(3)
    );
    assert_eq!(
        area(&copper.intersection(&required).unwrap()),
        area(&copper)
    );
    assert!(copper.difference(&required).unwrap().is_empty());
    assert!(
        rational_union::intersection(&[copper.operand(), Region::default().operand()])
            .unwrap()
            .is_empty()
    );
    assert!(rational_union::intersection(&[]).unwrap().is_empty());
    let touching = union(&[operand(rect(3, 0, 2, 2))]);
    assert!(copper.intersection(&touching).unwrap().is_empty());
}

#[test]
fn boolean_area_identities_and_holes() {
    for i in 0..64 {
        let a = union(&[operand(polygon(&[(0, 0), (11, 0), (i % 7, 13)]))]);
        let b = union(&[operand(polygon(&[(i % 11, -2), (12, 6), (-1, 5)]))]);
        let overlap = a.intersection(&b).unwrap();
        assert_eq!(
            overlap,
            rational_union::intersection(&[a.operand(), b.operand()]).unwrap()
        );
        let difference = a.difference(&b).unwrap();
        assert_eq!(area(&difference) + area(&overlap), area(&a));
        assert_eq!(
            area(&union(&[a.operand(), b.operand()])) + area(&overlap),
            area(&a) + area(&b)
        );
        assert_eq!(overlap, b.intersection(&a).unwrap());
        assert!(difference.intersection(&b).unwrap().is_empty());
    }
    let outer = union(&[operand(rect(0, 0, 20, 20))]);
    let hole = union(&[operand(rect(5, 5, 10, 10))]);
    let donut = outer.difference(&hole).unwrap();
    assert_eq!(donut.paths().len(), 2);
    assert_eq!(area(&donut), RBig::from(300));
    assert!(donut.intersection(&hole).unwrap().is_empty());
}

#[test]
fn exact_open_boundaries_keep_fractional_intervals_and_contacts() {
    let copper = union(&[operand(rect(0, 0, 2, 2))]);
    let path = polygon(&[(-1, 1), (3, 1)]);
    assert_eq!(
        copper.clip_polyline(&path, true).unwrap(),
        vec![polygon(&[(0, 1), (2, 1)])]
    );
    assert_eq!(
        copper.clip_polyline(&path, false).unwrap(),
        vec![polygon(&[(-1, 1), (0, 1)]), polygon(&[(2, 1), (3, 1)])]
    );
    let boundary = polygon(&[(0, 0), (2, 0)]);
    assert_eq!(
        copper.clip_polyline(&boundary, true).unwrap(),
        vec![boundary.clone()]
    );
    assert!(copper.clip_polyline(&boundary, false).unwrap().is_empty());
    assert!(
        copper
            .clip_polyline(&polygon(&[(-1, 1), (1, 3)]), true)
            .unwrap()
            .is_empty()
    );
    let triangle = union(&[operand(polygon(&[(0, 0), (1, 0), (0, 3)]))]);
    let clipped = triangle
        .clip_polyline(&polygon(&[(-1, 2), (1, 2)]), true)
        .unwrap();
    assert_eq!(
        clipped,
        vec![vec![
            Point::from_nm(0, 2),
            Point::new(RBig::ONE / RBig::from(3), RBig::from(2))
        ]]
    );
    let mut reversed = polygon(&[(-1, 2), (1, 2)]);
    reversed.reverse();
    let mut expected = clipped[0].clone();
    expected.reverse();
    assert_eq!(
        triangle.clip_polyline(&reversed, true).unwrap(),
        vec![expected]
    );
}

#[test]
fn query_box_clipping_preserves_disconnected_pieces_and_holes() {
    let u = union(&[operand(polygon(&[
        (0, 0),
        (9, 0),
        (9, 9),
        (6, 9),
        (6, 3),
        (3, 3),
        (3, 9),
        (0, 9),
    ]))]);
    let strip = union(&[operand(rect(-1, 5, 11, 2))]);
    let clipped = u.intersection(&strip).unwrap();
    assert_eq!(clipped.paths().len(), 2);
    assert_eq!(area(&clipped), RBig::from(12));
    assert_eq!(
        clipped,
        rational_union::intersection(&[u.operand(), strip.operand()]).unwrap()
    );
    let outer = union(&[operand(rect(-100, -100, 200, 200))]);
    let hole = union(&[operand(rect(1, 1, 6, 6))]);
    let donut = outer.difference(&hole).unwrap();
    for query in [rect(0, 0, 9, 9), rect(2, 2, 2, 2), rect(0, 3, 9, 2)] {
        let q = union(&[operand(query)]);
        assert_eq!(
            donut.intersection(&q).unwrap(),
            rational_union::intersection(&[donut.operand(), q.operand()]).unwrap()
        );
        assert_eq!(
            q.difference(&donut).unwrap(),
            q.intersection(&hole).unwrap()
        );
    }
}
