//! Replays test vectors produced by the original C++ Clipper library
//! (`golden.txt`) and checks that the port produces exactly the same output
//! (same vertices in the same order, same path order, same tree structure).
//!
//! The vectors are a subset of a differential test against the C++ library
//! (about one million random cases, all identical), which was run with a
//! one-off tool outside of the repository.

#[test]
fn golden_vectors() {
    let content = include_str!("golden.txt");
    let lines: Vec<&str> = content
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .collect();
    assert_eq!(lines.len() % 2, 0, "cases and results must come in pairs");
    let mut failures = Vec::new();
    for (i, pair) in lines.chunks(2).enumerate() {
        let got = crate::runner::run_case(pair[0]);
        if got != pair[1] {
            failures.push(i);
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ: {failures:?}",
        failures.len(),
        lines.len() / 2
    );
    assert!(lines.len() / 2 >= 300);
}
