//! Embed the workspace Git provenance and Cargo build settings in the server.

use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    String::from_utf8(output.stdout).ok()
}

fn source(root: &Path) -> Option<(String, &'static str)> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    if Path::new(top.trim_end()).canonicalize().ok()? != root {
        return None;
    }

    // Watch the actual Git paths so detached HEADs, branch switches, packed
    // refs and linked worktrees all invalidate cached build metadata.
    let paths = git(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "HEAD",
            "--git-path",
            "index",
            "--git-path",
            "refs",
            "--git-path",
            "packed-refs",
        ],
    )?;
    for path in paths.lines().filter(|path| Path::new(path).exists()) {
        println!("cargo::rerun-if-changed={path}");
    }
    if root.join(".git").is_file() {
        println!("cargo::rerun-if-changed={}", root.join(".git").display());
    }

    // Workspace path dependencies and embedded assets live outside this
    // crate. Watch tracked files, including any that are currently deleted.
    let tracked = git(root, &["ls-files", "-z"])?;
    for path in tracked.split_terminator('\0') {
        let path = root.join(path);
        if path.to_str()?.contains(['\r', '\n']) {
            return None;
        }
        println!("cargo::rerun-if-changed={}", path.display());
    }

    let head = git(root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let head = head.trim_end();
    let status = git(root, &["status", "--porcelain=v1", "--untracked-files=no"]);
    let state = match status {
        Some(status) if status.is_empty() => "clean",
        Some(_) => "dirty",
        None => "unknown",
    };
    Some((head.to_owned(), state))
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("set by Cargo");
    let root = Path::new(&manifest).join("../..").canonicalize();
    let provenance = root.ok().and_then(|root| source(&root));
    let (commit, state) = provenance.unwrap_or_else(|| (String::new(), "unknown"));
    println!("cargo::rustc-env=FOILFERN_SOURCE_COMMIT={commit}");
    println!("cargo::rustc-env=FOILFERN_TRACKED_SOURCE_STATE={state}");
    for key in ["PROFILE", "OPT_LEVEL"] {
        let value = std::env::var(key).expect("set by Cargo");
        println!("cargo::rustc-env=FOILFERN_CARGO_{key}={value}");
    }
}
