//! Port of tests/unittests/core/utils/tangentpathjoinertest.cpp.

use foilfern_core::geometry::{Path, Vertex};
use foilfern_core::serialization::{List, Mode, SExpression, SerializeObject};
use foilfern_core::types::{Angle, Point};
use foilfern_core::utils::tangent_path_joiner;

fn str(paths: &[Path]) -> String {
    let mut root = List::new("paths");
    for p in paths {
        root.ensure_line_break();
        p.serialize(root.append_list("path"));
    }
    root.ensure_line_break();
    SExpression::List(root)
        .to_string_with_mode(Mode::LibrePcb)
        .unwrap()
}

fn v(x: i64, y: i64) -> Vertex {
    Vertex::at(Point::from_nm(x, y))
}

fn va(x: i64, y: i64, angle: Angle) -> Vertex {
    Vertex::new(Point::from_nm(x, y), angle)
}

fn path(vertices: &[Vertex]) -> Path {
    Path::new(vertices.to_vec())
}

fn check(input: Vec<Path>, expected: Vec<Path>) {
    let output = tangent_path_joiner::join(input, None);
    assert!(!output.timed_out);
    assert_eq!(str(&output.paths), str(&expected));
}

const A90: Angle = Angle::DEG90;
const A180: Angle = Angle::DEG180;

#[test]
fn test_empty_input() {
    check(vec![], vec![]);
}

#[test]
fn test_no_vertices() {
    check(vec![Path::default()], vec![]);
}

#[test]
fn test_only_one_vertex() {
    check(vec![path(&[v(0, 0)])], vec![]);
}

#[test]
fn test_one_closed_path() {
    let p = path(&[v(0, 0), v(1000, 0), v(1000, 1000), v(0, 0)]);
    check(vec![p.clone()], vec![p]);
}

#[test]
fn test_one_open_path() {
    let p = path(&[v(0, 0), v(1000, 0), v(1000, 1000)]);
    check(vec![p.clone()], vec![p]);
}

#[test]
fn test_two_tangent_paths() {
    check(
        vec![
            path(&[va(0, 0, A90), v(1000, 0)]),
            path(&[va(1000, 0, A180), v(1000, 1000)]),
        ],
        vec![path(&[va(0, 0, A90), va(1000, 0, A180), v(1000, 1000)])],
    );
}

#[test]
fn test_two_tangent_paths_first_reversed() {
    check(
        vec![
            path(&[va(1000, 0, A90), v(0, 0)]),
            path(&[va(1000, 0, A180), v(1000, 1000)]),
        ],
        vec![path(&[va(0, 0, -A90), va(1000, 0, A180), v(1000, 1000)])],
    );
}

#[test]
fn test_two_tangent_paths_second_reversed() {
    check(
        vec![
            path(&[va(0, 0, A90), v(1000, 0)]),
            path(&[va(1000, 1000, A180), v(1000, 0)]),
        ],
        vec![path(&[va(0, 0, A90), va(1000, 0, -A180), v(1000, 1000)])],
    );
}

#[test]
fn test_two_tangent_paths_both_reversed() {
    check(
        vec![
            path(&[va(1000, 0, A90), v(0, 0)]),
            path(&[va(1000, 1000, A180), v(1000, 0)]),
        ],
        vec![path(&[va(1000, 1000, A180), va(1000, 0, A90), v(0, 0)])],
    );
}

#[test]
fn test_two_nested_rects() {
    check(
        vec![
            path(&[v(0, 0), v(0, 1000), v(1000, 1000)]),
            path(&[v(0, 0), v(1000, 0)]),
            path(&[v(1000, 0), v(1000, 1000)]),
            path(&[v(1000, 0), v(2000, 0), v(2000, 1000), v(1000, 1000)]),
        ],
        vec![
            path(&[
                v(1000, 1000),
                v(0, 1000),
                v(0, 0),
                v(1000, 0),
                v(2000, 0),
                v(2000, 1000),
                v(1000, 1000),
            ]),
            path(&[v(1000, 0), v(1000, 1000)]),
        ],
    );
}

#[test]
fn test_several_tangent_and_non_tangent_paths() {
    check(
        vec![
            // Path 2, Segment 2
            path(&[v(1000, 0), va(1000, 1000, A90), v(2000, 1000)]),
            // Path 1 (closed)
            path(&[va(0, 0, A90), v(1000, 0), v(1000, 1000), v(0, 0)]),
            // Path 3 (open)
            path(&[va(5000, 5000, A90), v(6000, 6000), v(7000, 7000)]),
            // Path 2, Segment 1
            path(&[v(0, 0), v(1000, 0)]),
            // Path 2, Segment 4 (reversed)
            path(&[v(4000, 1000), va(3000, 1000, A90), v(3000, 0), v(2000, 0)]),
            // Path 2, Segment 3
            path(&[va(2000, 1000, A90), v(2000, 0)]),
        ],
        vec![
            // Path 1 (closed)
            path(&[va(0, 0, A90), v(1000, 0), v(1000, 1000), v(0, 0)]),
            // Path 2 (joined)
            path(&[
                v(0, 0),
                v(1000, 0),
                va(1000, 1000, A90),
                va(2000, 1000, A90),
                v(2000, 0),
                va(3000, 0, -A90),
                v(3000, 1000),
                v(4000, 1000),
            ]),
            // Path 3 (open)
            path(&[va(5000, 5000, A90), v(6000, 6000), v(7000, 7000)]),
        ],
    );
}

fn many_vertices(i: i64, extra: bool) -> Vec<Vertex> {
    let mut vertices = vec![v(i, 0), va(i, 1, A90)];
    vertices.extend((2..=17).map(|k| v(i, k * 1000)));
    if extra {
        vertices.push(v(i + 1, 0));
    }
    vertices
}

// For testing performance with huge input.
#[test]
fn test_many_independent_paths() {
    let input: Vec<Path> = (0..1000).map(|i| path(&many_vertices(i, false))).collect();
    check(input.clone(), input);
}

// For testing performance with huge input.
#[test]
fn test_many_tangent_paths() {
    let mut input = Vec::new();
    let mut expected_vertices: Vec<Vertex> = Vec::new();
    for i in 0..1000 {
        let vertices = many_vertices(i, true);
        input.push(path(&vertices));
        expected_vertices.pop();
        expected_vertices.extend(vertices);
    }
    check(input, vec![Path::new(expected_vertices)]);
}
