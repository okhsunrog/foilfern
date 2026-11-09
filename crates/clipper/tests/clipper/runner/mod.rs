//! Executes a test case given in the text protocol of the C++ oracle which
//! was used to verify this port (one case per line, one result line per
//! case), see `golden.rs`.
//!
//! Case formats (paths are `count` followed by `n x1 y1 ... xn yn` each):
//!
//! - `C <clip type> <subj fill> <clip fill> <flags> <subject paths> <clip paths>`:
//!   boolean operation. Flags: 1 = reverse solution, 2 = strictly simple,
//!   4 = preserve collinear, 8 = PolyTree output, 16 = open subject paths,
//!   32 = execute a second operation on the same clipper.
//! - `O <miter limit> <arc tolerance> <delta> <tree> <groups>` followed by
//!   `<join type> <end type> <paths>` per group: offset.
//! - `P <x> <y> <path>`: point in polygon.
//! - `A <path>`: area (as hex bits) and orientation.
//! - `L <distance> <path>`: clean polygon.
//! - `S <fill> <paths>`: simplify polygons.
//! - `M <0=sum|1=sum paths|2=diff> <closed> <pattern> <paths>`: Minkowski.
//! - `Q <n> <keys...>`: `std::sort()` of `(key, index)` pairs by key,
//!   returns the index order.

#![allow(dead_code)]

use clipper::{
    ClipType, Clipper, ClipperOffset, EndType, Error, IntPoint, JoinType, Path, Paths,
    PolyFillType, PolyNode, PolyTree, PolyType,
};

struct Tokens<'a>(std::str::SplitAsciiWhitespace<'a>);

impl Tokens<'_> {
    fn next<T: std::str::FromStr>(&mut self) -> T {
        self.0
            .next()
            .and_then(|t| t.parse().ok())
            .expect("malformed test case")
    }

    fn path(&mut self) -> Path {
        let n: usize = self.next();
        (0..n)
            .map(|_| {
                let x = self.next();
                let y = self.next();
                IntPoint::new(x, y)
            })
            .collect()
    }

    fn paths(&mut self) -> Paths {
        let n: usize = self.next();
        (0..n).map(|_| self.path()).collect()
    }
}

fn clip_type(i: u32) -> ClipType {
    [
        ClipType::Intersection,
        ClipType::Union,
        ClipType::Difference,
        ClipType::Xor,
    ][i as usize % 4]
}

fn fill_type(i: u32) -> PolyFillType {
    [
        PolyFillType::EvenOdd,
        PolyFillType::NonZero,
        PolyFillType::Positive,
        PolyFillType::Negative,
    ][i as usize]
}

fn join_type(i: u32) -> JoinType {
    [JoinType::Square, JoinType::Round, JoinType::Miter][i as usize]
}

fn end_type(i: u32) -> EndType {
    [
        EndType::ClosedPolygon,
        EndType::ClosedLine,
        EndType::OpenButt,
        EndType::OpenSquare,
        EndType::OpenRound,
    ][i as usize]
}

fn write_path(out: &mut String, p: &[IntPoint]) {
    out.push_str(&format!(" {}", p.len()));
    for pt in p {
        out.push_str(&format!(" {} {}", pt.x, pt.y));
    }
}

fn write_paths(out: &mut String, ps: &[Path]) {
    out.push_str(&format!(" {}", ps.len()));
    for p in ps {
        write_path(out, p);
    }
}

fn write_node(out: &mut String, n: PolyNode<'_>) {
    out.push_str(" (");
    out.push(if n.is_hole() { 'h' } else { 'o' });
    out.push(if n.is_open() { 'O' } else { 'C' });
    write_path(out, n.contour());
    for c in n.children() {
        write_node(out, c);
    }
    out.push_str(" )");
}

fn write_tree(out: &mut String, t: &PolyTree) {
    out.push_str(&format!("T {}", t.total()));
    write_node(out, t.root());
    let mut cnt = 0;
    let mut n = t.first();
    while let Some(node) = n {
        cnt += 1;
        n = node.next();
    }
    out.push_str(&format!(" next {cnt}"));
}

fn write_error(out: &mut String, e: &Error) {
    match e {
        Error::ExecutionFailed => out.push('F'),
        e => out.push_str(&format!("X {e}")),
    }
}

fn write_paths_result(out: &mut String, r: clipper::Result<Paths>) {
    match r {
        Ok(p) => {
            out.push('R');
            write_paths(out, &p);
        }
        Err(e) => write_error(out, &e),
    }
}

fn write_tree_result(out: &mut String, r: clipper::Result<PolyTree>) {
    match r {
        Ok(t) => write_tree(out, &t),
        Err(e) => write_error(out, &e),
    }
}

/// Runs one test case line and returns the result line.
pub fn run_case(line: &str) -> String {
    let mut t = Tokens(line.split_ascii_whitespace());
    let mut out = String::new();
    let op: String = t.next();
    match op.as_str() {
        "C" => {
            let ct: u32 = t.next();
            let sft: u32 = t.next();
            let cft: u32 = t.next();
            let flags: u32 = t.next();
            let subj = t.paths();
            let clip = t.paths();
            let mut c = Clipper::new();
            c.set_reverse_solution(flags & 1 != 0);
            c.set_strictly_simple(flags & 2 != 0);
            c.set_preserve_collinear(flags & 4 != 0);
            let added = c
                .add_paths(&subj, PolyType::Subject, flags & 16 == 0)
                .and_then(|_| c.add_paths(&clip, PolyType::Clip, true));
            if let Err(e) = added {
                write_error(&mut out, &e);
                return out;
            }
            let (ft1, ft2) = (fill_type(sft), fill_type(cft));
            if flags & 8 != 0 {
                write_tree_result(&mut out, c.execute_tree(clip_type(ct), ft1, ft2));
                if flags & 32 != 0 {
                    out.push_str(" |");
                    write_tree_result(&mut out, c.execute_tree(clip_type(ct + 1), ft2, ft1));
                }
            } else {
                write_paths_result(&mut out, c.execute(clip_type(ct), ft1, ft2));
                if flags & 32 != 0 {
                    out.push_str(" |");
                    write_paths_result(&mut out, c.execute(clip_type(ct + 1), ft2, ft1));
                }
            }
        }
        "O" => {
            let miter: f64 = t.next();
            let arc_tol: f64 = t.next();
            let delta: f64 = t.next();
            let tree: u32 = t.next();
            let groups: u32 = t.next();
            let mut o = ClipperOffset::new(miter, arc_tol);
            for _ in 0..groups {
                let jt: u32 = t.next();
                let et: u32 = t.next();
                o.add_paths(&t.paths(), join_type(jt), end_type(et));
            }
            // Upstream ignores failed executions of the internal clipper
            // (empty result).
            if tree != 0 {
                write_tree(&mut out, &o.execute_tree(delta).unwrap_or_default());
            } else {
                out.push('R');
                write_paths(&mut out, &o.execute(delta).unwrap_or_default());
            }
        }
        "P" => {
            let x = t.next();
            let y = t.next();
            let p = t.path();
            out.push_str(&format!(
                "I {}",
                clipper::point_in_polygon(IntPoint::new(x, y), &p)
            ));
        }
        "A" => {
            let p = t.path();
            out.push_str(&format!(
                "A {:016x} {}",
                clipper::area(&p).to_bits(),
                u8::from(clipper::orientation(&p))
            ));
        }
        "L" => {
            let dist: f64 = t.next();
            let p = t.path();
            out.push('R');
            write_path(&mut out, &clipper::clean_polygon(&p, dist));
        }
        "S" => {
            let fill: u32 = t.next();
            let ps = t.paths();
            write_paths_result(&mut out, clipper::simplify_polygons(&ps, fill_type(fill)));
        }
        "M" => {
            let kind: u32 = t.next();
            let closed: u32 = t.next();
            let pattern = t.path();
            let ps = t.paths();
            let r = match kind {
                0 => clipper::minkowski_sum(&pattern, &ps[0], closed != 0),
                1 => clipper::minkowski_sum_paths(&pattern, &ps, closed != 0),
                _ => clipper::minkowski_diff(&pattern, &ps[0]),
            };
            write_paths_result(&mut out, r);
        }
        "Q" => {
            let n: usize = t.next();
            let mut v: Vec<(i64, usize)> = (0..n).map(|i| (t.next(), i)).collect();
            clipper::std_sort::sort_by(&mut v, |a, b| a.0 < b.0);
            out.push('Q');
            for x in v {
                out.push_str(&format!(" {}", x.1));
            }
        }
        _ => out.push('?'),
    }
    out
}
